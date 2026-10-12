//! StageLink native audio service.
//!
//! Spawned and supervised by StageLink Core (the Electron main process), which
//! passes it a pipe instance id and a one-time handshake token. It does not
//! listen on any network port for control traffic and it does not read
//! configuration from anywhere except its own command line and its database.
//!
//! ## Lifecycle, and the question of outliving the UI
//!
//! The service is a child of the desktop application and shuts down with it.
//! That is a deliberate product decision rather than a limitation: a monitor
//! feed with no operator UI attached is a hazard. Nobody can see clipping,
//! nobody can stop it, and performers wearing in-ear monitors have no way to
//! intervene. So quitting StageLink Core shuts the service down in an orderly
//! way — notifying receivers, draining send queues, releasing the capture
//! device — and quitting during a live session is gated on explicit
//! confirmation in the UI.
//!
//! The renderer is a different matter entirely. Reloading the Nuxt window, or
//! an HMR update in development, drops the control channel and nothing else:
//! the service keeps capturing, keeps streaming, and accepts the next client
//! that presents the token. Audio does not stop because a window reloaded.

// The modules live in the library target (src/lib.rs) so the protocol
// cross-check tests can drive the real implementation.
use stagelink_service::{state, telemetry_task};

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use desktop_ipc::{IpcServer, ServerConfig};
use persistence::{Database, SecretStore};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use stagelink_service::handler::StageLinkHandler;
use stagelink_service::state::ServiceState;

#[derive(Parser, Debug)]
#[command(
    name = "stagelink-service",
    about = "StageLink native audio service",
    version
)]
struct Args {
    /// Named-pipe instance id. Supplied by StageLink Core.
    #[arg(long)]
    instance_id: Option<String>,

    /// One-time handshake token. Supplied by StageLink Core.
    ///
    /// Passed on the command line rather than in an environment variable so it
    /// is not inherited by any process this one spawns.
    #[arg(long)]
    token: Option<String>,

    /// Directory for the database, secrets and logs.
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Log level: error, warn, info, debug or trace.
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Use an in-memory database; nothing is persisted. For testing.
    #[arg(long)]
    ephemeral: bool,

    /// Treat this as a packaged release build, which refuses the plaintext
    /// audio transport.
    #[arg(long)]
    release_build: bool,

    /// Print the pipe path and token that were generated, then exit.
    ///
    /// Lets the service be started by hand for debugging without Electron.
    #[arg(long)]
    print_connection: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing(&args.log_level);

    let data_dir = resolve_data_dir(args.data_dir.clone())?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %data_dir.display(),
        release_build = args.release_build,
        "StageLink service starting"
    );

    // The service is spawned with a pipe id and token. If they are absent it
    // generates its own and prints them, which is how it can be driven by hand.
    let config = match (args.instance_id.clone(), args.token.clone()) {
        (Some(instance_id), Some(token)) => ServerConfig { instance_id, token },
        (None, None) => {
            let generated = ServerConfig::generate();
            println!("STAGELINK_PIPE={}", generated.pipe_path());
            println!("STAGELINK_TOKEN={}", generated.token);
            generated
        }
        _ => anyhow::bail!("--instance-id and --token must be supplied together"),
    };

    if args.print_connection {
        println!("{}", config.pipe_path());
        return Ok(());
    }

    let database = if args.ephemeral {
        Database::open_in_memory().context("opening an in-memory database")?
    } else {
        Database::open(data_dir.join("stagelink.db")).context("opening the StageLink database")?
    };
    info!(
        schema_version = database.schema_version().unwrap_or(0),
        "database ready"
    );

    let secrets = SecretStore::new(data_dir.join("secrets")).context("opening the secret store")?;
    let state = ServiceState::new(database, secrets, args.release_build);

    // A multi-threaded runtime: enumeration and device opening run on blocking
    // workers so telemetry keeps flowing while a driver takes its time. The
    // capture thread is a plain OS thread outside the runtime entirely.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("stagelink-rt")
        .enable_all()
        .build()
        .context("building the Tokio runtime")?;

    runtime.block_on(async move { serve(config, state).await })
}

async fn serve(config: ServerConfig, state: Arc<ServiceState>) -> Result<()> {
    let handler = Arc::new(StageLinkHandler::new(Arc::clone(&state)));

    // Reopen the previously selected device so a restart comes up on the same
    // input without the operator touching anything. A failure here is logged,
    // not fatal: the UI will show the device as unavailable and let them pick
    // another, which is far better than refusing to start before a service.
    restore_audio_device(&state).await;

    let (telemetry_shutdown_tx, telemetry_shutdown_rx) = tokio::sync::watch::channel(false);
    let telemetry = tokio::spawn(telemetry_task::run(
        Arc::clone(&handler),
        telemetry_shutdown_rx,
    ));

    let (ipc_shutdown_tx, ipc_shutdown_rx) = tokio::sync::oneshot::channel();
    let server = IpcServer::new(config, Arc::clone(&handler))
        .context("creating the control channel")?;
    let pipe_path = server.pipe_path();
    info!(pipe = %pipe_path, "control channel ready");

    let ipc = tokio::spawn(async move { server.run(ipc_shutdown_rx).await });

    // Ctrl-C is the signal a hand-started service gets; Electron closes the
    // pipe and terminates the child.
    tokio::select! {
        result = ipc => {
            match result {
                Ok(Ok(())) => info!("control channel closed"),
                Ok(Err(error)) => error!(%error, "control channel failed"),
                Err(error) => error!(%error, "control channel task panicked"),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            info!("interrupt received; shutting down");
            let _ = ipc_shutdown_tx.send(());
        }
    }

    // Orderly shutdown: stop telemetry, then release the capture device.
    let _ = telemetry_shutdown_tx.send(true);
    let _ = telemetry.await;
    state.audio.close(state::now_ms());
    info!("StageLink service stopped");
    Ok(())
}

/// Reopens the persisted capture device, if there is one.
async fn restore_audio_device(state: &Arc<ServiceState>) {
    let settings = state.settings_snapshot();
    let Some(device_id) = settings.audio.device_id.clone() else {
        info!("no audio device has been configured yet");
        return;
    };

    let state = Arc::clone(state);
    let channel_index = settings.audio.channel_index;
    let sample_rate = settings.audio.sample_rate;
    let share_mode = settings.share_mode();
    let device_name = settings.audio.device_name.clone().unwrap_or_default();

    let result = tokio::task::spawn_blocking(move || {
        state
            .audio
            .open(&device_id, channel_index, sample_rate, share_mode, state::now_ms())
    })
    .await;

    match result {
        Ok(Ok(status)) => info!(
            device = %status.device.as_ref().map_or("", |device| device.name.as_str()),
            channel = channel_index,
            "restored the configured audio input"
        ),
        Ok(Err(error)) => warn!(
            device = %device_name,
            %error,
            "could not reopen the configured audio input; the UI will report it as unavailable"
        ),
        Err(error) => warn!(%error, "the device restore task failed"),
    }
}

/// Resolves the data directory, creating it if needed.
fn resolve_data_dir(provided: Option<PathBuf>) -> Result<PathBuf> {
    let directory = match provided {
        Some(path) => path,
        None => dirs::data_local_dir()
            .context("could not determine the local app data directory")?
            .join("StageLink"),
    };
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating the data directory {}", directory.display()))?;
    Ok(directory)
}

/// Sets up structured logging to stderr, which Electron captures.
fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_env("STAGELINK_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("stagelink_service={level},audio_capture={level},desktop_ipc={level},persistence=warn,{level}")));

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // stderr, so stdout stays clean for the connection details the service
        // prints when started by hand.
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(false)
        .compact();

    if subscriber.try_init().is_err() {
        // Already initialised (a test harness, most likely). Not fatal.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_accepts_the_arguments_electron_passes() {
        let args = Args::try_parse_from([
            "stagelink-service",
            "--instance-id",
            "abc123",
            "--token",
            "deadbeefdeadbeefdeadbeefdeadbeef",
            "--log-level",
            "debug",
            "--release-build",
        ])
        .unwrap();
        assert_eq!(args.instance_id.as_deref(), Some("abc123"));
        assert_eq!(args.log_level, "debug");
        assert!(args.release_build);
        assert!(!args.ephemeral);
    }

    #[test]
    fn the_cli_runs_with_no_arguments_at_all() {
        let args = Args::try_parse_from(["stagelink-service"]).unwrap();
        assert!(args.instance_id.is_none());
        assert!(args.token.is_none());
        assert_eq!(args.log_level, "info");
        assert!(
            !args.release_build,
            "a hand-started service must not claim to be a release build"
        );
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        assert!(Args::try_parse_from(["stagelink-service", "--run-arbitrary-code"]).is_err());
    }

    #[test]
    fn the_data_directory_is_created_on_demand() {
        let temp = std::env::temp_dir().join(format!("stagelink-dd-{}", uuid::Uuid::new_v4()));
        let resolved = resolve_data_dir(Some(temp.clone())).unwrap();
        assert_eq!(resolved, temp);
        assert!(temp.is_dir());
        std::fs::remove_dir_all(&temp).ok();
    }

    #[test]
    fn resolving_with_no_argument_picks_a_local_app_data_path() {
        let resolved = resolve_data_dir(None).unwrap();
        assert!(resolved.ends_with("StageLink"));
        assert!(resolved.is_dir());
    }
}

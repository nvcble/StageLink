//! StageLink headless receiver.
//!
//! Joins a real desktop from a real QR payload and writes what it hears to a
//! WAV file. **This is the end-to-end test for the desktop half**: a
//! dashboard saying "streaming" proves nothing, and a file you can listen to
//! proves a great deal.
//!
//! ```text
//! # Paste the QR payload the Receivers page shows
//! stagelink-receiver --qr "stagelink://join?h=10.0.0.1&c=45871&s=45872&t=9f2c…&v=1.0.0" \
//!     --duration-secs 30 --output monitor.wav
//!
//! # Two at once, to prove multi-receiver streaming
//! stagelink-receiver --qr "..." --output one.wav &
//! stagelink-receiver --qr "..." --output two.wav --device-name "Second" --key cd
//! ```
//!
//! It is not a simulation of the mobile app. It is the app's own lower half —
//! the same `receiver-client` and the same `receiver-core` — with a file where
//! the audio sink will be. That is why testing this proves something about the
//! app.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use receiver_client::{
    CONTROL_PROTOCOL_VERSION, ControlEvent, DeviceIdentity, JoinPayload, ReceiverSession,
    SessionConfig,
};
use receiver_core::receiver::Preset;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "stagelink-receiver",
    about = "Joins a StageLink desktop and records what it hears",
    version
)]
struct Args {
    /// The QR payload from the desktop's Receivers page.
    #[arg(long)]
    qr: String,

    /// Write the received audio here.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Stop after this many seconds.
    #[arg(long, default_value_t = 30)]
    duration_secs: u64,

    /// Name shown in the desktop's roster.
    #[arg(long, default_value = "Headless Receiver")]
    device_name: String,

    /// Seed for this receiver's long-term key, so two instances are two
    /// devices rather than one reconnecting.
    #[arg(long, default_value = "ab")]
    key: String,

    /// Bind an OS-chosen audio port instead of the one in the QR code.
    ///
    /// Needed to run more than one receiver on a single machine, since two
    /// processes cannot hold one UDP port. The bound port is declared in the
    /// handshake, so the desktop sends to the right place either way.
    #[arg(long)]
    ephemeral_audio_port: bool,

    /// Buffer preset: tight, normal or safe.
    #[arg(long, default_value = "normal")]
    preset: String,

    /// Fail the process if the session did not meet its quality bar.
    #[arg(long)]
    assert_clean: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact()
        .init();

    let args = Args::parse();

    let payload = JoinPayload::parse(&args.qr, CONTROL_PROTOCOL_VERSION)
        .context("the QR payload could not be used")?;
    info!(
        host = %payload.host,
        control_port = payload.control_port,
        stream_port = payload.stream_port,
        "joining"
    );

    let preset = match args.preset.to_ascii_lowercase().as_str() {
        "tight" => Preset::Tight,
        "normal" => Preset::Normal,
        "safe" => Preset::Safe,
        other => anyhow::bail!("unknown preset {other:?}; choose tight, normal or safe"),
    };

    let mut config = SessionConfig::new(DeviceIdentity {
        device_name: args.device_name.clone(),
        // Declared honestly, so an operator debugging a session is not told a
        // test harness is a phone.
        platform: "headless-test".to_owned(),
        platform_version: None,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        public_key: args.key.repeat(32),
        // Filled in by the session once the socket is bound.
        audio_port: None,
    });
    config.preset = preset;
    config.ephemeral_audio_port = args.ephemeral_audio_port;

    let mut session = ReceiverSession::join(&payload, config)
        .await
        .context("could not join the desktop")?;

    let sample_rate = session
        .control_status()
        .welcome()
        .and_then(|welcome| welcome.stream.map(|stream| stream.sample_rate))
        .unwrap_or(48_000);

    // The render loop. On a phone this is the Oboe callback; here it is paced
    // against the clock, which is the same contract — somebody asks for N
    // frames on a deadline and silence is not an acceptable answer.
    let block_frames = 240_usize;
    let interval = Duration::from_secs_f64(block_frames as f64 / f64::from(sample_rate));
    let deadline_end = Instant::now() + Duration::from_secs(args.duration_secs);
    let mut next = Instant::now();
    let mut block = vec![0.0_f32; block_frames];
    let mut audio: Vec<f32> = Vec::new();
    let mut heard_stream = false;

    while Instant::now() < deadline_end {
        for event in session.poll() {
            match event {
                ControlEvent::StreamParams(params) => {
                    info!(
                        session = params.session_id,
                        sample_rate = params.sample_rate,
                        transport = %params.transport,
                        "the operator started a stream"
                    );
                    heard_stream = true;
                }
                ControlEvent::Disconnected(reason) => {
                    warn!(%reason, "the control channel closed");
                    anyhow::bail!("disconnected: {reason}");
                }
                ControlEvent::Connected(_) => {}
            }
        }

        session.render(&mut block);
        audio.extend_from_slice(&block);

        next += interval;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }

    session.leave();

    let telemetry = session.telemetry();
    let socket = session.socket_stats();
    let status = session.control_status();

    info!(
        datagrams = socket.datagrams(),
        foreign = socket.foreign(),
        bytes = socket.bytes(),
        "socket"
    );
    info!(
        received = telemetry.packets_received,
        lost = telemetry.packets_lost,
        loss_percent = format!("{:.2}", telemetry.loss_percent),
        duplicates = telemetry.duplicates,
        reordered = telemetry.reordered,
        rejected = telemetry.rejected,
        "packets"
    );
    info!(
        buffer_ms = format!("{:.1}", telemetry.buffer_health_ms),
        concealed_runs = telemetry.concealed_runs,
        concealed_ms = format!("{:.1}", telemetry.concealed_ms),
        resyncs = telemetry.resyncs,
        drift_ppm = format!("{:.1}", telemetry.resample_ratio_ppm),
        clipped = telemetry.clipped,
        "audio"
    );
    info!(
        heartbeats = status.heartbeats(),
        missed = status.missed_heartbeats(),
        rtt_ms = ?status.rtt_ms(),
        "control"
    );

    if let Some(path) = &args.output {
        write_wav(path, &audio, sample_rate)
            .with_context(|| format!("writing {}", path.display()))?;
        info!(path = %path.display(), frames = audio.len(), "wrote WAV");
    }

    if args.assert_clean {
        assert_clean(&telemetry, socket.datagrams(), heard_stream, &audio)?;
        info!("quality bar met");
    }

    Ok(())
}

/// What a good session looks like.
///
/// Deliberately about what a performer would notice, not about internals.
fn assert_clean(
    telemetry: &receiver_core::receiver::Telemetry,
    datagrams: u64,
    heard_stream: bool,
    audio: &[f32],
) -> Result<()> {
    anyhow::ensure!(
        datagrams > 0,
        "no audio datagrams arrived at all. Is the stream started, and is the firewall \
         allowing the audio port inbound?"
    );
    anyhow::ensure!(
        telemetry.packets_received > 0,
        "{datagrams} datagrams arrived but none were valid StageLink packets"
    );
    anyhow::ensure!(
        telemetry.rejected == 0,
        "{} packets were rejected as malformed",
        telemetry.rejected
    );
    anyhow::ensure!(
        telemetry.clipped == 0,
        "the safety clip fired {} times, which means the limiter let something through",
        telemetry.clipped
    );
    anyhow::ensure!(
        telemetry.loss_percent < 5.0,
        "{:.1}% packet loss",
        telemetry.loss_percent
    );

    // Real audio, not silence. A receiver that reports perfect statistics and
    // outputs nothing is the failure this whole binary exists to catch.
    let peak = audio.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()));
    anyhow::ensure!(
        peak > 0.0001,
        "every packet arrived but the output is digital silence (peak {peak:.6}). Either the \
         desktop is sending silence, or the mixer is not feeding its input."
    );

    if !heard_stream {
        // Not a failure: the stream may have been running before we joined,
        // in which case the parameters came in the welcome instead.
        info!("no stream.params event arrived; the stream was already running when we joined");
    }

    Ok(())
}

fn write_wav(path: &std::path::Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(audio_core::convert::f32_to_i16(sample))?;
    }
    writer.finalize()?;
    Ok(())
}

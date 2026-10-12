//! Shared service state.
//!
//! Everything the control handler and the telemetry task need, behind locks
//! that are only ever held briefly. The database lives behind its own mutex so
//! it is structurally impossible to touch it from anywhere near the audio
//! thread: the audio thread does not have a reference to this type at all.

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use persistence::{Database, SecretStore};
use receiver_session::{JoinCode, ReceiverRoster, Session};
use telemetry::SystemProbe;

use crate::audio_engine::AudioEngine;
use crate::settings::AppSettings;

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// A minted join code together with the address it was minted for.
///
/// The host is kept with the code rather than looked up when the QR is drawn.
/// An operator holding up a code on stage must be holding up what was actually
/// issued; if the network selection changes underneath it, the honest outcome
/// is a code that no longer matches the current adapter — visible on the
/// Network page — not a QR that quietly rewrites itself to a new address.
pub struct MintedJoinCode {
    pub code: JoinCode,
    pub host: String,
}

/// Service-wide state.
pub struct ServiceState {
    pub started_at: Instant,
    /// Behind an `Arc` so the sender thread can hold the engine it reads
    /// captured frames from without the engine having to be cloneable.
    pub audio: Arc<AudioEngine>,
    pub database: Mutex<Database>,
    /// Opened at startup so the protected directory exists before it is
    /// needed. Read when receiver pairing lands in Phase 5.
    #[allow(dead_code, reason = "consumed by Phase 5 pairing")]
    pub secrets: SecretStore,
    pub settings: Mutex<AppSettings>,
    pub session: Mutex<Option<Session>>,
    pub roster: Mutex<ReceiverRoster>,
    /// The join code currently encoded in the QR, if one has been minted.
    /// Replaced wholesale on rotation, so the previous token stops working.
    pub join_code: Mutex<Option<MintedJoinCode>>,
    pub system: SystemProbe,
    /// Network adapter chosen for streaming, mirrored from settings.
    pub selected_adapter: Mutex<Option<String>>,
    /// Whether the renderer has asked for periodic diagnostics pushes.
    pub diagnostics_streaming: Mutex<DiagnosticsSubscription>,
    /// The running audio sender, while a stream is live.
    pub sender: Mutex<Option<ActiveStream>>,
    /// The receiver control server, once an address is known to bind to.
    pub control_server: Mutex<Option<control_server::ControlServer>>,
    /// The renderer event sink.
    ///
    /// Lives here rather than only on the control handler because the control
    /// *server* also has things the operator needs to see — a receiver
    /// joining, leaving or reporting — and it has no handle on the handler.
    pub event_sink: Mutex<Option<desktop_ipc::EventSink>>,
    /// Whether this is a packaged release build. Gates the insecure transport.
    pub release_build: bool,
}

impl ServiceState {
    /// Pushes an event to the renderer, if one is attached.
    ///
    /// Dropped silently otherwise: a renderer that is not listening asks for
    /// the current state when it reconnects, so there is nothing to queue.
    pub fn push_event(&self, event: &str, payload: serde_json::Value) {
        if let Some(sink) = self.event_sink.lock().clone() {
            let _ = sink.send(desktop_ipc::codec::Envelope::event(
                event,
                payload,
                now_ms(),
            ));
        }
    }
}

/// A live stream and what is needed to describe it.
pub struct ActiveStream {
    pub sender: stream_server::StreamSender,
    pub session_id: u32,
    pub started_at: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticsSubscription {
    pub enabled: bool,
    pub interval_ms: u32,
}

impl Default for DiagnosticsSubscription {
    fn default() -> Self {
        Self { enabled: false, interval_ms: 500 }
    }
}

impl ServiceState {
    /// Builds the state, loading and validating persisted settings.
    /// An in-memory service for tests.
    ///
    /// Shared rather than copied into each test module, because every copy was
    /// the same four lines and a secret store in a temporary directory that
    /// has to be unique per test or two tests racing would share one.
    #[cfg(test)]
    #[must_use]
    pub fn for_test() -> Self {
        let database = Database::open_in_memory().unwrap();
        let directory =
            std::env::temp_dir().join(format!("stagelink-state-{}", uuid::Uuid::new_v4()));
        let secrets = SecretStore::new(&directory).unwrap();
        Arc::try_unwrap(Self::new(database, secrets, false))
            .unwrap_or_else(|_| unreachable!("a freshly built state has one reference"))
    }

    pub fn new(database: Database, secrets: SecretStore, release_build: bool) -> Arc<Self> {
        let settings = load_settings(&database);
        let selected_adapter = settings.network.adapter_id.clone();

        Arc::new(Self {
            started_at: Instant::now(),
            audio: Arc::new(AudioEngine::new()),
            database: Mutex::new(database),
            secrets,
            settings: Mutex::new(settings),
            session: Mutex::new(None),
            roster: Mutex::new(ReceiverRoster::new()),
            join_code: Mutex::new(None),
            system: SystemProbe::new(),
            selected_adapter: Mutex::new(selected_adapter),
            diagnostics_streaming: Mutex::new(DiagnosticsSubscription::default()),
            sender: Mutex::new(None),
            control_server: Mutex::new(None),
            event_sink: Mutex::new(None),
            release_build,
        })
    }

    #[must_use]
    pub fn uptime_ms(&self) -> i64 {
        i64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    #[must_use]
    pub fn settings_snapshot(&self) -> AppSettings {
        self.settings.lock().clone()
    }

    /// Persists a settings document group by group.
    pub fn persist_settings(&self, settings: &AppSettings) -> persistence::Result<()> {
        let database = self.database.lock();
        let repository = persistence::SettingsRepository::new(database.connection());
        let at = now_ms();
        repository.set("audio", &settings.audio, at)?;
        repository.set("network", &settings.network, at)?;
        repository.set("streaming", &settings.streaming, at)?;
        repository.set("appearance", &settings.appearance, at)?;
        repository.set("startup", &settings.startup, at)?;
        repository.set("logging", &settings.logging, at)?;
        repository.set("diagnostics", &settings.diagnostics, at)?;
        Ok(())
    }
}

/// Reads settings from the database, falling back to defaults per group.
///
/// A group that fails to decode (a corrupt row, or one written by a future
/// build) is replaced with its defaults and logged, rather than taking the
/// service down. Refusing to start because a UI preference is unreadable would
/// be the wrong trade for a system that has to come up before a service starts.
fn load_settings(database: &Database) -> AppSettings {
    let repository = persistence::SettingsRepository::new(database.connection());
    let mut settings = AppSettings::default();

    macro_rules! load_group {
        ($key:literal, $field:ident) => {
            match repository.get($key) {
                Ok(Some(value)) => settings.$field = value,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        group = $key,
                        %error,
                        "stored settings could not be decoded; using defaults for this group"
                    );
                }
            }
        };
    }

    load_group!("audio", audio);
    load_group!("network", network);
    load_group!("streaming", streaming);
    load_group!("appearance", appearance);
    load_group!("startup", startup);
    load_group!("logging", logging);
    load_group!("diagnostics", diagnostics);

    if let Err(error) = settings.validate() {
        tracing::warn!(
            field = %error.field,
            message = %error.message,
            "persisted settings failed validation; falling back to defaults"
        );
        return AppSettings::default();
    }

    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> Arc<ServiceState> {
        let database = Database::open_in_memory().unwrap();
        let temp = std::env::temp_dir().join(format!("stagelink-test-{}", uuid::Uuid::new_v4()));
        let secrets = SecretStore::new(&temp).unwrap();
        ServiceState::new(database, secrets, false)
    }

    #[test]
    fn a_fresh_service_starts_with_default_settings_and_no_session() {
        let state = state();
        let settings = state.settings_snapshot();
        assert_eq!(settings, AppSettings::default());
        assert!(state.session.lock().is_none());
        assert!(state.roster.lock().is_empty());
        assert!(!state.diagnostics_streaming.lock().enabled);
    }

    #[test]
    fn uptime_is_measured_not_assumed() {
        let state = state();
        let first = state.uptime_ms();
        std::thread::sleep(core::time::Duration::from_millis(20));
        assert!(state.uptime_ms() >= first);
    }

    #[test]
    fn settings_survive_a_write_and_reload() {
        let database = Database::open_in_memory().unwrap();
        let temp = std::env::temp_dir().join(format!("stagelink-test-{}", uuid::Uuid::new_v4()));
        let secrets = SecretStore::new(&temp).unwrap();

        let mut settings = AppSettings::default();
        settings.audio.channel_index = 3;
        settings.audio.device_name = Some("Focusrite".to_owned());
        settings.appearance.theme = "light".to_owned();

        let state = ServiceState::new(database, secrets, false);
        state.persist_settings(&settings).unwrap();

        // Reload from the same connection, as a restart would.
        let reloaded = load_settings(&state.database.lock());
        assert_eq!(reloaded.audio.channel_index, 3);
        assert_eq!(reloaded.audio.device_name.as_deref(), Some("Focusrite"));
        assert_eq!(reloaded.appearance.theme, "light");
    }

    #[test]
    fn a_corrupt_settings_group_falls_back_to_defaults_rather_than_failing_to_start() {
        let database = Database::open_in_memory().unwrap();
        database
            .connection()
            .execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('appearance', 'not json', 1)",
                [],
            )
            .unwrap();

        let settings = load_settings(&database);
        assert_eq!(
            settings.appearance.theme, "dark",
            "a corrupt group must fall back to its defaults"
        );
    }

    #[test]
    fn settings_that_fail_validation_are_replaced_wholesale() {
        let database = Database::open_in_memory().unwrap();
        // A frame duration that the packetiser cannot honour.
        database
            .connection()
            .execute(
                "INSERT INTO settings (key, value, updated_at)
                 VALUES ('streaming', '{\"frameDurationMs\":20.0}', 1)",
                [],
            )
            .unwrap();

        let settings = load_settings(&database);
        assert_eq!(
            settings.streaming.frame_duration_ms, 5.0,
            "an invalid persisted configuration must not reach the audio engine"
        );
    }

    #[test]
    fn a_patch_applied_through_the_state_is_validated() {
        let state = state();
        let current = state.settings_snapshot();
        assert!(
            current
                .apply_patch(&json!({ "audio": { "sampleRate": 2 } }))
                .is_err()
        );
    }
}

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

/// Service-wide state.
pub struct ServiceState {
    pub started_at: Instant,
    pub audio: AudioEngine,
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
    pub join_code: Mutex<Option<JoinCode>>,
    pub system: SystemProbe,
    /// Network adapter chosen for streaming, mirrored from settings.
    pub selected_adapter: Mutex<Option<String>>,
    /// Whether the renderer has asked for periodic diagnostics pushes.
    pub diagnostics_streaming: Mutex<DiagnosticsSubscription>,
    /// Whether this is a packaged release build. Gates the insecure transport.
    pub release_build: bool,
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
    pub fn new(database: Database, secrets: SecretStore, release_build: bool) -> Arc<Self> {
        let settings = load_settings(&database);
        let selected_adapter = settings.network.adapter_id.clone();

        Arc::new(Self {
            started_at: Instant::now(),
            audio: AudioEngine::new(),
            database: Mutex::new(database),
            secrets,
            settings: Mutex::new(settings),
            session: Mutex::new(None),
            roster: Mutex::new(ReceiverRoster::new()),
            join_code: Mutex::new(None),
            system: SystemProbe::new(),
            selected_adapter: Mutex::new(selected_adapter),
            diagnostics_streaming: Mutex::new(DiagnosticsSubscription::default()),
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
            match repository.get(($key)) {
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

//! Persisted settings, mirroring `@stagelink/shared-protocol`.
//!
//! The field names and defaults here must match `appSettingsSchema` exactly —
//! the renderer validates what we send with the zod schema, so a mismatch is a
//! runtime error rather than a compile error. `tests/protocol` drives real
//! settings documents through both to keep them aligned.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioSettings {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub channel_index: u16,
    pub sample_rate: u32,
    pub share_mode: String,
    pub auto_reconnect: bool,
    pub hot_signal_warning_db: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            device_id: None,
            device_name: None,
            channel_index: 0,
            sample_rate: 48_000,
            share_mode: "shared".to_owned(),
            auto_reconnect: true,
            hot_signal_warning_db: -3.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkSettings {
    pub adapter_id: Option<String>,
    pub bind_address: Option<String>,
    pub stream_port: u16,
    pub control_port: u16,
    pub discovery_enabled: bool,
    pub discovery_instance_name: String,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            adapter_id: None,
            bind_address: None,
            stream_port: 45_872,
            control_port: 45_871,
            discovery_enabled: true,
            discovery_instance_name: "StageLink Core".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamingSettings {
    pub frame_duration_ms: f64,
    pub payload_format: String,
    pub transport: String,
    pub max_receivers: u16,
    pub per_receiver_queue_packets: usize,
    pub auto_start_streaming: bool,
    pub require_approval_every_join: bool,
}

impl Default for StreamingSettings {
    fn default() -> Self {
        Self {
            frame_duration_ms: 5.0,
            payload_format: "pcm_s16le".to_owned(),
            // Secure by default. A development build may override this via the
            // UI; a release build refuses the plaintext transport outright.
            transport: "udp-encrypted".to_owned(),
            max_receivers: 16,
            per_receiver_queue_packets: 64,
            auto_start_streaming: false,
            require_approval_every_join: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppearanceSettings {
    pub theme: String,
    pub meter_style: String,
    pub reduce_motion: bool,
    pub meter_refresh_hz: u8,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "dark".to_owned(),
            meter_style: "peak+rms".to_owned(),
            reduce_motion: false,
            meter_refresh_hz: 25,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartupSettings {
    pub launch_on_login: bool,
    pub start_minimised_to_tray: bool,
    pub show_setup_wizard: bool,
    pub confirm_quit_during_session: bool,
}

impl Default for StartupSettings {
    fn default() -> Self {
        Self {
            launch_on_login: false,
            start_minimised_to_tray: false,
            show_setup_wizard: true,
            confirm_quit_during_session: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoggingSettings {
    pub level: String,
    pub retain_files: u8,
    pub persist_diagnostics: bool,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            retain_files: 7,
            persist_diagnostics: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiagnosticsSettings {
    pub stream_while_visible: bool,
    pub interval_ms: u32,
    pub chart_history_points: u32,
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self {
            stream_while_visible: true,
            interval_ms: 500,
            chart_history_points: 300,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub audio: AudioSettings,
    pub network: NetworkSettings,
    pub streaming: StreamingSettings,
    pub appearance: AppearanceSettings,
    pub startup: StartupSettings,
    pub logging: LoggingSettings,
    pub diagnostics: DiagnosticsSettings,
}

/// Settings groups, used as the SQLite `settings` table keys.
pub const SETTINGS_GROUPS: [&str; 7] = [
    "audio",
    "network",
    "streaming",
    "appearance",
    "startup",
    "logging",
    "diagnostics",
];

/// Validation failure, reported with the offending field path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsValidationError {
    pub field: String,
    pub message: String,
}

impl AppSettings {
    /// Checks every constraint the audio engine and transport depend on.
    ///
    /// Run on read as well as write: a hand-edited database must not be able to
    /// push the engine into an invalid configuration.
    pub fn validate(&self) -> Result<(), SettingsValidationError> {
        let fail = |field: &str, message: &str| SettingsValidationError {
            field: field.to_owned(),
            message: message.to_owned(),
        };

        if !(8_000..=192_000).contains(&self.audio.sample_rate) {
            return Err(fail("audio.sampleRate", "must be between 8000 and 192000 Hz"));
        }
        if !matches!(self.audio.share_mode.as_str(), "shared" | "exclusive") {
            return Err(fail("audio.shareMode", "must be \"shared\" or \"exclusive\""));
        }
        if !(-24.0..=0.0).contains(&self.audio.hot_signal_warning_db) {
            return Err(fail(
                "audio.hotSignalWarningDb",
                "must be between -24 and 0 dBFS",
            ));
        }

        for (field, port) in [
            ("network.streamPort", self.network.stream_port),
            ("network.controlPort", self.network.control_port),
        ] {
            if port < 1_024 {
                return Err(fail(field, "must be above 1023"));
            }
        }
        if self.network.stream_port == self.network.control_port {
            return Err(fail(
                "network.streamPort",
                "must differ from the control port",
            ));
        }
        if self.network.discovery_instance_name.trim().is_empty()
            || self.network.discovery_instance_name.len() > 63
        {
            return Err(fail(
                "network.discoveryInstanceName",
                "must be between 1 and 63 characters",
            ));
        }

        if !stream_server::config::SUPPORTED_FRAME_DURATIONS_MS
            .contains(&self.streaming.frame_duration_ms)
        {
            return Err(fail(
                "streaming.frameDurationMs",
                "must be 2.5, 5 or 10 milliseconds",
            ));
        }
        if self.streaming.payload_format != "pcm_s16le" {
            return Err(fail("streaming.payloadFormat", "must be \"pcm_s16le\""));
        }
        if !matches!(
            self.streaming.transport.as_str(),
            "udp-plaintext" | "udp-encrypted"
        ) {
            return Err(fail("streaming.transport", "is not a known transport"));
        }
        if !(1..=64).contains(&self.streaming.max_receivers) {
            return Err(fail("streaming.maxReceivers", "must be between 1 and 64"));
        }
        if !(2..=512).contains(&self.streaming.per_receiver_queue_packets) {
            return Err(fail(
                "streaming.perReceiverQueuePackets",
                "must be between 2 and 512",
            ));
        }

        if !matches!(self.appearance.theme.as_str(), "dark" | "light" | "system") {
            return Err(fail("appearance.theme", "must be dark, light or system"));
        }
        if !(10..=60).contains(&self.appearance.meter_refresh_hz) {
            return Err(fail("appearance.meterRefreshHz", "must be between 10 and 60"));
        }

        if !matches!(
            self.logging.level.as_str(),
            "error" | "warn" | "info" | "debug" | "trace"
        ) {
            return Err(fail("logging.level", "is not a known log level"));
        }
        if !(1..=60).contains(&self.logging.retain_files) {
            return Err(fail("logging.retainFiles", "must be between 1 and 60"));
        }

        if !(200..=10_000).contains(&self.diagnostics.interval_ms) {
            return Err(fail(
                "diagnostics.intervalMs",
                "must be between 200 and 10000 ms",
            ));
        }
        if !(30..=3_600).contains(&self.diagnostics.chart_history_points) {
            return Err(fail(
                "diagnostics.chartHistoryPoints",
                "must be between 30 and 3600",
            ));
        }

        Ok(())
    }

    /// Applies a partial JSON patch, group by group.
    ///
    /// Each group is merged and re-validated, so a patch that would produce an
    /// invalid configuration is rejected whole rather than half-applied.
    pub fn apply_patch(&self, patch: &serde_json::Value) -> Result<Self, SettingsValidationError> {
        let mut current = serde_json::to_value(self).map_err(|error| SettingsValidationError {
            field: String::new(),
            message: format!("could not serialise current settings: {error}"),
        })?;

        if let (Some(target), Some(incoming)) = (current.as_object_mut(), patch.as_object()) {
            for (group, values) in incoming {
                if !SETTINGS_GROUPS.contains(&group.as_str()) {
                    return Err(SettingsValidationError {
                        field: group.clone(),
                        message: "is not a known settings group".to_owned(),
                    });
                }
                let Some(group_patch) = values.as_object() else {
                    return Err(SettingsValidationError {
                        field: group.clone(),
                        message: "must be an object".to_owned(),
                    });
                };
                if let Some(existing) = target.get_mut(group).and_then(|v| v.as_object_mut()) {
                    for (key, value) in group_patch {
                        existing.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        let merged: Self = serde_json::from_value(current).map_err(|error| {
            SettingsValidationError {
                field: String::new(),
                message: format!("patch produced an invalid settings document: {error}"),
            }
        })?;
        merged.validate()?;
        Ok(merged)
    }

    /// Settings paths in `patch` that only take effect after a stream restart.
    #[must_use]
    pub fn restart_required_for(patch: &serde_json::Value) -> Vec<String> {
        const RESTART_PATHS: [(&str, &str); 11] = [
            ("audio", "deviceId"),
            ("audio", "channelIndex"),
            ("audio", "sampleRate"),
            ("audio", "shareMode"),
            ("network", "adapterId"),
            ("network", "bindAddress"),
            ("network", "streamPort"),
            ("network", "controlPort"),
            ("streaming", "frameDurationMs"),
            ("streaming", "payloadFormat"),
            ("streaming", "transport"),
        ];
        RESTART_PATHS
            .iter()
            .filter(|(group, key)| {
                patch
                    .get(group)
                    .and_then(|values| values.get(key))
                    .is_some()
            })
            .map(|(group, key)| format!("{group}.{key}"))
            .collect()
    }

    /// The capture share mode as the audio engine understands it.
    #[must_use]
    pub fn share_mode(&self) -> audio_core::ShareMode {
        if self.audio.share_mode == "exclusive" {
            audio_core::ShareMode::Exclusive
        } else {
            audio_core::ShareMode::Shared
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_defaults_are_valid() {
        AppSettings::default().validate().unwrap();
    }

    #[test]
    fn defaults_match_the_shared_protocol_schema() {
        // These exact values are asserted on the TypeScript side too; if one
        // moves, this test and its counterpart both have to be updated
        // deliberately rather than drifting apart.
        let settings = AppSettings::default();
        assert_eq!(settings.audio.sample_rate, 48_000);
        assert_eq!(settings.audio.channel_index, 0);
        assert_eq!(settings.streaming.frame_duration_ms, 5.0);
        assert_eq!(settings.streaming.transport, "udp-encrypted");
        assert_eq!(settings.streaming.max_receivers, 16);
        assert_eq!(settings.network.stream_port, 45_872);
        assert_eq!(settings.network.control_port, 45_871);
        assert!(settings.startup.show_setup_wizard);
        assert_eq!(settings.appearance.theme, "dark");
    }

    #[test]
    fn the_wire_form_is_camel_case() {
        let json = serde_json::to_string(&AppSettings::default()).unwrap();
        assert!(json.contains("\"sampleRate\""), "got {json}");
        assert!(json.contains("\"frameDurationMs\""));
        assert!(json.contains("\"showSetupWizard\""));
        assert!(!json.contains("sample_rate"));
    }

    #[test]
    fn a_partial_document_fills_in_defaults() {
        let settings: AppSettings = serde_json::from_value(json!({
            "audio": { "channelIndex": 3 }
        }))
        .unwrap();
        assert_eq!(settings.audio.channel_index, 3);
        assert_eq!(settings.audio.sample_rate, 48_000, "unspecified keys default");
        assert_eq!(settings.network.stream_port, 45_872);
    }

    #[test]
    fn a_patch_merges_without_clobbering_sibling_keys() {
        let settings = AppSettings::default();
        let patched = settings
            .apply_patch(&json!({ "audio": { "channelIndex": 2 } }))
            .unwrap();
        assert_eq!(patched.audio.channel_index, 2);
        assert_eq!(patched.audio.sample_rate, 48_000);
        assert!(patched.audio.auto_reconnect);
    }

    #[test]
    fn a_patch_that_would_be_invalid_is_rejected_whole() {
        let settings = AppSettings::default();
        let error = settings
            .apply_patch(&json!({ "streaming": { "frameDurationMs": 20.0 } }))
            .expect_err("20 ms must be refused");
        assert_eq!(error.field, "streaming.frameDurationMs");
        // The original is untouched.
        assert_eq!(settings.streaming.frame_duration_ms, 5.0);
    }

    #[test]
    fn an_unknown_settings_group_is_refused() {
        let error = AppSettings::default()
            .apply_patch(&json!({ "secrets": { "token": "x" } }))
            .expect_err("unknown groups must be refused");
        assert_eq!(error.field, "secrets");
    }

    #[test]
    fn restart_required_paths_are_detected() {
        let paths = AppSettings::restart_required_for(&json!({
            "audio": { "deviceId": "x", "autoReconnect": false },
            "appearance": { "theme": "light" }
        }));
        assert_eq!(paths, vec!["audio.deviceId".to_owned()]);
    }

    #[test]
    fn appearance_changes_never_require_a_restart() {
        let paths = AppSettings::restart_required_for(&json!({
            "appearance": { "theme": "light", "meterRefreshHz": 60 }
        }));
        assert!(paths.is_empty());
    }

    #[test]
    fn invalid_patches_are_refused_with_the_right_field() {
        let cases: Vec<(serde_json::Value, &str)> = vec![
            (json!({ "audio": { "sampleRate": 1 } }), "audio.sampleRate"),
            (json!({ "audio": { "shareMode": "magic" } }), "audio.shareMode"),
            (json!({ "network": { "streamPort": 80 } }), "network.streamPort"),
            (json!({ "streaming": { "maxReceivers": 0 } }), "streaming.maxReceivers"),
            (json!({ "appearance": { "theme": "neon" } }), "appearance.theme"),
            (json!({ "logging": { "level": "shout" } }), "logging.level"),
            (json!({ "diagnostics": { "intervalMs": 1 } }), "diagnostics.intervalMs"),
        ];
        for (patch, expected_field) in cases {
            match AppSettings::default().apply_patch(&patch) {
                Err(error) => assert_eq!(error.field, expected_field, "for patch {patch}"),
                Ok(_) => panic!("{patch} should have been refused"),
            }
        }
    }

    #[test]
    fn duplicate_ports_are_refused() {
        let error = AppSettings::default()
            .apply_patch(&json!({ "network": { "streamPort": 45_871 } }))
            .expect_err("matching ports must be refused");
        assert_eq!(error.field, "network.streamPort");
    }

    #[test]
    fn share_mode_maps_to_the_audio_core_enum() {
        let mut settings = AppSettings::default();
        assert_eq!(settings.share_mode(), audio_core::ShareMode::Shared);
        settings.audio.share_mode = "exclusive".to_owned();
        assert_eq!(settings.share_mode(), audio_core::ShareMode::Exclusive);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = AppSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, restored);
    }
}

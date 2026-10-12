//! The service's side of the receiver control channel.
//!
//! Implements [`ControlHost`] over the real roster, the real database and the
//! real audio sender, so a receiver joining actually changes what the operator
//! sees and what the socket sends.

use std::sync::Arc;

use control_server::join::Admission;
use control_server::server::ControlHost;
use persistence::{ReceiverRecord, ReceiverRepository};
use receiver_session::{JoinCode, PlaybackState, ReceiverReportedTelemetry};
use serde_json::Value;
use tracing::{debug, warn};

/// Pushes a renderer event. A type alias because the closure type is noisy
/// enough to obscure the signatures that carry it.
pub type NotifyFn = Arc<dyn Fn(&str, Value) + Send + Sync>;

use crate::state::{ServiceState, now_ms};

/// Where a receiver's audio is sent.
///
/// The address the control connection came from, with the **stream port** from
/// the configuration rather than the control connection's ephemeral source
/// port. A receiver binds its audio socket to the port it advertised in the QR
/// code; its control connection comes from whatever source port the OS handed
/// it, which is not where it is listening for audio.
fn audio_destination(
    admission: &Admission,
    stream_port: u16,
) -> std::net::SocketAddr {
    let mut address = admission.address;
    address.set_port(stream_port);
    address
}

/// Bridges the control server to the rest of the service.
pub struct ServiceControlHost {
    state: Arc<ServiceState>,
    /// Pushes a renderer event. Set by the service once the pipe is up.
    notify: NotifyFn,
}

impl ServiceControlHost {
    #[must_use]
    pub fn new(
        state: Arc<ServiceState>,
        notify: NotifyFn,
    ) -> Self {
        Self { state, notify }
    }

    /// Pushes the roster to the renderer.
    ///
    /// Called on every join, departure and telemetry report, because the
    /// Receivers page is the operator's only window into whether the band can
    /// hear anything.
    fn push_roster(&self) {
        let roster = self.state.roster.lock();
        let receivers: Vec<Value> = roster
            .entries()
            .iter()
            .map(crate::serialize::roster_entry)
            .collect();
        drop(roster);
        (self.notify)("receivers.changed", serde_json::json!({ "receivers": receivers }));
    }

    /// Records the receiver in SQLite so its label and its authorisation
    /// outlive a restart.
    fn persist(&self, admission: &Admission, now: i64) {
        let database = self.state.database.lock();
        let repository = ReceiverRepository::new(database.connection());

        // Keep the operator's label and the first-seen time if we have met
        // this device before.
        let existing = repository.get(&admission.receiver_id).ok().flatten();
        let record = ReceiverRecord {
            id: admission.receiver_id.clone(),
            device_name: admission.identity.device_name.clone(),
            display_name: existing.as_ref().and_then(|row| row.display_name.clone()),
            platform: admission.identity.platform.as_wire().to_owned(),
            platform_version: admission.identity.platform_version.clone(),
            app_version: admission.identity.app_version.clone(),
            public_key: existing.as_ref().and_then(|row| row.public_key.clone()),
            key_fingerprint: existing.as_ref().and_then(|row| row.key_fingerprint.clone()),
            pairing_state: "approved".to_owned(),
            last_ip_address: Some(admission.address.ip().to_string()),
            first_seen_at: existing.as_ref().map_or(now, |row| row.first_seen_at),
            last_seen_at: now,
            approved_at: Some(now),
            revoked_at: None,
        };
        if let Err(error) = repository.upsert(&record) {
            // Not fatal. A receiver that cannot be written to the database can
            // still hear the mix for this session; losing its label after a
            // restart is worse than nothing but far better than refusing it.
            warn!(%error, receiver = %admission.receiver_id, "could not persist the receiver");
        }
    }
}

impl ControlHost for ServiceControlHost {
    fn join_code(&self) -> Option<JoinCode> {
        self.state
            .join_code
            .lock()
            .as_ref()
            .map(|minted| minted.code.clone())
    }

    fn control_protocol_version(&self) -> String {
        crate::handler::CONTROL_PROTOCOL_VERSION.to_owned()
    }

    fn service_version(&self) -> String {
        crate::handler::SERVICE_VERSION.to_owned()
    }

    fn max_receivers(&self) -> u16 {
        self.state.settings_snapshot().streaming.max_receivers
    }

    fn is_revoked(&self, receiver_id: &str) -> bool {
        let database = self.state.database.lock();
        let repository = ReceiverRepository::new(database.connection());
        // A database error is treated as *not* revoked rather than as
        // revoked: failing closed here would lock the whole band out of their
        // monitors because of an unrelated storage fault.
        repository
            .get(receiver_id)
            .ok()
            .flatten()
            .is_some_and(|record| record.pairing_state == "revoked")
    }

    fn stream_params(&self) -> Option<Value> {
        let active = self.state.sender.lock();
        let active = active.as_ref()?;
        let config = active.sender.config();
        Some(serde_json::json!({
            "sessionId": active.session_id,
            // The only place a receiver learns the rate. The packet header
            // does not carry it.
            "sampleRate": config.sample_rate,
            "frameDurationMs": config.frame_duration_ms,
            "payloadFormat": "pcm_s16le",
            "transport": config.transport.as_str(),
            "streamPort": config.stream_port,
            "framesPerPacket": config.frames_per_packet(),
        }))
    }

    fn on_joined(&self, admission: &Admission) {
        let now = now_ms();

        self.state.roster.lock().join(
            &admission.receiver_id,
            admission.identity.clone(),
            Some(admission.address.ip().to_string()),
            now,
        );

        // Restore the operator's label from a previous session.
        let stored = {
            let database = self.state.database.lock();
            ReceiverRepository::new(database.connection())
                .get(&admission.receiver_id)
                .ok()
                .flatten()
        };
        if let Some(display_name) = stored.and_then(|record| record.display_name) {
            self.state
                .roster
                .lock()
                .rename(&admission.receiver_id, Some(&display_name));
        }

        self.persist(admission, now);

        // Start sending audio, if a stream is running. If one is not, the
        // receiver is added when it starts.
        if let Some(active) = self.state.sender.lock().as_ref() {
            let destination = audio_destination(admission, active.sender.config().stream_port);
            active
                .sender
                .add_destination(&admission.receiver_id, destination);
            debug!(receiver = %admission.receiver_id, %destination, "sending audio");
        }

        self.push_roster();
    }

    fn on_left(&self, receiver_id: &str) {
        self.state.roster.lock().leave(receiver_id);
        if let Some(active) = self.state.sender.lock().as_ref() {
            active.sender.remove_destination(receiver_id);
        }
        self.push_roster();
    }

    fn on_heartbeat(&self, receiver_id: &str, _sent_at: i64) {
        // The receiver's own clock time is deliberately not stored: it is
        // unverifiable and comparing it to ours would produce a latency figure
        // that looks measured and is not. It is echoed back in the ack so the
        // receiver can compute its own round-trip time, and that is all.
        self.state.roster.lock().record_heartbeat(receiver_id, now_ms());
    }

    fn on_telemetry(&self, receiver_id: &str, report: &Value) {
        let number = |key: &str| report.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let count = |key: &str| report.get(key).and_then(Value::as_u64).unwrap_or(0);

        let telemetry = ReceiverReportedTelemetry {
            // Stamped here, not taken from the report: a receiver must not be
            // able to forge its own provenance or its arrival time.
            received_at: now_ms(),
            reported_at: report
                .get("reportedAt")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            packets_received: count("packetsReceived"),
            packets_lost: count("packetsLost"),
            loss_percent: number("lossPercent"),
            jitter_ms: number("jitterMs"),
            buffer_health_ms: number("bufferHealthMs"),
            underruns: count("underruns"),
            playback_state: PlaybackState::from_wire(
                report
                    .get("playbackState")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
            ),
            output_latency_ms: report.get("outputLatencyMs").and_then(Value::as_f64),
            resample_ratio_ppm: report.get("resampleRatioPpm").and_then(Value::as_f64),
        };

        let mut roster = self.state.roster.lock();
        roster.record_telemetry(receiver_id, telemetry);
        // A report is also proof of life, so it counts as a heartbeat. A
        // receiver whose heartbeats were lost but whose telemetry arrives is
        // plainly still there, and expiring it would cut its audio for no
        // reason.
        roster.record_heartbeat(receiver_id, now_ms());
        drop(roster);

        self.push_roster();
    }

    fn expire_stale(&self, now_ms: i64, timeout_ms: i64) -> Vec<String> {
        let expired = self.state.roster.lock().expire_stale(now_ms, timeout_ms);
        if !expired.is_empty() {
            self.push_roster();
        }
        expired
    }

    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use control_server::join::receiver_id_for;
    use receiver_session::ReceiverIdentity;

    fn admission(key: &str) -> Admission {
        Admission {
            receiver_id: receiver_id_for(key),
            identity: ReceiverIdentity::from_wire(
                "Pixel 8",
                "android",
                Some("14".to_owned()),
                Some("0.1.0".to_owned()),
            ),
            address: "10.0.0.5:51000".parse().unwrap(),
        }
    }

    fn host() -> (Arc<ServiceControlHost>, Arc<ServiceState>, Arc<parking_lot::Mutex<Vec<String>>>) {
        let state = Arc::new(ServiceState::for_test());
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&events);
        let notify = Arc::new(move |event: &str, _payload: Value| {
            recorder.lock().push(event.to_owned());
        });
        (
            Arc::new(ServiceControlHost::new(Arc::clone(&state), notify)),
            state,
            events,
        )
    }

    /// The audio port is the configured one, not the control connection's
    /// ephemeral source port — which is not where the receiver is listening.
    #[test]
    fn audio_goes_to_the_stream_port_not_the_control_source_port() {
        let admission = admission("ab");
        assert_eq!(admission.address.port(), 51_000);
        let destination = audio_destination(&admission, 45_872);
        assert_eq!(destination.port(), 45_872);
        assert_eq!(destination.ip(), admission.address.ip());
    }

    #[test]
    fn joining_puts_the_receiver_in_the_roster_and_tells_the_renderer() {
        let (host, state, events) = host();
        host.on_joined(&admission("ab"));

        let roster = state.roster.lock();
        assert_eq!(roster.len(), 1);
        let entry = &roster.entries()[0];
        assert_eq!(entry.identity.device_name, "Pixel 8");
        assert_eq!(entry.label(), "Pixel 8");
        drop(roster);

        assert!(events.lock().contains(&"receivers.changed".to_owned()));
    }

    #[test]
    fn leaving_removes_it_again() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);
        host.on_left(&admission.receiver_id);
        assert_eq!(state.roster.lock().len(), 0);
    }

    /// The operator renamed a channel strip. That must survive the phone going
    /// into a pocket and coming back, or the labels are useless mid-service.
    #[test]
    fn an_operator_label_is_restored_when_the_device_rejoins() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);

        // The operator renames it, which persists.
        {
            let database = state.database.lock();
            ReceiverRepository::new(database.connection())
                .set_display_name(&admission.receiver_id, "Lead Vocal")
                .unwrap();
        }
        host.on_left(&admission.receiver_id);

        host.on_joined(&admission);
        let roster = state.roster.lock();
        assert_eq!(roster.entries()[0].label(), "Lead Vocal");
    }

    #[test]
    fn a_joining_receiver_is_persisted_so_it_is_known_after_a_restart() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);

        let database = state.database.lock();
        let record = ReceiverRepository::new(database.connection())
            .get(&admission.receiver_id)
            .unwrap()
            .expect("the receiver must be in the database");
        assert_eq!(record.device_name, "Pixel 8");
        assert_eq!(record.pairing_state, "approved");
        assert_eq!(record.last_ip_address.as_deref(), Some("10.0.0.5"));
    }

    #[test]
    fn a_revoked_receiver_is_recognised_as_revoked() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);
        assert!(!host.is_revoked(&admission.receiver_id));

        {
            let database = state.database.lock();
            ReceiverRepository::new(database.connection())
                .revoke(&admission.receiver_id, now_ms())
                .unwrap();
        }
        assert!(host.is_revoked(&admission.receiver_id));
    }

    #[test]
    fn an_unknown_device_is_not_revoked() {
        let (host, _state, _events) = host();
        assert!(!host.is_revoked("never-seen"));
    }

    /// Stamped by the desktop, never taken from the report: a receiver must
    /// not be able to claim its figures were measured here.
    #[test]
    fn telemetry_provenance_is_stamped_rather_than_trusted() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);

        host.on_telemetry(
            &admission.receiver_id,
            &serde_json::json!({
                "reportedAt": 42,
                "receivedAt": 999_999,
                "source": "measured",
                "packetsReceived": 100,
                "packetsLost": 3,
                "lossPercent": 2.9,
                "jitterMs": 1.5,
                "bufferHealthMs": 38.0,
                "underruns": 1,
                "playbackState": "playing",
                "outputLatencyMs": 30.0,
                "resampleRatioPpm": 12.0,
            }),
        );

        let roster = state.roster.lock();
        let telemetry = roster.entries()[0].telemetry.expect("telemetry was recorded");
        assert_eq!(telemetry.reported_at, 42, "the receiver's own clock is kept");
        assert_ne!(
            telemetry.received_at, 999_999,
            "the receiver must not be able to set its own arrival time"
        );
        assert!(telemetry.received_at > 1_600_000_000_000);
        assert_eq!(telemetry.packets_lost, 3);
        assert_eq!(telemetry.playback_state, PlaybackState::Playing);
        assert_eq!(telemetry.resample_ratio_ppm, Some(12.0));
    }

    #[test]
    fn a_nonsense_telemetry_report_does_not_panic_or_poison_the_roster() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);

        for report in [
            serde_json::json!({}),
            serde_json::json!({ "packetsLost": "lots" }),
            serde_json::json!({ "playbackState": "dancing" }),
            Value::Null,
        ] {
            host.on_telemetry(&admission.receiver_id, &report);
        }

        let roster = state.roster.lock();
        let telemetry = roster.entries()[0].telemetry.unwrap();
        assert_eq!(
            telemetry.playback_state,
            PlaybackState::Unknown,
            "an unrecognised state is reported as unknown, not guessed at"
        );
        assert_eq!(telemetry.packets_lost, 0);
    }

    /// A phone whose heartbeats are being lost but whose telemetry arrives is
    /// plainly still there; expiring it would cut its audio for no reason.
    #[test]
    fn a_telemetry_report_counts_as_proof_of_life() {
        let (host, state, _events) = host();
        let admission = admission("ab");
        host.on_joined(&admission);

        // Expire it as far as the roster is concerned.
        let stale = state.roster.lock().expire_stale(now_ms() + 100_000, 5_000);
        assert_eq!(stale.len(), 1);

        host.on_telemetry(&admission.receiver_id, &serde_json::json!({ "reportedAt": 1 }));
        assert_eq!(
            state.roster.lock().expire_stale(now_ms(), 5_000).len(),
            0,
            "a reporting receiver should not still look stale"
        );
    }

    #[test]
    fn the_stream_parameters_are_absent_until_a_stream_runs() {
        let (host, _state, _events) = host();
        assert!(
            host.stream_params().is_none(),
            "joining before the operator starts is normal, not an error"
        );
    }
}

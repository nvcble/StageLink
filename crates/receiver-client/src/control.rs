//! The receiver's control-channel client.
//!
//! Owns the socket, the handshake, the heartbeat and the telemetry reports.
//! **This is Rust, not the app's language, and that is a deliberate
//! architectural choice** — see `docs/mobile-plan.md`:
//!
//! * Five missed one-second heartbeats and the desktop stops sending audio. If
//!   the app's runtime owned the heartbeat, a performer's monitor feed would
//!   depend on a garbage-collected isolate being scheduled every second for 90
//!   minutes, through Doze and OEM power management. That failure mode would
//!   be created by the architecture rather than found in it.
//! * Key material must not touch a garbage-collected heap, where it cannot be
//!   reliably zeroized — which would make "never persist a join token" an
//!   untestable claim.
//!
//! The app above gets the semantics: connection state, the roster, telemetry
//! to display. It never sees the token.

use std::sync::Arc;
use std::time::Duration;

use desktop_ipc::codec::{Envelope, EnvelopeKind, ReceiverIdentity, StreamParams};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::join_payload::JoinPayload;

/// How often a heartbeat is sent. Must stay well inside the desktop's miss
/// limit; five missed heartbeats stops the audio.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(1_000);
/// How often telemetry is reported.
pub const TELEMETRY_INTERVAL: Duration = Duration::from_millis(2_000);
/// How long to wait for the welcome before giving up.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// What the control channel is doing. Surfaced to the app for display.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    /// Refused or dropped, with a reason the app can show a person.
    Failed(String),
}

/// What the desktop told us on admission.
#[derive(Debug, Clone, PartialEq)]
pub struct Welcome {
    pub service_version: String,
    pub control_protocol_version: String,
    pub audio_protocol_version: u8,
    /// Audio parameters, if a stream was already running when we joined.
    ///
    /// `None` is normal — people connect their phones while the band is still
    /// setting up — and the parameters arrive as an event when the operator
    /// starts the stream.
    pub stream: Option<StreamParams>,
    pub key_fingerprint: Option<String>,
}

/// Shared connection state, readable from the app's thread.
#[derive(Default)]
pub struct ControlStatus {
    state: Mutex<ConnectionState>,
    welcome: Mutex<Option<Welcome>>,
    /// Round-trip time of the last heartbeat, in milliseconds.
    rtt_ms: Mutex<Option<f64>>,
    heartbeats: Mutex<u64>,
    missed: Mutex<u64>,
}

impl ControlStatus {
    #[must_use]
    pub fn state(&self) -> ConnectionState {
        self.state.lock().clone()
    }
    #[must_use]
    pub fn welcome(&self) -> Option<Welcome> {
        self.welcome.lock().clone()
    }
    #[must_use]
    pub fn rtt_ms(&self) -> Option<f64> {
        *self.rtt_ms.lock()
    }
    #[must_use]
    pub fn heartbeats(&self) -> u64 {
        *self.heartbeats.lock()
    }
    #[must_use]
    pub fn missed_heartbeats(&self) -> u64 {
        *self.missed.lock()
    }
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self.state(), ConnectionState::Connected)
    }
}

/// Something the control channel learned, for the layer above.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    Connected(Welcome),
    /// The audio parameters changed — a new session, or the stream starting.
    ///
    /// A receiver **must** flush its buffer on a `sessionId` change: the audio
    /// either side of it is not continuous.
    StreamParams(StreamParams),
    Disconnected(String),
}

/// Telemetry the client sends on the app's behalf.
///
/// Passed in rather than read from `receiver-core` directly, so the control
/// client does not have to know how the audio path is wired — and so a test
/// can send a known report.
#[derive(Debug, Clone, Copy, Default)]
pub struct TelemetryReport {
    pub packets_received: u64,
    pub packets_lost: u64,
    pub loss_percent: f64,
    pub jitter_ms: f64,
    pub buffer_health_ms: f64,
    pub underruns: u64,
    pub playback_state: &'static str,
    pub output_latency_ms: Option<f64>,
    pub resample_ratio_ppm: Option<f64>,
}

impl TelemetryReport {
    fn to_params(self, reported_at: i64) -> Value {
        json!({
            "reportedAt": reported_at,
            "packetsReceived": self.packets_received,
            "packetsLost": self.packets_lost,
            "lossPercent": self.loss_percent,
            "jitterMs": self.jitter_ms,
            "bufferHealthMs": self.buffer_health_ms,
            "underruns": self.underruns,
            "playbackState": self.playback_state,
            "outputLatencyMs": self.output_latency_ms,
            "resampleRatioPpm": self.resample_ratio_ppm,
        })
    }
}

/// How the client identifies this device.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub device_name: String,
    pub platform: String,
    pub platform_version: Option<String>,
    pub app_version: String,
    /// Long-term public key, hex. Not a secret; it is how the desktop
    /// recognises this device across reconnects so a label and a revocation
    /// both stick.
    pub public_key: String,
    /// The UDP port this receiver has already bound for audio.
    ///
    /// Declared so the desktop does not have to assume. The socket is bound
    /// *before* the handshake precisely so this is knowable — and so a port
    /// that cannot be opened is discovered before a roster entry exists for a
    /// receiver that can never hear anything.
    pub audio_port: Option<u16>,
}

impl DeviceIdentity {
    fn to_wire(&self) -> ReceiverIdentity {
        ReceiverIdentity {
            device_name: self.device_name.clone(),
            platform: self.platform.clone(),
            platform_version: self.platform_version.clone(),
            app_version: Some(self.app_version.clone()),
            public_key: self.public_key.clone(),
            audio_port: self.audio_port,
        }
    }
}

/// A connected control channel.
pub struct ControlClient {
    status: Arc<ControlStatus>,
    events: mpsc::UnboundedReceiver<ControlEvent>,
    telemetry: Arc<Mutex<TelemetryReport>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ControlClient {
    /// Connects, completes the handshake, and starts the heartbeat.
    ///
    /// Returns once the `welcome` has arrived, so a caller knows it is in
    /// before it starts expecting audio. The heartbeat and telemetry then run
    /// in the background.
    pub async fn join(
        payload: &JoinPayload,
        identity: DeviceIdentity,
    ) -> anyhow::Result<Self> {
        let status = Arc::new(ControlStatus::default());
        *status.state.lock() = ConnectionState::Connecting;

        let url = payload.control_url();
        debug!(%url, "connecting to the desktop");
        let (socket, _response) =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, tokio_tungstenite::connect_async(url))
                .await
                .map_err(|_| anyhow::anyhow!("the desktop did not answer"))??;

        let (mut writer, mut reader) = socket.split();

        let hello = Envelope::new(EnvelopeKind::Hello {
            token: payload.token.clone(),
            client: format!("stagelink-mobile/{}", identity.app_version),
            control_protocol_version: payload.control_protocol_version.clone(),
            receiver: Some(identity.to_wire()),
        });
        writer
            .send(Message::text(hello.encode()?.trim_end().to_owned()))
            .await?;

        // The welcome, or a closed socket. A closed socket *is* the refusal —
        // the desktop does not say why, because there is no correlation id on
        // a hello to answer. See the open item in docs/protocol/pairing.md.
        let welcome = match tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.next()).await {
            Ok(Some(Ok(message))) => {
                let text = message.into_text()?;
                Envelope::decode(&text)?
            }
            _ => {
                let reason = "The desktop refused the join code. Ask for a fresh QR code."
                    .to_owned();
                *status.state.lock() = ConnectionState::Failed(reason.clone());
                anyhow::bail!(reason);
            }
        };

        let EnvelopeKind::Welcome {
            service_version,
            control_protocol_version,
            audio_protocol_version,
            stream,
            key_fingerprint,
            ..
        } = welcome.kind
        else {
            anyhow::bail!("the desktop did not send a welcome");
        };

        let welcome = Welcome {
            service_version,
            control_protocol_version,
            audio_protocol_version,
            stream,
            key_fingerprint,
        };
        *status.welcome.lock() = Some(welcome.clone());
        *status.state.lock() = ConnectionState::Connected;
        info!(
            service = %welcome.service_version,
            audio_protocol = welcome.audio_protocol_version,
            "joined"
        );

        let (event_tx, events) = mpsc::unbounded_channel();
        let _ = event_tx.send(ControlEvent::Connected(welcome));

        let telemetry = Arc::new(Mutex::new(TelemetryReport::default()));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(run(
            writer,
            reader,
            Arc::clone(&status),
            Arc::clone(&telemetry),
            event_tx,
            shutdown_rx,
        ));

        Ok(Self {
            status,
            events,
            telemetry,
            shutdown: Some(shutdown_tx),
        })
    }

    #[must_use]
    pub fn status(&self) -> Arc<ControlStatus> {
        Arc::clone(&self.status)
    }

    /// Updates the figures sent in the next telemetry report.
    ///
    /// Called by the audio layer. The client sends whatever is here on its own
    /// schedule, so the audio path never waits on a socket.
    pub fn set_telemetry(&self, report: TelemetryReport) {
        *self.telemetry.lock() = report;
    }

    pub fn try_next_event(&mut self) -> Option<ControlEvent> {
        self.events.try_recv().ok()
    }

    pub async fn next_event(&mut self) -> Option<ControlEvent> {
        self.events.recv().await
    }

    /// Leaves cleanly, so the desktop removes the roster entry at once rather
    /// than waiting five heartbeats to notice.
    pub fn leave(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for ControlClient {
    fn drop(&mut self) {
        self.leave();
    }
}

/// The background loop: heartbeat out, telemetry out, events in.
async fn run<W, R>(
    mut writer: W,
    mut reader: R,
    status: Arc<ControlStatus>,
    telemetry: Arc<Mutex<TelemetryReport>>,
    events: mpsc::UnboundedSender<ControlEvent>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) where
    W: SinkExt<Message> + Unpin,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    let mut telemetry_tick = tokio::time::interval(TELEMETRY_INTERVAL);
    // Skip the immediate first tick for telemetry: there is nothing to report
    // a microsecond after joining, and an empty report would look like a
    // receiver with no audio.
    telemetry_tick.reset();

    let mut sequence = 0_u64;
    let mut outstanding: Option<(String, i64)> = None;
    let mut missed = 0_u64;

    let reason = loop {
        tokio::select! {
            _ = &mut shutdown => break "left".to_owned(),

            _ = heartbeat.tick() => {
                // A heartbeat still outstanding when the next is due is a
                // missed one. Five of those and the desktop stops sending
                // audio, so this is counted and surfaced rather than ignored.
                if outstanding.take().is_some() {
                    missed += 1;
                    *status.missed.lock() = missed;
                    warn!(missed, "a heartbeat went unanswered");
                }
                sequence += 1;
                let id = format!("h{sequence}");
                let sent_at = now_ms();
                let request = Envelope::new(EnvelopeKind::Request {
                    id: id.clone(),
                    method: "receiver.heartbeat".to_owned(),
                    params: json!({ "sentAt": sent_at }),
                });
                if send(&mut writer, &request).await.is_err() {
                    break "the connection to the desktop was lost".to_owned();
                }
                outstanding = Some((id, sent_at));
            }

            _ = telemetry_tick.tick() => {
                sequence += 1;
                let report = *telemetry.lock();
                let request = Envelope::new(EnvelopeKind::Request {
                    id: format!("t{sequence}"),
                    method: "receiver.telemetry".to_owned(),
                    params: report.to_params(now_ms()),
                });
                if send(&mut writer, &request).await.is_err() {
                    break "the connection to the desktop was lost".to_owned();
                }
            }

            incoming = reader.next() => match incoming {
                Some(Ok(message)) => {
                    if message.is_close() {
                        break "the desktop closed the connection".to_owned();
                    }
                    let Ok(text) = message.into_text() else { continue };
                    let Ok(envelope) = Envelope::decode(&text) else {
                        // One bad line does not cost a performer their feed.
                        debug!("discarding an unparseable control message");
                        continue;
                    };
                    handle(envelope, &status, &events, &mut outstanding);
                }
                _ => break "the connection to the desktop was lost".to_owned(),
            },
        }
    };

    *status.state.lock() = ConnectionState::Failed(reason.clone());
    let _ = events.send(ControlEvent::Disconnected(reason));
    debug!("control channel closed");
}

fn handle(
    envelope: Envelope,
    status: &Arc<ControlStatus>,
    events: &mpsc::UnboundedSender<ControlEvent>,
    outstanding: &mut Option<(String, i64)>,
) {
    match envelope.kind {
        EnvelopeKind::Response { id, ok, result, .. } => {
            if !ok {
                debug!(%id, "the desktop refused a request");
                return;
            }
            if let Some((expected, sent_at)) = outstanding.as_ref() {
                if *expected == id {
                    // Round-trip time from the echo, with no bookkeeping: the
                    // desktop sends `sentAt` back for exactly this.
                    let echoed = result
                        .as_ref()
                        .and_then(|value| value.get("sentAt"))
                        .and_then(Value::as_i64)
                        .unwrap_or(*sent_at);
                    *status.rtt_ms.lock() = Some((now_ms() - echoed) as f64);
                    *status.heartbeats.lock() += 1;
                    *outstanding = None;
                }
            }
        }
        EnvelopeKind::Event { event, payload, .. } => {
            if event == "stream.params" {
                match serde_json::from_value::<StreamParams>(payload) {
                    Ok(params) => {
                        info!(
                            session = params.session_id,
                            sample_rate = params.sample_rate,
                            "audio parameters received"
                        );
                        let _ = events.send(ControlEvent::StreamParams(params));
                    }
                    Err(error) => {
                        // Refused rather than guessed at: the sample rate
                        // reaches a receiver nowhere else, and playing at the
                        // wrong one puts the mix sharp in somebody's ears.
                        warn!(%error, "the audio parameters did not parse");
                    }
                }
            }
        }
        _ => {}
    }
}

async fn send<W>(writer: &mut W, envelope: &Envelope) -> Result<(), ()>
where
    W: SinkExt<Message> + Unpin,
{
    let line = envelope.encode().map_err(|_| ())?;
    writer
        .send(Message::text(line.trim_end().to_owned()))
        .await
        .map_err(|_| ())
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_telemetry_report_matches_the_protocol_field_names() {
        let report = TelemetryReport {
            packets_received: 1_990,
            packets_lost: 10,
            loss_percent: 0.5,
            jitter_ms: 2.5,
            buffer_health_ms: 40.0,
            underruns: 1,
            playback_state: "playing",
            output_latency_ms: Some(38.0),
            resample_ratio_ppm: Some(-12.0),
        };
        let params = report.to_params(1_700_000_000_000);

        // These names are the protocol. A typo here is a field the desktop
        // silently reads as zero.
        for key in [
            "reportedAt",
            "packetsReceived",
            "packetsLost",
            "lossPercent",
            "jitterMs",
            "bufferHealthMs",
            "underruns",
            "playbackState",
            "outputLatencyMs",
            "resampleRatioPpm",
        ] {
            assert!(params.get(key).is_some(), "{key} is missing");
        }
        assert_eq!(params["packetsLost"], 10);
        assert_eq!(params["playbackState"], "playing");

        // Never sent: the desktop stamps both, so a receiver cannot forge its
        // own provenance.
        assert!(params.get("source").is_none());
        assert!(params.get("receivedAt").is_none());
    }

    #[test]
    fn an_unmeasurable_latency_is_null_rather_than_zero() {
        let report = TelemetryReport {
            output_latency_ms: None,
            resample_ratio_ppm: None,
            ..TelemetryReport::default()
        };
        let params = report.to_params(0);
        assert!(params["outputLatencyMs"].is_null());
        assert!(
            params["resampleRatioPpm"].is_null(),
            "a receiver that cannot measure something must say so, not report zero"
        );
    }

    #[test]
    fn the_hello_carries_the_token_and_the_device_description() {
        let identity = DeviceIdentity {
            device_name: "Pixel 8".to_owned(),
            platform: "android".to_owned(),
            platform_version: Some("14".to_owned()),
            app_version: "0.1.0".to_owned(),
            public_key: "ab".repeat(32),
            audio_port: Some(45_872),
        };
        let wire = identity.to_wire();
        assert_eq!(wire.device_name, "Pixel 8");
        assert_eq!(wire.app_version.as_deref(), Some("0.1.0"));

        let hello = Envelope::new(EnvelopeKind::Hello {
            token: "9f2c".to_owned(),
            client: "stagelink-mobile/0.1.0".to_owned(),
            control_protocol_version: "1.0.0".to_owned(),
            receiver: Some(wire),
        });
        let line = hello.encode().unwrap();
        assert!(line.contains("\"kind\":\"hello\""));
        assert!(line.contains("\"receiver\""));
        assert!(line.contains("\"deviceName\":\"Pixel 8\""));
    }

    #[test]
    fn the_heartbeat_interval_leaves_room_before_the_desktop_gives_up() {
        // Five missed heartbeats stops the audio. Sending every second means
        // five seconds of total silence from this client before that happens,
        // which is a real network outage rather than a scheduling hiccup.
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(1));
        assert!(TELEMETRY_INTERVAL > HEARTBEAT_INTERVAL);
    }

    #[test]
    fn a_fresh_status_is_disconnected_and_claims_nothing() {
        let status = ControlStatus::default();
        assert_eq!(status.state(), ConnectionState::Disconnected);
        assert!(status.welcome().is_none());
        assert!(status.rtt_ms().is_none(), "no round trip has been measured");
        assert_eq!(status.heartbeats(), 0);
        assert!(!status.is_connected());
    }
}

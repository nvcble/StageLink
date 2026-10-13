//! Starting and stopping the audio stream.
//!
//! The glue between the capture engine, the UDP sender and the session state
//! machine. Kept out of the control handler because the handler is a
//! dispatcher and this is a lifecycle.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use audio_capture::CaptureStats;
use audio_core::RingStats;
use audio_protocol::TransportKind;
use desktop_ipc::ServiceErrorPayload;
use serde_json::{Value, json};
use stream_server::sender::{FrameSource, StreamSender};
use stream_server::{StreamConfig, StreamConfigError};
use tracing::{info, warn};

use crate::audio_engine::AudioEngine;
use crate::settings::AppSettings;

/// Reads captured frames for the sender thread.
///
/// Also watches the capture xrun counter, which is what makes a desktop
/// glitch visible to a receiver as `DISCONTINUITY` rather than as silently
/// late packets.
struct CaptureSource {
    engine: Arc<AudioEngine>,
    stats: Option<Arc<CaptureStats>>,
    /// Ring counters, for the trims that bound capture latency.
    ring: Option<Arc<RingStats>>,
    /// Last xrun total seen, so only the increase is reported.
    seen_xruns: AtomicU64,
    /// Last trim total seen, likewise.
    seen_trims: AtomicU64,
}

impl FrameSource for CaptureSource {
    fn pop(&mut self, out: &mut [f32]) -> bool {
        self.engine.pop_frames(out)
    }

    /// Driver xruns and ring trims, because both break continuity.
    ///
    /// A trim belongs here as much as an xrun does. It discards captured audio
    /// to claw latency back, so the samples either side of it were not
    /// adjacent. Told about it, a receiver conceals the seam and
    /// resynchronises; not told, it plays the splice as a click and reads the
    /// jump as clock drift its controller then tries to correct.
    fn take_glitches(&mut self) -> u64 {
        let xruns = match &self.stats {
            Some(stats) => {
                let total = stats.xruns();
                total.saturating_sub(self.seen_xruns.swap(total, Ordering::Relaxed))
            }
            None => 0,
        };
        let trims = match &self.ring {
            Some(ring) => {
                let total = ring.trim_events();
                total.saturating_sub(self.seen_trims.swap(total, Ordering::Relaxed))
            }
            None => 0,
        };
        xruns + trims
    }
}

/// A fresh session identifier.
///
/// The low 32 bits of a UUID, because the packet header carries only 32 bits.
/// It must **differ from the previous session's**: a receiver treats an
/// unchanged id as a continuation and would keep playing audio buffered before
/// the restart. It need not be unpredictable — `docs/protocol/audio-packets.md`
/// is explicit that possessing a session id grants nothing — but it is the
/// salt for the per-session audio key, so a repeat across sessions must not be
/// *likely* either.
#[must_use]
pub fn new_session_id() -> u32 {
    let bytes = uuid::Uuid::new_v4().as_u128().to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Builds the stream configuration from settings and the chosen adapter.
pub fn stream_config(
    settings: &AppSettings,
    bind_address: &str,
    sample_rate: u32,
) -> StreamConfig {
    StreamConfig {
        sample_rate,
        frame_duration_ms: settings.streaming.frame_duration_ms,
        transport: match settings.streaming.transport.as_str() {
            "udp-plaintext" => TransportKind::UdpPlaintext,
            _ => TransportKind::UdpEncrypted,
        },
        bind_address: bind_address.to_owned(),
        stream_port: settings.network.stream_port,
        control_port: settings.network.control_port,
        max_receivers: settings.streaming.max_receivers,
        per_receiver_queue_packets: stream_server::config::DEFAULT_QUEUE_PACKETS,
    }
}

/// Starts the sender.
///
/// Refuses rather than starting silently broken in three cases, each of which
/// would otherwise show as a dashboard saying "streaming" while nobody hears
/// anything: no capture running, no bound address, or a transport this build
/// will not carry.
pub fn start(
    engine: &Arc<AudioEngine>,
    config: StreamConfig,
    session_id: u32,
    release_build: bool,
    token: Option<&str>,
) -> Result<StreamSender, ServiceErrorPayload> {
    if !engine.is_running() {
        return Err(ServiceErrorPayload::new(
            "STREAM_NO_INPUT",
            "No audio is being captured, so there is nothing to stream.",
        )
        .with_detail("Select an input device on the Audio Input page first."));
    }

    config.validate(release_build).map_err(config_error)?;

    let stats = engine.capture_stats();
    let source = CaptureSource {
        engine: Arc::clone(engine),
        stats,
        ring: engine.ring_stats(),
        seen_xruns: AtomicU64::new(0),
        seen_trims: AtomicU64::new(0),
    };
    // Seed the xrun baseline so glitches from *before* this stream started are
    // not reported as a discontinuity in its first packet.
    let mut source = source;
    let _ = source.take_glitches();

    let transport = config.transport;

    // The key is derived from the join token and the session id. A receiver
    // that scanned the code derives the same one independently; nothing is
    // transmitted. See `audio_protocol::crypto` for what that does and does
    // not buy.
    let key = if transport.is_encrypted() {
        let Some(token) = token else {
            return Err(ServiceErrorPayload::new(
                "STREAM_TRANSPORT_REFUSED",
                "Create a join code before starting an encrypted stream.",
            )
            .with_detail(
                "The audio key is derived from the join code, so there has to be one. Press \n                 Create code on the Receivers page.",
            ));
        };
        Some(audio_protocol::SessionKey::derive(token, session_id))
    } else {
        None
    };

    // Start at live. The ring has been filling since the device was opened —
    // minutes ago, if the operator was setting levels — and the sender consumes
    // at exactly real time, so whatever is in there now is delay every
    // performer would carry for the whole session. Done as late as possible:
    // everything that can refuse to start has already done so above.
    engine.reset_ring();

    let sender = StreamSender::start(config, session_id, Box::new(source), key).map_err(|error| {
        ServiceErrorPayload::new(
            "STREAM_BIND_FAILED",
            format!("Could not open the audio socket: {error}"),
        )
        .with_detail(
            "Another application may be using the port, or the selected network adapter may \
             have changed. Check the Network page.",
        )
    })?;

    info!(
        session_id,
        transport = transport.as_str(),
        local = %sender.local_address(),
        "streaming started"
    );
    Ok(sender)
}

/// Stops the sender, draining so receivers stop cleanly.
pub fn stop(sender: &mut StreamSender, immediate: bool) {
    sender.stop(!immediate);
    if immediate {
        warn!("stream stopped without draining; receivers will time out rather than stop cleanly");
    }
}

fn config_error(error: StreamConfigError) -> ServiceErrorPayload {
    let (code, detail) = match &error {
        StreamConfigError::InsecureTransport(_) => (
            "STREAM_TRANSPORT_REFUSED",
            Some(
                "A release build will not send a live monitor mix in the clear over a venue \
                 network. Encrypted transport is required.",
            ),
        ),
        // Both mean the same thing to an operator: the packet length is not
        // one that can be sent, and these are the ones that can.
        StreamConfigError::WouldFragment { .. }
        | StreamConfigError::UnsupportedFrameDuration(_) => (
            "SETTINGS_INVALID",
            Some("Choose a supported packet duration: 2.5, 5 or 10 ms."),
        ),
        _ => ("SETTINGS_INVALID", None),
    };
    let mut payload = ServiceErrorPayload::new(code, error.to_string());
    if let Some(detail) = detail {
        payload = payload.with_detail(detail);
    }
    payload
}

/// What a receiver needs to know about the stream it is about to be sent.
///
/// Two things reach a receiver nowhere else. The **sample rate**, because the
/// packet header does not carry it and guessing wrong plays the mix sharp or
/// flat in somebody's ears. And the **session id**, because the audio key is
/// derived from it — a receiver without this cannot open a single packet.
#[must_use]
pub fn params(session_id: u32, config: &stream_server::StreamConfig) -> Value {
    serde_json::json!({
        "sessionId": session_id,
        "sampleRate": config.sample_rate,
        "frameDurationMs": config.frame_duration_ms,
        "payloadFormat": "pcm_s16le",
        "transport": config.transport.as_str(),
        "streamPort": config.stream_port,
        "framesPerPacket": config.frames_per_packet(),
    })
}

/// Live stream status, as `streamStatusSchema`.
///
/// Every counter here is **desktop-measured**: packets this machine handed to
/// the socket, bytes it wrote, drops it made because a receiver's queue was
/// full. None of it describes what any receiver actually heard — that can only
/// come from the receiver, and arrives separately labelled as its own claim.
#[must_use]
pub fn status(sender: &StreamSender, started_at: i64, session_id: u32) -> Value {
    let stats = sender.stats();
    let destinations = sender.destination_stats();
    let config = sender.config();

    json!({
        "state": if sender.is_running() { "running" } else { "stopped" },
        "config": {
            "sampleRate": config.sample_rate,
            "frameDurationMs": config.frame_duration_ms,
            "payloadFormat": "pcm_s16le",
            "transport": config.transport.as_str(),
            "bindAddress": config.bind_address,
            "streamPort": config.stream_port,
            "controlPort": config.control_port,
            "maxReceivers": config.max_receivers,
            "perReceiverQueuePackets": config.per_receiver_queue_packets,
        },
        "sessionId": session_id,
        "startedAt": started_at,
        "packetsSent": stats.packets_sent(),
        "bytesSent": stats.bytes_sent(),
        "sendFailures": stats.send_errors(),
        "packetsDropped": stats.dropped_queue_full(),
        "queueDepth": destinations
            .iter()
            .map(|destination| destination.queue_depth)
            .max()
            .unwrap_or(0),
        "activeReceivers": destinations.len(),
        "lastError": Value::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AppSettings {
        AppSettings::default()
    }

    #[test]
    fn the_configuration_comes_from_the_settings_and_the_chosen_adapter() {
        let config = stream_config(&settings(), "10.0.0.1", 48_000);
        assert_eq!(config.bind_address, "10.0.0.1");
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.frame_duration_ms, 5.0);
        assert_eq!(config.stream_port, 45_872);
        // Encrypted is the default, and a release build depends on that.
        assert_eq!(config.transport, TransportKind::UdpEncrypted);
    }

    #[test]
    fn the_capture_rate_is_used_rather_than_the_configured_one() {
        // The device decides: WASAPI may negotiate 44.1 kHz whatever the
        // setting says, and sending packets labelled 48 kHz would make every
        // receiver play the mix sharp.
        let config = stream_config(&settings(), "10.0.0.1", 44_100);
        assert_eq!(config.sample_rate, 44_100);
    }

    #[test]
    fn starting_without_capture_is_refused_with_the_action_to_take() {
        let engine = Arc::new(AudioEngine::new());
        let config = stream_config(&settings(), "127.0.0.1", 48_000);
        let error = match start(&engine, config, 1, false, None) {
            Err(error) => error,
            Ok(_) => panic!("no device is open, so this must not start"),
        };
        assert_eq!(error.code, "STREAM_NO_INPUT");
        assert!(error.detail.unwrap().contains("Audio Input"));
    }

    #[test]
    fn a_release_build_refuses_the_plaintext_transport_with_the_reason() {
        let mut settings = settings();
        settings.streaming.transport = "udp-plaintext".to_owned();
        let config = stream_config(&settings, "127.0.0.1", 48_000);

        let error = config_error(config.validate(true).expect_err("release must refuse"));
        assert_eq!(error.code, "STREAM_TRANSPORT_REFUSED");
        assert!(error.detail.unwrap().contains("in the clear"));

        // And a development build allows it, so the headless receiver can be
        // used without a key exchange.
        assert!(config.validate(false).is_ok());
    }

    #[test]
    fn an_unsupported_packet_duration_names_the_supported_ones() {
        let mut settings = settings();
        settings.streaming.frame_duration_ms = 20.0;
        let config = stream_config(&settings, "127.0.0.1", 48_000);
        let error = config_error(config.validate(false).expect_err("20 ms fragments"));
        assert!(error.detail.unwrap().contains("2.5, 5 or 10"));
    }

    #[test]
    fn the_status_reports_desktop_measured_counters_and_names_them_so() {
        let config = StreamConfig {
            transport: TransportKind::UdpPlaintext,
            bind_address: "127.0.0.1".to_owned(),
            ..StreamConfig::default()
        };
        struct Silent;
        impl FrameSource for Silent {
            fn pop(&mut self, out: &mut [f32]) -> bool {
                out.fill(0.0);
                true
            }
        }
        let mut sender = StreamSender::start(config, 7, Box::new(Silent), None).unwrap();
        let snapshot = status(&sender, 1_700_000_000_000, 7);

        assert_eq!(snapshot["state"], "running");
        assert_eq!(snapshot["sessionId"], 7);
        assert_eq!(snapshot["activeReceivers"], 0);
        assert_eq!(snapshot["config"]["transport"], "udp-plaintext");
        assert_eq!(snapshot["config"]["payloadFormat"], "pcm_s16le");

        sender.stop(false);
        assert_eq!(status(&sender, 1, 7)["state"], "stopped");
    }
}

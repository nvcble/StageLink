//! JSON projections of service state.
//!
//! One place where Rust types become the exact shapes the renderer's zod
//! schemas expect. Keeping the conversions here rather than deriving `Serialize`
//! on domain types means the wire format can be stable while the internals move.
//!
//! Two conventions that are load-bearing:
//!
//! * A figure that is not known is `null`, never `0`. "No receiver has reported
//!   jitter" and "jitter is zero" mean entirely different things to an operator.
//! * Every diagnostics figure carries a `source` of `measured`, `estimated` or
//!   `receiver-reported`.

use audio_capture::DeviceInfo;
use desktop_ipc::codec::ServiceErrorPayload;
use device_discovery::{NetworkAdapter, evaluate_warnings};
use persistence::SessionRecord;
use receiver_session::{JoinCode, RosterEntry, Session};
use serde_json::{Value, json};
use telemetry::{LatencyBreakdown, Measurement};

use crate::audio_engine::AudioStatus;
use crate::handler::{CONTROL_PROTOCOL_VERSION, SERVICE_VERSION};
use crate::settings::AppSettings;
use crate::state::ServiceState;

/// dBFS for a linear amplitude, or `None` for digital silence.
///
/// Silence must not be rendered as a large negative number: the UI draws `null`
/// as "—" and anything else as a level.
#[must_use]
pub fn finite_db(amplitude: f32) -> Option<f32> {
    let db = audio_core::linear_to_dbfs(amplitude);
    if db.is_finite() { Some(db) } else { None }
}

/// Maps a capture error onto the control protocol's structured errors.
#[must_use]
pub fn capture_error(error: audio_capture::CaptureError) -> ServiceErrorPayload {
    ServiceErrorPayload::new(error.code(), error.to_string()).retryable(error.is_retryable())
}

/// Maps a persistence error onto the control protocol's structured errors.
#[must_use]
pub fn persistence_error(error: persistence::PersistenceError) -> ServiceErrorPayload {
    ServiceErrorPayload::new(error.code(), error.to_string())
}

/// An audio device, as `audioDeviceSchema`.
#[must_use]
pub fn device(device: &DeviceInfo) -> Value {
    json!({
        "id": device.id,
        "name": device.name,
        "api": device.api.as_str(),
        "isDefault": device.is_default,
        "state": device.state.as_str(),
        "inputChannels": device.input_channels,
        // An endpoint that cannot be activated has no mix format to report, so
        // these are null rather than a meaningless zero. The operator still
        // sees the device in the list, with its state explaining why.
        "defaultSampleRate": if device.state.is_usable() && device.default_sample_rate > 0 {
            Value::from(device.default_sample_rate)
        } else {
            Value::Null
        },
        "defaultSampleFormat": if device.state.is_usable() && device.default_sample_rate > 0 {
            Value::from(device.default_sample_format.as_str())
        } else {
            Value::Null
        },
        "supportedSampleRates": device.supported_sample_rates,
    })
}

/// Capture status, as `audioStatusSchema`.
#[must_use]
pub fn audio_status(status: &AudioStatus) -> Value {
    json!({
        "state": status.state.as_str(),
        "device": status.device.as_ref().map(device),
        "channelIndex": status.channel_index,
        "format": status.negotiated.map(|negotiated| json!({
            "sampleRate": negotiated.format.sample_rate,
            "deviceChannels": negotiated.format.channels,
            "deviceSampleFormat": negotiated.format.sample_format.as_str(),
            "periodFrames": negotiated.period_frames,
            "shareMode": negotiated.share_mode.as_str(),
            // Surfaced because engine resampling costs a little latency and a
            // little quality, and the operator should know it is happening.
            "engineConverted": negotiated.engine_converted,
        })),
        "framesCaptured": status.frames_captured,
        "xruns": status.xruns,
        "deviceConnected": status.device_connected,
        "startedAt": status.started_at,
        "lastError": status.last_error,
    })
}

/// A meter frame, as `audioLevelsSchema`.
#[must_use]
pub fn audio_levels(snapshot: &audio_core::LevelSnapshot, at: i64) -> Value {
    json!({
        "at": at,
        "rmsDb": snapshot.rms_db,
        "peakDb": snapshot.peak_db,
        "peakHoldDb": snapshot.peak_hold_db,
        "clipping": snapshot.clipping,
        "clipCount": snapshot.clip_count,
        "windowFrames": snapshot.window_frames,
    })
}

/// Network status, as `networkStatusSchema`.
#[must_use]
pub fn network_status(
    adapters: &[NetworkAdapter],
    selected_adapter_id: Option<&str>,
    settings: &AppSettings,
) -> Value {
    let selected_address = selected_adapter_id
        .and_then(|id| adapters.iter().find(|adapter| adapter.id == id))
        .and_then(|adapter| {
            settings
                .network
                .bind_address
                .clone()
                .or_else(|| adapter.primary_ipv4().map(|info| info.address.clone()))
        });

    json!({
        "adapters": adapters.iter().map(adapter_json).collect::<Vec<_>>(),
        "selectedAdapterId": selected_adapter_id,
        "selectedAddress": selected_address,
        "controlPort": settings.network.control_port,
        "streamPort": settings.network.stream_port,
        // Discovery publishing lands in Phase 3; reported honestly as inactive
        // rather than as enabled-because-configured.
        "discoveryActive": false,
        "discoveryServiceName": settings.network.discovery_instance_name,
        "egressBitsPerSecond": 0,
        "sendErrors": 0,
        "warnings": evaluate_warnings(adapters, selected_adapter_id)
            .iter()
            .map(|warning| json!({
                "code": warning.code,
                "severity": warning.severity,
                "message": warning.message,
                "remediation": warning.remediation,
            }))
            .collect::<Vec<_>>(),
    })
}

fn adapter_json(adapter: &NetworkAdapter) -> Value {
    json!({
        "id": adapter.id,
        "name": adapter.name,
        "description": adapter.description,
        "kind": adapter.kind,
        "macAddress": adapter.mac_address,
        "isUp": adapter.is_up,
        "speedMbps": adapter.speed_mbps,
        "hasDefaultRoute": adapter.has_default_route,
        "addresses": adapter.addresses.iter().map(|info| json!({
            "family": info.family,
            "address": info.address,
            "netmask": info.netmask,
            "prefixLength": info.prefix_length,
            "broadcast": info.broadcast,
        })).collect::<Vec<_>>(),
    })
}

/// A live session, as `sessionSchema`.
#[must_use]
pub fn session(session: &Session, now_ms: i64) -> Value {
    json!({
        "id": session.id,
        "label": session.label,
        "state": session.state.as_str(),
        "createdAt": session.created_at,
        "startedAt": session.started_at,
        "endedAt": session.ended_at,
        "durationMs": session.duration_at(now_ms),
        "peakReceivers": session.peak_receivers,
        "totalReceivers": session.total_receivers,
        "lastError": session.last_error,
    })
}

/// A persisted session row, as `sessionSchema`.
#[must_use]
pub fn session_record(record: &SessionRecord) -> Value {
    json!({
        "id": record.id,
        "label": record.label,
        "state": record.state,
        "createdAt": record.created_at,
        "startedAt": record.started_at,
        "endedAt": record.ended_at,
        "durationMs": record.duration_ms,
        "peakReceivers": record.peak_receivers,
        "totalReceivers": record.total_receivers,
        "lastError": record.last_error,
    })
}

/// A roster entry, as `receiverSchema`.
///
/// Note the shape of `telemetry`: it carries the literal
/// `source: "receiver-reported"` so nothing downstream can mistake a phone's
/// claim for a desktop measurement.
#[must_use]
pub fn roster_entry(entry: &RosterEntry) -> Value {
    json!({
        "id": entry.receiver_id,
        "deviceName": entry.receiver_id,
        "displayName": Value::Null,
        "platform": "unknown",
        "platformVersion": Value::Null,
        "appVersion": Value::Null,
        "ipAddress": entry.ip_address,
        "connectionState": entry.connection_state,
        // A receiver in the roster got there by presenting a valid join code,
        // which is what authorisation means in this model.
        "pairingState": "approved",
        "firstSeenAt": entry.joined_at,
        "lastSeenAt": entry.last_heartbeat_at.unwrap_or(entry.joined_at),
        "lastHeartbeatAt": entry.last_heartbeat_at,
        "packetsSentToReceiver": entry.packets_sent,
        "packetsDroppedLocally": entry.packets_dropped_locally,
        "telemetry": entry.telemetry.map(|telemetry| json!({
            "source": "receiver-reported",
            "receivedAt": telemetry.received_at,
            "reportedAt": telemetry.reported_at,
            "packetsReceived": telemetry.packets_received,
            "packetsLost": telemetry.packets_lost,
            "lossPercent": telemetry.loss_percent,
            "jitterMs": telemetry.jitter_ms,
            "bufferHealthMs": telemetry.buffer_health_ms,
            "underruns": telemetry.underruns,
            "playbackState": "unknown",
            "outputLatencyMs": Value::Null,
        })),
    })
}

/// A join code, as `joinCodeSchema`.
#[must_use]
pub fn join_code(code: &JoinCode, settings: &AppSettings) -> Value {
    let host = settings
        .network
        .bind_address
        .clone()
        .unwrap_or_default();
    let control_port = settings.network.control_port;
    let stream_port = settings.network.stream_port;

    json!({
        "token": code.token,
        "qrPayload": code.qr_payload(&host, control_port, stream_port, CONTROL_PROTOCOL_VERSION),
        "host": host,
        "controlPort": control_port,
        "streamPort": stream_port,
        "issuedAt": code.issued_at,
        "expiresAt": code.expires_at,
    })
}

/// Stream status for a service with no stream running.
///
/// Phase 3 replaces this with the real counters. Until then it reports
/// `stopped` and zeroes, which is the truth.
#[must_use]
pub fn stream_status_stopped() -> Value {
    json!({
        "state": "stopped",
        "config": Value::Null,
        "sessionId": Value::Null,
        "startedAt": Value::Null,
        "packetsSent": 0,
        "bytesSent": 0,
        "sendFailures": 0,
        "packetsDropped": 0,
        "queueDepth": 0,
        "activeReceivers": 0,
        "lastError": Value::Null,
    })
}

/// Supervision info, as `serviceInfoSchema`.
#[must_use]
pub fn service_info(state: &ServiceState) -> Value {
    let audio = state.audio.status();
    json!({
        // From the service's own point of view it is always ready; the
        // supervising Electron process owns the starting/restarting states.
        "state": if audio.state == crate::audio_engine::CaptureState::Error {
            "degraded"
        } else {
            "ready"
        },
        "version": SERVICE_VERSION,
        "controlProtocolVersion": CONTROL_PROTOCOL_VERSION,
        "audioProtocolVersion": audio_protocol::AUDIO_PROTOCOL_VERSION,
        "pid": std::process::id(),
        "startedAt": crate::state::now_ms() - state.uptime_ms(),
        "restartCount": 0,
        "lastError": Value::Null,
        "availableAudioApis": audio_capture::AudioApi::available()
            .iter()
            .map(|api| api.as_str())
            .collect::<Vec<_>>(),
    })
}

fn measurement(measurement: &Measurement) -> Value {
    json!({
        "value": measurement.value,
        "unit": measurement.unit,
        "source": measurement.source.as_str(),
        "note": measurement.note,
    })
}

fn latency(breakdown: &LatencyBreakdown) -> Value {
    json!({
        "captureBufferMs": measurement(&breakdown.capture_buffer_ms),
        "packetFrameMs": measurement(&breakdown.packet_frame_ms),
        "sendQueueMs": measurement(&breakdown.send_queue_ms),
        "desktopSubtotalMs": measurement(&breakdown.desktop_subtotal_ms),
        "controlChannelRttMs": measurement(&breakdown.control_channel_rtt_ms),
        "receiverReportedOutputMs": measurement(&breakdown.receiver_reported_output_ms),
        "disclaimer": breakdown.disclaimer,
    })
}

/// A diagnostics snapshot, as `diagnosticsSnapshotSchema`.
#[must_use]
pub fn diagnostics_snapshot(state: &ServiceState) -> Value {
    let audio = state.audio.status();
    let settings = state.settings_snapshot();
    let system = state.system.sample();
    let ring = state.audio.ring_stats();
    let capture = state.audio.capture_stats();

    // Capture period is only knowable once a device is open; before that it is
    // reported as unknown rather than as the configured value, which the driver
    // has not yet agreed to.
    let capture_buffer_ms = audio.negotiated.and_then(|negotiated| {
        telemetry::latency::frames_to_millis(negotiated.period_frames, negotiated.format.sample_rate)
    });
    let packet_frame_ms = Some(settings.streaming.frame_duration_ms);
    // Audio waiting in the capture ring is a real, measured desktop-side
    // contribution. Nothing is queued for *transmission* until Phase 3, so the
    // ring depth is the whole of this term today.
    let send_queue_ms = state.audio.ring_millis();

    let latency_breakdown = LatencyBreakdown::new(
        capture_buffer_ms,
        // Only claim a subtotal once there is a capture period to add it to.
        capture_buffer_ms.and(packet_frame_ms),
        capture_buffer_ms.and(send_queue_ms),
        None,
        None,
    );

    let roster = state.roster.lock();

    json!({
        "at": crate::state::now_ms(),
        "serviceUptimeMs": state.uptime_ms(),
        "serviceVersion": SERVICE_VERSION,
        "audio": {
            "captureState": audio.state.as_str(),
            "framesCaptured": audio.frames_captured,
            "bufferOverruns": ring.as_ref().map_or(0, |stats| stats.overrun_events()),
            "bufferUnderruns": ring.as_ref().map_or(0, |stats| stats.underrun_events()),
            "xruns": audio.xruns,
            "callbackMaxUs": capture.as_ref().map_or(0, |stats| stats.callback_max_us()),
            "callbackAvgUs": capture.as_ref().map_or(0, |stats| stats.callback_avg_us()),
            "clipCount": state.audio.levels().total_clips(),
        },
        "stream": {
            "state": "stopped",
            "packetsSent": 0,
            "bytesSent": 0,
            "sendFailures": 0,
            "packetsDropped": 0,
            "queueDepthMax": 0,
            "queueDepthNow": 0,
            "egressBitsPerSecond": 0,
        },
        "receivers": {
            "connectedCount": roster.active_count(),
            "streamingCount": 0,
            // Null, not zero: no receiver has reported anything.
            "worstLossPercent": Value::Null,
            "meanLossPercent": Value::Null,
            "worstJitterMs": Value::Null,
            "meanJitterMs": Value::Null,
            "totalUnderruns": 0,
            "minBufferHealthMs": Value::Null,
        },
        "system": {
            "processCpuPercent": system.process_cpu_percent,
            "systemCpuPercent": system.system_cpu_percent,
            "processMemoryBytes": system.process_memory_bytes,
            "systemMemoryTotalBytes": system.system_memory_total_bytes,
            "systemMemoryAvailableBytes": system.system_memory_available_bytes,
            "realtimePriorityGranted": audio.realtime_granted,
        },
        "latency": latency(&latency_breakdown),
        "recentErrors": Vec::<Value>::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_core::{LevelSnapshot, PeakHold};

    #[test]
    fn digital_silence_serialises_as_null_not_as_a_floor_value() {
        assert_eq!(finite_db(0.0), None);
        assert!(finite_db(0.5).is_some());

        let mut hold = PeakHold::new(20.0);
        let snapshot = LevelSnapshot::stalled(&mut hold, 0.04, 0);
        let json = audio_levels(&snapshot, 1_000);
        assert_eq!(json["rmsDb"], Value::Null);
        assert_eq!(json["peakDb"], Value::Null);
        assert_eq!(json["windowFrames"], 0);
    }

    #[test]
    fn a_real_meter_frame_carries_decibel_values() {
        let snapshot = LevelSnapshot {
            rms_db: Some(-18.0),
            peak_db: Some(-6.0),
            peak_hold_db: Some(-6.0),
            clipping: false,
            clip_count: 0,
            window_frames: 1_920,
        };
        let json = audio_levels(&snapshot, 42);
        assert_eq!(json["at"], 42);
        assert_eq!(json["rmsDb"], -18.0);
        assert_eq!(json["peakDb"], -6.0);
        assert_eq!(json["clipping"], false);
        assert_eq!(json["windowFrames"], 1_920);
    }

    #[test]
    fn an_idle_stream_status_is_zeroed_and_says_stopped() {
        let status = stream_status_stopped();
        assert_eq!(status["state"], "stopped");
        assert_eq!(status["config"], Value::Null);
        assert_eq!(status["packetsSent"], 0);
    }

    #[test]
    fn capture_errors_keep_their_protocol_code_and_retryability() {
        let payload = capture_error(audio_capture::CaptureError::DeviceDisconnected);
        assert_eq!(payload.code, "AUDIO_DEVICE_DISCONNECTED");
        assert!(payload.retryable, "a reconnect could plausibly succeed");

        let payload = capture_error(audio_capture::CaptureError::ChannelOutOfRange {
            requested: 9,
            available: 2,
        });
        assert_eq!(payload.code, "AUDIO_CHANNEL_OUT_OF_RANGE");
        assert!(!payload.retryable);
    }

    #[test]
    fn the_latency_breakdown_labels_every_term() {
        let breakdown = LatencyBreakdown::new(Some(10.0), Some(5.0), Some(0.0), Some(30.0), None);
        let json = latency(&breakdown);
        assert_eq!(json["captureBufferMs"]["source"], "measured");
        assert_eq!(json["packetFrameMs"]["source"], "estimated");
        assert_eq!(json["desktopSubtotalMs"]["source"], "estimated");
        assert_eq!(json["receiverReportedOutputMs"]["source"], "receiver-reported");
        // RTT is present but explicitly not part of the audio subtotal.
        assert_eq!(json["controlChannelRttMs"]["value"], 30.0);
        assert_eq!(json["desktopSubtotalMs"]["value"], 15.0);
    }

    #[test]
    fn network_status_includes_warnings_and_the_selected_address() {
        let settings = AppSettings::default();
        let json = network_status(&[], None, &settings);
        assert_eq!(json["selectedAdapterId"], Value::Null);
        assert_eq!(json["selectedAddress"], Value::Null);
        assert_eq!(json["controlPort"], 45_871);
        assert_eq!(json["streamPort"], 45_872);
        let warnings = json["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["code"], "NO_ADAPTER_SELECTED");
    }

    #[test]
    fn discovery_is_reported_as_inactive_until_it_is_actually_publishing() {
        let settings = AppSettings::default();
        assert!(settings.network.discovery_enabled, "it is enabled in settings");
        let json = network_status(&[], None, &settings);
        assert_eq!(
            json["discoveryActive"], false,
            "configured is not the same as publishing"
        );
    }
}

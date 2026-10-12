//! Protocol probe: exposes the Rust protocol implementation for cross-checking.
//!
//! Reads one JSON query per line on stdin and writes one JSON answer per line
//! on stdout. `tests/protocol` drives it so the TypeScript and Rust definitions
//! of the control and audio protocols can be compared against each other with
//! the *real* code on both sides, rather than a reimplementation that could
//! drift.
//!
//! This is a development tool. It is not packaged, it opens no sockets, it
//! touches no hardware and it holds no state.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(query) => answer(&query),
            Err(error) => json!({ "ok": false, "reason": error.to_string() }),
        };
        let _ = writeln!(stdout, "{answer}");
        let _ = stdout.flush();
    }
}

fn answer(query: &Value) -> Value {
    match query.get("q").and_then(Value::as_str) {
        Some("controlProtocolVersion") => json!({
            "value": stagelink_service::CONTROL_PROTOCOL_VERSION,
        }),

        Some("packetConstants") => json!({
            "audioProtocolVersion": audio_protocol::AUDIO_PROTOCOL_VERSION,
            "headerBytes": audio_protocol::HEADER_BYTES,
            "magic": String::from_utf8_lossy(&audio_protocol::PACKET_MAGIC),
            "aeadTagBytes": audio_protocol::AEAD_TAG_BYTES,
            "maxPayloadBytes": audio_protocol::MAX_PAYLOAD_BYTES,
        }),

        Some("methodNames") => json!({
            "methods": stagelink_service::DISPATCHED_METHODS,
        }),

        Some("defaultSettings") => {
            serde_json::to_value(stagelink_service::AppSettings::default())
                .unwrap_or(Value::Null)
        }

        Some("encodeEnvelope") => {
            let kind = query.get("envelope").cloned().unwrap_or(Value::Null);
            match build_envelope(&kind) {
                Some(envelope) => match envelope.encode() {
                    Ok(line) => json!({ "ok": true, "line": line.trim_end() }),
                    Err(error) => json!({ "ok": false, "reason": error.to_string() }),
                },
                None => json!({ "ok": false, "reason": "unsupported envelope kind" }),
            }
        }

        Some("decodeEnvelope") => {
            let line = query.get("line").and_then(Value::as_str).unwrap_or("");
            match desktop_ipc::codec::Envelope::decode(line) {
                Ok(envelope) => json!({
                    "ok": true,
                    "kind": envelope_kind_name(&envelope),
                }),
                Err(error) => json!({ "ok": false, "reason": error.to_string() }),
            }
        }

        Some("packetGeometry") => {
            let rate = query.get("sampleRate").and_then(Value::as_u64).unwrap_or(48_000) as u32;
            let duration = query
                .get("frameDurationMs")
                .and_then(Value::as_f64)
                .unwrap_or(5.0);
            let frames = audio_core::frames_per_packet(rate, duration);
            json!({
                "framesPerPacket": frames,
                "datagramBytesPlaintext": audio_protocol::datagram_bytes(frames, false),
                "datagramBytesEncrypted": audio_protocol::datagram_bytes(frames, true),
                "fitsInMtu": audio_protocol::fits_in_mtu(frames, true, 1_500),
            })
        }

        Some("sequenceDelta") => {
            let previous = query.get("previous").and_then(Value::as_u64).unwrap_or(0) as u32;
            let current = query.get("current").and_then(Value::as_u64).unwrap_or(0) as u32;
            json!({ "value": audio_protocol::sequence_delta(previous, current) })
        }

        // Serialises a roster entry so the TypeScript side can validate it
        // against `receiverSchema`. The entry is a fixture built here, not a
        // real or simulated receiver: nothing about it reaches the UI, and the
        // roster the application serves stays empty until a device joins.
        Some("rosterEntryShape") => {
            let mut entry = receiver_session::RosterEntry::new(
                "0191f2c4-0000-7000-8000-00000000abcd",
                receiver_session::ReceiverIdentity::from_wire(
                    "Pixel 8",
                    "android",
                    Some("14".into()),
                    Some("0.1.0".into()),
                ),
                Some("10.0.0.5".into()),
                1_700_000_000_000,
            );
            entry.display_name = Some("Lead Vocal".into());
            entry.last_heartbeat_at = Some(1_700_000_001_000);
            entry.packets_sent = 2_000;
            entry.packets_dropped_locally = 3;
            if query
                .get("withTelemetry")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                entry.telemetry = Some(receiver_session::ReceiverReportedTelemetry {
                    received_at: 1_700_000_002_000,
                    reported_at: 1_700_000_001_900,
                    packets_received: 1_990,
                    packets_lost: 10,
                    loss_percent: 0.5,
                    jitter_ms: 2.5,
                    buffer_health_ms: 40.0,
                    underruns: 0,
                    playback_state: receiver_session::PlaybackState::Playing,
                    output_latency_ms: Some(38.0),
                    resample_ratio_ppm: Some(-12.0),
                });
            }
            stagelink_service::serialize::roster_entry(&entry)
        }

        Some("aeadNonce") => {
            let session_id = query.get("sessionId").and_then(Value::as_u64).unwrap_or(0) as u32;
            let sequence = query.get("sequence").and_then(Value::as_u64).unwrap_or(0) as u32;
            json!({ "bytes": audio_protocol::aead_nonce(session_id, sequence).to_vec() })
        }

        other => json!({
            "ok": false,
            "reason": format!("unknown query: {other:?}"),
        }),
    }
}

/// Builds an envelope from the JSON description the test sends.
fn build_envelope(description: &Value) -> Option<desktop_ipc::codec::Envelope> {
    use desktop_ipc::codec::{Envelope, EnvelopeKind, ServiceErrorPayload};

    let kind = description.get("kind")?.as_str()?;
    let envelope = match kind {
        "request" => Envelope::new(EnvelopeKind::Request {
            id: description.get("id")?.as_str()?.to_owned(),
            method: description.get("method")?.as_str()?.to_owned(),
            params: description.get("params").cloned().unwrap_or(Value::Null),
        }),
        "event" => Envelope::event(
            description.get("event")?.as_str()?,
            description.get("payload").cloned().unwrap_or(Value::Null),
            description.get("at").and_then(Value::as_i64).unwrap_or(0),
        ),
        "response" => {
            let id = description.get("id")?.as_str()?.to_owned();
            if description.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                Envelope::ok_response(id, description.get("result").cloned().unwrap_or(Value::Null))
            } else {
                let error = description.get("error")?;
                Envelope::error_response(
                    id,
                    ServiceErrorPayload {
                        code: error.get("code")?.as_str()?.to_owned(),
                        message: error.get("message")?.as_str()?.to_owned(),
                        detail: error
                            .get("detail")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        retryable: error
                            .get("retryable")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        field: error.get("field").and_then(Value::as_str).map(str::to_owned),
                    },
                )
            }
        }
        _ => return None,
    };
    Some(envelope)
}

fn envelope_kind_name(envelope: &desktop_ipc::codec::Envelope) -> &'static str {
    use desktop_ipc::codec::EnvelopeKind;
    match envelope.kind {
        EnvelopeKind::Hello { .. } => "hello",
        EnvelopeKind::Welcome { .. } => "welcome",
        EnvelopeKind::Request { .. } => "request",
        EnvelopeKind::Response { .. } => "response",
        EnvelopeKind::Event { .. } => "event",
    }
}

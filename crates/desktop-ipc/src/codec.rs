//! NDJSON envelope framing.
//!
//! Mirrors `packages/shared-protocol/src/envelope.ts`. The two are kept in step
//! by `tests/protocol`, which drives real envelopes through both implementations.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{IpcError, Result};

/// Framing version. Bumped only when the envelope shape changes.
pub const ENVELOPE_VERSION: u8 = 1;

/// Hard cap on one line. Generous for control traffic (the largest realistic
/// message is a device list), small enough that a runaway or hostile peer
/// cannot exhaust memory.
pub const MAX_ENVELOPE_BYTES: usize = 1_048_576;

/// A structured error as it crosses the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl ServiceErrorPayload {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
            retryable: false,
            field: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub const fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}

/// A receiver's self-description, sent in its `hello`. Untrusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverIdentity {
    pub device_name: String,
    pub platform: String,
    pub platform_version: Option<String>,
    pub app_version: Option<String>,
    /// Long-term public key, hex. Not a secret.
    pub public_key: String,
    /// UDP port this receiver has **actually bound** for audio.
    ///
    /// Absent means "use the configured stream port", which is what an older
    /// receiver implies. Present is better and is what ours sends: the
    /// desktop then needs no assumption about where a receiver is listening,
    /// and two receivers sharing one IP address — two devices behind the same
    /// NAT, or two processes on one machine — do not collide on a single port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_port: Option<u16>,
}

/// Audio parameters an admitted receiver needs.
///
/// The sample rate lives here and nowhere else — the packet header does not
/// carry it, and a receiver that guessed wrong would play the mix sharp or flat
/// in somebody's ears.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamParams {
    pub session_id: u32,
    pub sample_rate: u32,
    pub frame_duration_ms: f64,
    pub payload_format: String,
    pub transport: String,
    pub stream_port: u16,
    pub frames_per_packet: u32,
}

/// The message kinds that travel on the control channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EnvelopeKind {
    /// Client -> service: authenticate and declare a protocol version.
    ///
    /// A receiver joins with this same message: its join token goes in `token`
    /// and its self-description in `receiver`. There is no separate join
    /// method, because the envelope contract requires `hello` to be first and
    /// to carry the credential.
    Hello {
        token: String,
        client: String,
        #[serde(rename = "controlProtocolVersion")]
        control_protocol_version: String,
        /// Present only when a receiver is joining.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receiver: Option<ReceiverIdentity>,
    },
    /// Service -> client: handshake accepted.
    Welcome {
        #[serde(rename = "serviceVersion")]
        service_version: String,
        #[serde(rename = "controlProtocolVersion")]
        control_protocol_version: String,
        #[serde(rename = "audioProtocolVersion")]
        audio_protocol_version: u8,
        #[serde(rename = "availableAudioApis")]
        available_audio_apis: Vec<String>,
        pid: u32,
        /// Present only when a receiver was admitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<StreamParams>,
        /// Fingerprint of the desktop's long-term key, for the receiver to pin.
        #[serde(
            default,
            rename = "keyFingerprint",
            skip_serializing_if = "Option::is_none"
        )]
        key_fingerprint: Option<String>,
    },
    Request {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
    Response {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ServiceErrorPayload>,
    },
    Event {
        event: String,
        payload: Value,
        at: i64,
    },
}

/// A framed control message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    #[serde(flatten)]
    pub kind: EnvelopeKind,
}

impl Envelope {
    #[must_use]
    pub const fn new(kind: EnvelopeKind) -> Self {
        Self { v: ENVELOPE_VERSION, kind }
    }

    #[must_use]
    pub fn ok_response(id: impl Into<String>, result: Value) -> Self {
        Self::new(EnvelopeKind::Response {
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        })
    }

    #[must_use]
    pub fn error_response(id: impl Into<String>, error: ServiceErrorPayload) -> Self {
        Self::new(EnvelopeKind::Response {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(error),
        })
    }

    #[must_use]
    pub fn event(event: impl Into<String>, payload: Value, at: i64) -> Self {
        Self::new(EnvelopeKind::Event { event: event.into(), payload, at })
    }

    /// Serialises to a single NDJSON line, terminator included.
    ///
    /// `serde_json` escapes control characters, so a message containing a
    /// newline cannot break the framing — the escaping test below pins that.
    pub fn encode(&self) -> Result<String> {
        let json = serde_json::to_string(self)
            .map_err(|error| IpcError::Malformed(format!("could not encode envelope: {error}")))?;
        if json.len() > MAX_ENVELOPE_BYTES {
            return Err(IpcError::EnvelopeTooLarge { limit: MAX_ENVELOPE_BYTES });
        }
        Ok(format!("{json}\n"))
    }

    /// Parses one NDJSON line. Never panics on hostile input.
    pub fn decode(line: &str) -> Result<Self> {
        if line.len() > MAX_ENVELOPE_BYTES {
            return Err(IpcError::EnvelopeTooLarge { limit: MAX_ENVELOPE_BYTES });
        }
        let envelope: Self = serde_json::from_str(line)
            .map_err(|error| IpcError::Malformed(format!("{error}")))?;
        if envelope.v != ENVELOPE_VERSION {
            return Err(IpcError::Malformed(format!(
                "envelope framing version {} is not supported (this build speaks {ENVELOPE_VERSION})",
                envelope.v
            )));
        }
        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_round_trips() {
        let envelope = Envelope::new(EnvelopeKind::Request {
            id: "r1".into(),
            method: "audio.getStatus".into(),
            params: json!({}),
        });
        let line = envelope.encode().unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(Envelope::decode(line.trim_end()).unwrap(), envelope);
    }

    #[test]
    fn a_response_round_trips_and_omits_the_unused_arm() {
        let envelope = Envelope::ok_response("r1", json!({ "state": "running" }));
        let line = envelope.encode().unwrap();
        assert!(!line.contains("\"error\""), "got {line}");
        assert_eq!(Envelope::decode(line.trim_end()).unwrap(), envelope);
    }

    #[test]
    fn an_error_response_carries_the_structured_payload() {
        let envelope = Envelope::error_response(
            "r1",
            ServiceErrorPayload::new("AUDIO_DEVICE_NOT_FOUND", "gone")
                .with_detail("endpoint vanished")
                .retryable(true),
        );
        let line = envelope.encode().unwrap();
        assert!(!line.contains("\"result\""));
        let decoded = Envelope::decode(line.trim_end()).unwrap();
        match decoded.kind {
            EnvelopeKind::Response { ok, error: Some(error), .. } => {
                assert!(!ok);
                assert_eq!(error.code, "AUDIO_DEVICE_NOT_FOUND");
                assert!(error.retryable);
                assert_eq!(error.detail.as_deref(), Some("endpoint vanished"));
            }
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    #[test]
    fn the_wire_form_uses_the_camel_case_names_the_typescript_side_expects() {
        let envelope = Envelope::new(EnvelopeKind::Welcome {
            service_version: "0.1.0".into(),
            control_protocol_version: "1.0.0".into(),
            audio_protocol_version: 1,
            available_audio_apis: vec!["wasapi".into()],
            pid: 1234,
            stream: None,
            key_fingerprint: None,
        });
        let line = envelope.encode().unwrap();
        assert!(line.contains("\"kind\":\"welcome\""), "got {line}");
        assert!(line.contains("\"serviceVersion\""));
        assert!(line.contains("\"controlProtocolVersion\""));
        assert!(line.contains("\"audioProtocolVersion\""));
        assert!(line.contains("\"availableAudioApis\""));
        // The receiver-only fields must vanish for the local client, whose
        // schema has them optional — emitting explicit nulls would be a
        // gratuitous difference between the two implementations.
        assert!(!line.contains("\"stream\""), "got {line}");
        assert!(!line.contains("\"keyFingerprint\""), "got {line}");
    }

    #[test]
    fn a_hello_decodes_from_the_typescript_wire_form() {
        let line = r#"{"v":1,"kind":"hello","token":"deadbeef","client":"stagelink-desktop/0.1.0","controlProtocolVersion":"1.0.0"}"#;
        match Envelope::decode(line).unwrap().kind {
            EnvelopeKind::Hello { token, client, control_protocol_version, receiver } => {
                assert_eq!(token, "deadbeef");
                assert_eq!(client, "stagelink-desktop/0.1.0");
                assert_eq!(control_protocol_version, "1.0.0");
                assert!(receiver.is_none(), "the local client is not a receiver");
            }
            other => panic!("expected a hello, got {other:?}"),
        }
    }

    /// A receiver's join, which is a `hello` and not a separate method: the
    /// envelope contract requires the first frame to carry the credential, so
    /// a second credential-bearing frame could never be first.
    #[test]
    fn a_receiver_hello_round_trips_with_its_identity() {
        let envelope = Envelope::new(EnvelopeKind::Hello {
            token: "0".repeat(64),
            client: "stagelink-mobile/0.1.0".into(),
            control_protocol_version: "1.0.0".into(),
            receiver: Some(ReceiverIdentity {
                device_name: "Pixel 8".into(),
                platform: "android".into(),
                platform_version: Some("14".into()),
                app_version: Some("0.1.0".into()),
                public_key: "ab".repeat(32),
                audio_port: Some(45_872),
            }),
        });
        let line = envelope.encode().unwrap();
        assert!(line.contains("\"deviceName\":\"Pixel 8\""), "got {line}");
        assert!(line.contains("\"platformVersion\""));
        assert!(line.contains("\"publicKey\""));
        assert_eq!(Envelope::decode(line.trim_end()).unwrap(), envelope);
    }

    /// The sample rate reaches a receiver only here. A `welcome` that carried
    /// no rate would leave the receiver guessing, and guessing wrong means
    /// playing the mix sharp or flat in a performer's ears.
    #[test]
    fn an_admitted_receivers_welcome_carries_the_stream_parameters() {
        let envelope = Envelope::new(EnvelopeKind::Welcome {
            service_version: "0.1.0".into(),
            control_protocol_version: "1.0.0".into(),
            audio_protocol_version: 1,
            available_audio_apis: vec!["wasapi".into()],
            pid: 1234,
            stream: Some(StreamParams {
                session_id: 0x5EED,
                sample_rate: 48_000,
                frame_duration_ms: 5.0,
                payload_format: "pcm_s16le".into(),
                transport: "udp-plaintext".into(),
                stream_port: 45872,
                frames_per_packet: 240,
            }),
            key_fingerprint: Some("fingerprint".into()),
        });
        let line = envelope.encode().unwrap();
        assert!(line.contains("\"sampleRate\":48000"), "got {line}");
        assert!(line.contains("\"framesPerPacket\":240"));
        assert!(line.contains("\"keyFingerprint\":\"fingerprint\""));
        assert_eq!(Envelope::decode(line.trim_end()).unwrap(), envelope);
    }

    #[test]
    fn newlines_in_a_payload_cannot_break_the_framing() {
        let envelope = Envelope::event(
            "service.log",
            json!({ "message": "line one\nline two\r\nline three" }),
            1,
        );
        let line = envelope.encode().unwrap();
        // Exactly one newline: the terminator.
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(Envelope::decode(line.trim_end()).unwrap(), envelope);
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(matches!(
            Envelope::decode("{ not json"),
            Err(IpcError::Malformed(_))
        ));
        assert!(Envelope::decode("").is_err());
        assert!(Envelope::decode("null").is_err());
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        let line = r#"{"v":1,"kind":"exec","command":"calc.exe"}"#;
        assert!(Envelope::decode(line).is_err());
    }

    #[test]
    fn a_future_framing_version_is_rejected_with_a_clear_message() {
        let line = r#"{"v":9,"kind":"request","id":"a","method":"audio.getStatus","params":{}}"#;
        match Envelope::decode(line) {
            Err(IpcError::Malformed(message)) => assert!(message.contains("framing version 9")),
            other => panic!("expected a version complaint, got {other:?}"),
        }
    }

    #[test]
    fn an_oversized_line_is_refused_before_parsing() {
        let line = "x".repeat(MAX_ENVELOPE_BYTES + 1);
        assert!(matches!(
            Envelope::decode(&line),
            Err(IpcError::EnvelopeTooLarge { .. })
        ));
    }

    #[test]
    fn a_request_with_no_params_field_decodes_to_null() {
        let line = r#"{"v":1,"kind":"request","id":"a","method":"service.ping"}"#;
        match Envelope::decode(line).unwrap().kind {
            EnvelopeKind::Request { params, .. } => assert!(params.is_null()),
            other => panic!("expected a request, got {other:?}"),
        }
    }
}

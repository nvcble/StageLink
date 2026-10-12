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

/// The message kinds that travel on the control channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EnvelopeKind {
    /// Client -> service: authenticate and declare a protocol version.
    Hello {
        token: String,
        client: String,
        #[serde(rename = "controlProtocolVersion")]
        control_protocol_version: String,
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
        });
        let line = envelope.encode().unwrap();
        assert!(line.contains("\"kind\":\"welcome\""), "got {line}");
        assert!(line.contains("\"serviceVersion\""));
        assert!(line.contains("\"controlProtocolVersion\""));
        assert!(line.contains("\"audioProtocolVersion\""));
        assert!(line.contains("\"availableAudioApis\""));
    }

    #[test]
    fn a_hello_decodes_from_the_typescript_wire_form() {
        let line = r#"{"v":1,"kind":"hello","token":"deadbeef","client":"stagelink-desktop/0.1.0","controlProtocolVersion":"1.0.0"}"#;
        match Envelope::decode(line).unwrap().kind {
            EnvelopeKind::Hello { token, client, control_protocol_version } => {
                assert_eq!(token, "deadbeef");
                assert_eq!(client, "stagelink-desktop/0.1.0");
                assert_eq!(control_protocol_version, "1.0.0");
            }
            other => panic!("expected a hello, got {other:?}"),
        }
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

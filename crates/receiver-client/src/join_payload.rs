//! Parsing the QR code.
//!
//! The payload grammar is frozen in `docs/protocol/pairing.md`:
//!
//! ```text
//! stagelink://join?h=<host>&c=<controlPort>&s=<streamPort>&t=<token>&v=<version>
//! ```
//!
//! Hand-parsed rather than pulled through a URL crate. The grammar is five
//! known keys and the input is a string a camera read off a screen, so the
//! useful work is **rejecting** things — and a URL parser would happily accept
//! a different scheme, a different path, or a token with the wrong length and
//! leave the checking to be done afterwards anyway.

use std::net::SocketAddr;

/// Why a scanned payload could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    #[error("this is not a StageLink join code")]
    NotAJoinCode,

    #[error("the join code is missing its {0}")]
    Missing(&'static str),

    #[error("the join code's {field} is not valid: {value}")]
    Invalid { field: &'static str, value: String },

    #[error(
        "this desktop speaks StageLink control protocol {theirs}, this app speaks {ours}; one \
         of them needs updating"
    )]
    IncompatibleVersion { theirs: String, ours: String },
}

/// Everything a receiver needs in order to join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinPayload {
    pub host: String,
    pub control_port: u16,
    pub stream_port: u16,
    /// The bearer credential. **Never persisted** — see receiver obligation 9.
    pub token: String,
    pub control_protocol_version: String,
}

impl JoinPayload {
    /// Parses a scanned payload and checks it against `ours`.
    ///
    /// The version is checked here, at the point of scanning, because that is
    /// where a person is standing in front of the laptop and can be told
    /// something useful. Failing later, during the handshake, would show up as
    /// a connection that silently does not work.
    pub fn parse(payload: &str, ours: &str) -> Result<Self, PayloadError> {
        let query = payload
            .trim()
            .strip_prefix("stagelink://join?")
            .ok_or(PayloadError::NotAJoinCode)?;

        let mut host = None;
        let mut control_port = None;
        let mut stream_port = None;
        let mut token = None;
        let mut version = None;

        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match key {
                "h" => host = Some(value),
                "c" => control_port = Some(value),
                "s" => stream_port = Some(value),
                "t" => token = Some(value),
                "v" => version = Some(value),
                // Unknown keys are ignored rather than rejected, so a newer
                // desktop adding a field does not make its codes unscannable
                // by an older app.
                _ => {}
            }
        }

        let host = host.ok_or(PayloadError::Missing("address"))?;
        if host.is_empty() {
            return Err(PayloadError::Missing("address"));
        }
        let token = token.ok_or(PayloadError::Missing("token"))?;
        // 256 bits of hex. Checked because a truncated scan is a realistic
        // failure and produces a token that would simply be rejected, which
        // looks like "the code does not work" rather than "rescan it".
        if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PayloadError::Invalid {
                field: "token",
                value: format!("{} characters", token.len()),
            });
        }
        let version = version.ok_or(PayloadError::Missing("version"))?;

        let port = |field: &'static str, value: Option<&str>| {
            value
                .ok_or(PayloadError::Missing(field))?
                .parse::<u16>()
                .ok()
                .filter(|port| *port >= 1_024)
                .ok_or_else(|| PayloadError::Invalid {
                    field,
                    value: value.unwrap_or_default().to_owned(),
                })
        };

        let payload = Self {
            host: host.to_owned(),
            control_port: port("control port", control_port)?,
            stream_port: port("stream port", stream_port)?,
            token: token.to_owned(),
            control_protocol_version: version.to_owned(),
        };

        if !major_matches(&payload.control_protocol_version, ours) {
            return Err(PayloadError::IncompatibleVersion {
                theirs: payload.control_protocol_version,
                ours: ours.to_owned(),
            });
        }

        Ok(payload)
    }

    /// The control channel's WebSocket URL.
    #[must_use]
    pub fn control_url(&self) -> String {
        format!("ws://{}:{}/", self.host, self.control_port)
    }

    /// Where to bind the audio socket.
    ///
    /// All interfaces, not the desktop's address: the receiver is *listening*,
    /// and on a phone the Wi-Fi interface's address is not known in advance
    /// and changes when the network does.
    #[must_use]
    pub fn audio_bind(&self) -> String {
        format!("0.0.0.0:{}", self.stream_port)
    }

    /// The desktop's address, for logging and for checking that audio came
    /// from the machine that was joined.
    pub fn desktop_audio_address(&self) -> Option<SocketAddr> {
        format!("{}:{}", self.host, self.stream_port).parse().ok()
    }
}

fn major_matches(theirs: &str, ours: &str) -> bool {
    let major = |version: &str| {
        version
            .split('.')
            .next()
            .and_then(|part| part.parse::<u32>().ok())
    };
    match (major(theirs), major(ours)) {
        (Some(theirs), Some(ours)) => theirs == ours,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "1.0.0";
    const TOKEN: &str = "9f2c3d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f";

    fn payload() -> String {
        format!("stagelink://join?h=10.0.0.1&c=45871&s=45872&t={TOKEN}&v=1.0.0")
    }

    #[test]
    fn a_real_payload_parses() {
        let parsed = JoinPayload::parse(&payload(), OURS).unwrap();
        assert_eq!(parsed.host, "10.0.0.1");
        assert_eq!(parsed.control_port, 45_871);
        assert_eq!(parsed.stream_port, 45_872);
        assert_eq!(parsed.token, TOKEN);
        assert_eq!(parsed.control_url(), "ws://10.0.0.1:45871/");
        assert_eq!(parsed.audio_bind(), "0.0.0.0:45872");
    }

    #[test]
    fn whitespace_around_a_scan_is_tolerated() {
        let padded = format!("  {}\n", payload());
        assert!(JoinPayload::parse(&padded, OURS).is_ok());
    }

    #[test]
    fn keys_may_arrive_in_any_order() {
        let reordered =
            format!("stagelink://join?v=1.0.0&t={TOKEN}&s=45872&c=45871&h=10.0.0.1");
        assert_eq!(
            JoinPayload::parse(&reordered, OURS).unwrap(),
            JoinPayload::parse(&payload(), OURS).unwrap()
        );
    }

    /// A newer desktop adding a field must not make its codes unscannable by
    /// an app that has not been updated.
    #[test]
    fn an_unknown_key_is_ignored_rather_than_rejected() {
        let extended = format!("{}&fp=ABCD&future=1", payload());
        assert!(JoinPayload::parse(&extended, OURS).is_ok());
    }

    #[test]
    fn something_that_is_not_a_join_code_is_refused_clearly() {
        for input in [
            "https://example.com/",
            "stagelink://other?h=1",
            "",
            "just some text",
            // A QR from a different product.
            "WIFI:S:MyNetwork;T:WPA;P:secret;;",
        ] {
            assert_eq!(
                JoinPayload::parse(input, OURS),
                Err(PayloadError::NotAJoinCode),
                "for {input:?}"
            );
        }
    }

    /// Refusing at the scan, not at the handshake: a person is standing in
    /// front of the laptop and can be told to update something, where a failed
    /// connection later just looks broken.
    #[test]
    fn an_incompatible_major_version_is_refused_at_the_scan() {
        let future = format!("stagelink://join?h=10.0.0.1&c=45871&s=45872&t={TOKEN}&v=2.0.0");
        match JoinPayload::parse(&future, OURS) {
            Err(PayloadError::IncompatibleVersion { theirs, ours }) => {
                assert_eq!(theirs, "2.0.0");
                assert_eq!(ours, OURS);
            }
            other => panic!("expected a version refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_newer_minor_is_accepted() {
        let newer = format!("stagelink://join?h=10.0.0.1&c=45871&s=45872&t={TOKEN}&v=1.9.2");
        assert!(JoinPayload::parse(&newer, OURS).is_ok());
    }

    /// A partial scan produces a short token, which would otherwise be
    /// rejected by the desktop and look like "the code does not work" rather
    /// than "hold the phone still".
    #[test]
    fn a_truncated_token_is_caught_before_connecting() {
        let short = format!("stagelink://join?h=10.0.0.1&c=45871&s=45872&t={}&v=1.0.0", &TOKEN[..40]);
        match JoinPayload::parse(&short, OURS) {
            Err(PayloadError::Invalid { field, .. }) => assert_eq!(field, "token"),
            other => panic!("expected a token complaint, got {other:?}"),
        }

        let not_hex = format!(
            "stagelink://join?h=10.0.0.1&c=45871&s=45872&t={}&v=1.0.0",
            "z".repeat(64)
        );
        assert!(matches!(
            JoinPayload::parse(&not_hex, OURS),
            Err(PayloadError::Invalid { field: "token", .. })
        ));
    }

    #[test]
    fn a_missing_field_says_which_one() {
        let cases = [
            (format!("stagelink://join?c=45871&s=45872&t={TOKEN}&v=1.0.0"), "address"),
            ("stagelink://join?h=10.0.0.1&c=45871&s=45872&v=1.0.0".to_owned(), "token"),
            (format!("stagelink://join?h=10.0.0.1&c=45871&s=45872&t={TOKEN}"), "version"),
            (format!("stagelink://join?h=10.0.0.1&s=45872&t={TOKEN}&v=1.0.0"), "control port"),
            (format!("stagelink://join?h=10.0.0.1&c=45871&t={TOKEN}&v=1.0.0"), "stream port"),
        ];
        for (input, expected) in cases {
            match JoinPayload::parse(&input, OURS) {
                Err(PayloadError::Missing(field)) => assert_eq!(field, expected),
                other => panic!("for {input}: expected {expected} missing, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_privileged_or_nonsense_port_is_refused() {
        for port in ["0", "80", "70000", "notaport", ""] {
            let input =
                format!("stagelink://join?h=10.0.0.1&c={port}&s=45872&t={TOKEN}&v=1.0.0");
            assert!(
                matches!(
                    JoinPayload::parse(&input, OURS),
                    Err(PayloadError::Invalid { field: "control port", .. })
                        | Err(PayloadError::Missing("control port"))
                ),
                "port {port} should have been refused"
            );
        }
    }

    #[test]
    fn a_hostname_is_accepted_as_well_as_an_address() {
        let named =
            format!("stagelink://join?h=stage-laptop.local&c=45871&s=45872&t={TOKEN}&v=1.0.0");
        let parsed = JoinPayload::parse(&named, OURS).unwrap();
        assert_eq!(parsed.control_url(), "ws://stage-laptop.local:45871/");
        // Not resolvable to a socket address without DNS, which is fine: the
        // WebSocket client resolves it.
        assert!(parsed.desktop_audio_address().is_none());
    }
}

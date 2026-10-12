//! Join codes.
//!
//! A receiver is authorised by presenting a token it got from the QR code on
//! the operator's screen. There is no approval step: being in the room with the
//! operator *is* the authorisation.
//!
//! That makes the token a bearer credential, so two properties matter:
//!
//! * **It expires.** Anyone who photographs the screen can join while the code
//!   lives, so it does not live long ([`DEFAULT_TTL_MS`]).
//! * **It can be rotated.** Minting a new code invalidates the old one
//!   immediately, which is the operator's answer to "that got shared".
//!
//! Revoking an individual receiver is separate and still available — a device
//! that already joined keeps its authorisation until revoked, independently of
//! whether the code it used has expired.

use serde::{Deserialize, Serialize};

/// How long a freshly minted code stays valid.
pub const DEFAULT_TTL_MS: i64 = 300_000;

/// Bytes of randomness in a token (256 bits, hex-encoded to 64 characters).
pub const TOKEN_BYTES: usize = 32;

/// A short-lived credential that authorises any receiver presenting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCode {
    pub token: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

impl JoinCode {
    /// Mints a code valid for `ttl_ms` from `now_ms`.
    #[must_use]
    pub fn mint(now_ms: i64, ttl_ms: i64) -> Self {
        Self {
            token: random_token(),
            issued_at: now_ms,
            expires_at: now_ms.saturating_add(ttl_ms.max(1)),
        }
    }

    #[must_use]
    pub fn is_valid(&self, now_ms: i64) -> bool {
        self.expires_at > now_ms
    }

    /// Milliseconds remaining, clamped at zero.
    #[must_use]
    pub fn time_remaining(&self, now_ms: i64) -> i64 {
        (self.expires_at - now_ms).max(0)
    }

    /// Whether `presented` matches this code and the code is still valid.
    ///
    /// Compared in constant time so a wrong token leaks nothing about the right
    /// one. This is the check the control server performs when a receiver
    /// connects (Phase 3/5).
    #[must_use]
    pub fn accepts(&self, presented: &str, now_ms: i64) -> bool {
        self.is_valid(now_ms) && constant_time_eq(&self.token, presented)
    }

    /// Builds the string the QR encodes.
    ///
    /// Short keys keep the QR small, which matters more than it sounds: fewer
    /// modules means a phone can read it from further away, in stage lighting,
    /// off a laptop screen.
    ///
    /// ```text
    /// stagelink://join?h=<host>&c=<control>&s=<stream>&t=<token>&v=<protocol>
    /// ```
    #[must_use]
    pub fn qr_payload(
        &self,
        host: &str,
        control_port: u16,
        stream_port: u16,
        protocol_version: &str,
    ) -> String {
        format!(
            "stagelink://join?h={host}&c={control_port}&s={stream_port}&t={}&v={protocol_version}",
            self.token
        )
    }
}

/// 256 bits from the OS generator. A guessable token would make the whole
/// scheme worthless.
fn random_token() -> String {
    use rand::CryptoRng;
    fn fill(rng: &mut impl CryptoRng, bytes: &mut [u8]) {
        rng.fill_bytes(bytes);
    }
    let mut bytes = [0_u8; TOKEN_BYTES];
    fill(&mut rand::rng(), &mut bytes);
    hex::encode(bytes)
}

/// Compares two tokens without leaking their relationship through timing.
fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_code_is_valid_until_its_ttl_elapses() {
        let code = JoinCode::mint(1_000, 5_000);
        assert!(code.is_valid(1_000));
        assert!(code.is_valid(5_999));
        assert!(!code.is_valid(6_000), "expiry is exclusive");
        assert_eq!(code.time_remaining(2_000), 4_000);
        assert_eq!(code.time_remaining(99_999), 0, "never negative");
    }

    #[test]
    fn tokens_are_long_and_never_repeat() {
        let first = JoinCode::mint(0, DEFAULT_TTL_MS);
        let second = JoinCode::mint(0, DEFAULT_TTL_MS);
        assert_eq!(first.token.len(), TOKEN_BYTES * 2);
        assert!(first.token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first.token, second.token, "rotation must produce a new token");
    }

    #[test]
    fn the_right_token_is_accepted_and_anything_else_is_not() {
        let code = JoinCode::mint(0, 10_000);
        assert!(code.accepts(&code.token, 500));

        assert!(!code.accepts("", 500));
        assert!(!code.accepts(&code.token[..32], 500), "a prefix must not match");
        assert!(!code.accepts(&format!("{}0", code.token), 500));

        let mut tampered = code.token.clone();
        tampered.replace_range(0..1, if code.token.starts_with('a') { "b" } else { "a" });
        assert!(!code.accepts(&tampered, 500));
    }

    #[test]
    fn an_expired_code_accepts_nothing_even_with_the_right_token() {
        let code = JoinCode::mint(0, 1_000);
        assert!(code.accepts(&code.token, 999));
        assert!(
            !code.accepts(&code.token, 1_000),
            "a photographed QR must stop working once it expires"
        );
    }

    #[test]
    fn the_qr_payload_carries_everything_a_receiver_needs() {
        let code = JoinCode::mint(0, DEFAULT_TTL_MS);
        let payload = code.qr_payload("10.0.0.1", 45_871, 45_872, "1.0.0");

        assert!(payload.starts_with("stagelink://join?"));
        assert!(payload.contains("h=10.0.0.1"));
        assert!(payload.contains("c=45871"));
        assert!(payload.contains("s=45872"));
        assert!(payload.contains(&format!("t={}", code.token)));
        assert!(payload.contains("v=1.0.0"));
    }

    #[test]
    fn the_qr_payload_stays_small_enough_to_scan_easily() {
        let payload = JoinCode::mint(0, DEFAULT_TTL_MS).qr_payload(
            "192.168.100.103",
            45_871,
            45_872,
            "1.0.0",
        );
        // Under 150 bytes keeps the QR at a low version, so it reads from
        // across a stage rather than needing a phone held against the screen.
        assert!(payload.len() < 150, "payload is {} bytes", payload.len());
    }

    #[test]
    fn a_code_round_trips_through_json_in_protocol_form() {
        let code = JoinCode::mint(1, DEFAULT_TTL_MS);
        let json = serde_json::to_string(&code).unwrap();
        assert!(json.contains("\"issuedAt\""), "got {json}");
        assert!(json.contains("\"expiresAt\""));
        assert_eq!(serde_json::from_str::<JoinCode>(&json).unwrap(), code);
    }
}

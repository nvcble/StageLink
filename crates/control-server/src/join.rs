//! Admitting a receiver.
//!
//! The whole authorisation decision, in one place, so there is exactly one
//! answer to "who is allowed in" and it can be read in one sitting.

use std::net::SocketAddr;

use desktop_ipc::codec::ReceiverIdentity as WireIdentity;
use receiver_session::{JoinCode, ReceiverIdentity};

/// Why a receiver was refused.
///
/// The variants are distinct because the *operator* needs to tell them apart —
/// "the code expired" and "the session is full" call for different actions.
/// The receiver is told none of this: it sees its connection close. See the
/// open item in `docs/protocol/pairing.md`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    #[error("the first message was not a hello")]
    NotAHello,

    #[error("the hello carried no receiver description, so it is not a receiver")]
    NotAReceiver,

    #[error("no join code has been minted")]
    NoJoinCode,

    #[error("the join code has expired")]
    Expired,

    #[error("the token does not match the current join code")]
    TokenRejected,

    #[error("the receiver limit of {limit} has been reached")]
    LimitReached { limit: u16 },

    #[error("this device's authorisation was withdrawn")]
    Revoked,

    #[error("the receiver speaks control protocol {peer}, this desktop speaks {ours}")]
    ProtocolMismatch { peer: String, ours: String },
}

impl JoinError {
    /// The protocol error code an operator sees in the log.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LimitReached { .. } => "RECEIVER_LIMIT_REACHED",
            Self::ProtocolMismatch { .. } => "PROTOCOL_MISMATCH",
            Self::NotAHello | Self::NotAReceiver => "INVALID_REQUEST",
            _ => "PAIRING_REJECTED",
        }
    }
}

/// An admitted receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    /// Desktop-assigned, stable for as long as the device keeps its key.
    pub receiver_id: String,
    pub identity: ReceiverIdentity,
    pub address: SocketAddr,
}

/// Everything the decision depends on.
pub struct JoinRequest<'a> {
    pub token: &'a str,
    pub control_protocol_version: &'a str,
    pub receiver: Option<&'a WireIdentity>,
    pub address: SocketAddr,
    pub now_ms: i64,
}

/// Decides whether to admit a receiver.
///
/// Order matters. The token is checked **before** the receiver limit, so a
/// stranger cannot learn that the session is full — and after the protocol
/// version, so a receiver that could never work is turned away without a
/// constant-time comparison it was going to fail anyway.
pub fn admit(
    request: &JoinRequest<'_>,
    code: Option<&JoinCode>,
    ours: &str,
    current_receivers: usize,
    limit: u16,
    is_revoked: impl Fn(&str) -> bool,
) -> Result<Admission, JoinError> {
    let Some(receiver) = request.receiver else {
        return Err(JoinError::NotAReceiver);
    };

    if !major_versions_match(request.control_protocol_version, ours) {
        return Err(JoinError::ProtocolMismatch {
            peer: request.control_protocol_version.to_owned(),
            ours: ours.to_owned(),
        });
    }

    let Some(code) = code else {
        return Err(JoinError::NoJoinCode);
    };
    if !code.is_valid(request.now_ms) {
        return Err(JoinError::Expired);
    }
    // Constant time, so a wrong token leaks nothing about the right one. A
    // token is 256 bits, and a timing oracle is the only realistic way to
    // attack it.
    if !code.accepts(request.token, request.now_ms) {
        return Err(JoinError::TokenRejected);
    }

    let receiver_id = receiver_id_for(&receiver.public_key);
    if is_revoked(&receiver_id) {
        // A revoked device holding a valid code is still refused: revocation
        // is the operator's override, and a fresh QR must not silently undo
        // it. They have to be un-revoked deliberately.
        return Err(JoinError::Revoked);
    }

    if current_receivers >= usize::from(limit) {
        return Err(JoinError::LimitReached { limit });
    }

    Ok(Admission {
        receiver_id,
        identity: ReceiverIdentity::from_wire(
            &receiver.device_name,
            &receiver.platform,
            receiver.platform_version.clone(),
            receiver.app_version.clone(),
        ),
        address: request.address,
    })
}

/// A stable identifier derived from the device's long-term public key.
///
/// Derived rather than assigned, so a device that reconnects is recognised as
/// the same one — which is what lets an operator's "Lead Vocal" label and a
/// revocation both survive a reconnect. The key is not a secret, so the hash
/// is for stability and length, not confidentiality.
#[must_use]
pub fn receiver_id_for(public_key: &str) -> String {
    // FNV-1a over the key, hex. Not cryptographic, and does not need to be:
    // a collision would merge two devices' labels, not grant access, because
    // access is decided by the join token above.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in public_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Semver major comparison. A differing major cannot interoperate.
fn major_versions_match(peer: &str, ours: &str) -> bool {
    let major = |version: &str| {
        version
            .split('.')
            .next()
            .and_then(|part| part.parse::<u32>().ok())
    };
    match (major(peer), major(ours)) {
        (Some(peer), Some(ours)) => peer == ours,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "1.0.0";
    const NOW: i64 = 1_700_000_000_000;

    fn identity() -> WireIdentity {
        WireIdentity {
            device_name: "Pixel 8".to_owned(),
            platform: "android".to_owned(),
            platform_version: Some("14".to_owned()),
            app_version: Some("0.1.0".to_owned()),
            public_key: "ab".repeat(32),
        }
    }

    fn address() -> SocketAddr {
        "10.0.0.5:51000".parse().unwrap()
    }

    fn request<'a>(token: &'a str, receiver: &'a WireIdentity) -> JoinRequest<'a> {
        JoinRequest {
            token,
            control_protocol_version: OURS,
            receiver: Some(receiver),
            address: address(),
            now_ms: NOW,
        }
    }

    fn code() -> JoinCode {
        JoinCode::mint(NOW, receiver_session::join_code::DEFAULT_TTL_MS)
    }

    fn never_revoked(_: &str) -> bool {
        false
    }

    #[test]
    fn a_valid_token_is_admitted_and_nothing_else_is_asked_of_it() {
        let code = code();
        let identity = identity();
        let admission = admit(
            &request(&code.token, &identity),
            Some(&code),
            OURS,
            0,
            16,
            never_revoked,
        )
        .expect("a valid code is the whole authorisation");

        assert_eq!(admission.identity.device_name, "Pixel 8");
        assert_eq!(admission.address, address());
        assert!(!admission.receiver_id.is_empty());
    }

    #[test]
    fn a_wrong_token_is_refused() {
        let code = code();
        let identity = identity();
        let error = admit(
            &request(&"0".repeat(64), &identity),
            Some(&code),
            OURS,
            0,
            16,
            never_revoked,
        )
        .expect_err("a wrong token must not be admitted");
        assert_eq!(error, JoinError::TokenRejected);
        assert_eq!(error.code(), "PAIRING_REJECTED");
    }

    #[test]
    fn an_expired_code_is_refused_even_with_the_right_token() {
        let code = code();
        let identity = identity();
        let mut request = request(&code.token, &identity);
        // Five minutes and a second later.
        request.now_ms = NOW + 301_000;

        let error = admit(&request, Some(&code), OURS, 0, 16, never_revoked)
            .expect_err("expiry is the point of a short-lived credential");
        assert_eq!(error, JoinError::Expired);
    }

    #[test]
    fn with_no_code_minted_nobody_gets_in() {
        let identity = identity();
        let error = admit(
            &request(&"a".repeat(64), &identity),
            None,
            OURS,
            0,
            16,
            never_revoked,
        )
        .expect_err("no code means no admission");
        assert_eq!(error, JoinError::NoJoinCode);
    }

    /// The operator's override must not be undone by a fresh QR code: they
    /// revoked a device for a reason, and the next code they display is for
    /// somebody else.
    #[test]
    fn a_revoked_device_is_refused_even_with_a_fresh_valid_code() {
        let code = code();
        let identity = identity();
        let revoked_id = receiver_id_for(&identity.public_key);

        let error = admit(
            &request(&code.token, &identity),
            Some(&code),
            OURS,
            0,
            16,
            |id| id == revoked_id,
        )
        .expect_err("revocation outranks a valid code");
        assert_eq!(error, JoinError::Revoked);
    }

    /// Checked after the token, so a stranger cannot discover that the session
    /// is full — that is information about the event, and they have not proved
    /// they belong at it.
    #[test]
    fn the_receiver_limit_is_enforced_but_only_after_the_token() {
        let code = code();
        let identity = identity();

        let error = admit(
            &request(&code.token, &identity),
            Some(&code),
            OURS,
            16,
            16,
            never_revoked,
        )
        .expect_err("the session is full");
        assert_eq!(error, JoinError::LimitReached { limit: 16 });
        assert_eq!(error.code(), "RECEIVER_LIMIT_REACHED");

        // A stranger at a full session is told it was their token, not that
        // the session is full.
        let error = admit(
            &request(&"0".repeat(64), &identity),
            Some(&code),
            OURS,
            16,
            16,
            never_revoked,
        )
        .expect_err("a wrong token at a full session");
        assert_eq!(error, JoinError::TokenRejected);
    }

    #[test]
    fn an_incompatible_protocol_major_is_refused_before_the_token_is_compared() {
        let code = code();
        let identity = identity();
        let mut request = request(&code.token, &identity);
        request.control_protocol_version = "2.0.0";

        let error = admit(&request, Some(&code), OURS, 0, 16, never_revoked)
            .expect_err("a different major cannot interoperate");
        assert!(matches!(error, JoinError::ProtocolMismatch { .. }));
    }

    #[test]
    fn a_newer_minor_is_accepted() {
        let code = code();
        let identity = identity();
        let mut request = request(&code.token, &identity);
        request.control_protocol_version = "1.7.3";
        assert!(admit(&request, Some(&code), OURS, 0, 16, never_revoked).is_ok());
    }

    #[test]
    fn a_hello_with_no_receiver_block_is_not_a_receiver() {
        let code = code();
        let request = JoinRequest {
            token: &code.token,
            control_protocol_version: OURS,
            receiver: None,
            address: address(),
            now_ms: NOW,
        };
        let error = admit(&request, Some(&code), OURS, 0, 16, never_revoked)
            .expect_err("the local client is not a receiver");
        assert_eq!(error, JoinError::NotAReceiver);
    }

    /// The property that makes the operator's label and a revocation survive a
    /// reconnect: the same device gets the same id, every time.
    #[test]
    fn the_same_device_always_gets_the_same_id() {
        let key = "ab".repeat(32);
        assert_eq!(receiver_id_for(&key), receiver_id_for(&key));
        assert_ne!(receiver_id_for(&key), receiver_id_for(&"cd".repeat(32)));
        assert_eq!(receiver_id_for(&key).len(), 16);
    }

    #[test]
    fn a_self_reported_name_is_clamped_on_the_way_in() {
        let code = code();
        let mut identity = identity();
        identity.device_name = "x".repeat(500);
        identity.platform = "PalmOS".to_owned();

        let admission = admit(
            &request(&code.token, &identity),
            Some(&code),
            OURS,
            0,
            16,
            never_revoked,
        )
        .unwrap();
        assert_eq!(admission.identity.device_name.chars().count(), 120);
        assert_eq!(
            admission.identity.platform,
            receiver_session::ReceiverPlatform::Unknown,
            "an unrecognised platform is reported as unknown, not guessed at"
        );
    }
}

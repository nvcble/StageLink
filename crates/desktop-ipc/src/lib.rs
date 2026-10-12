//! Authenticated local control channel.
//!
//! The Electron main process and the Rust audio service talk over a Windows
//! named pipe carrying newline-delimited JSON. Audio never travels on this
//! channel — only control requests, responses and lightweight telemetry.
//!
//! Three things make it trustworthy:
//!
//! 1. **The pipe name is unguessable.** It embeds 128 bits of randomness
//!    generated at service start, so nothing can squat the name before us.
//! 2. **The DACL admits only the creating user.** See [`security`]. Without
//!    this, Windows' default named-pipe permissions would let any local
//!    account connect.
//! 3. **The first line must carry the right token.** Electron receives the
//!    token on the service's command line and presents it in a `hello`.
//!    Comparison is constant-time, and a single bad handshake drops the
//!    connection rather than giving an attacker a retry loop on one socket.
//!
//! The server accepts one client at a time — there is exactly one Electron main
//! process — but it accepts them *sequentially*, so the renderer reloading or
//! Electron restarting does not require restarting the audio service.

pub mod codec;
pub mod error;
pub mod handler;

#[cfg(windows)]
pub mod security;

#[cfg(windows)]
pub mod server;

pub use codec::{Envelope, EnvelopeKind, ServiceErrorPayload};
pub use error::{IpcError, Result};
pub use handler::{ControlHandler, EventSink};

#[cfg(windows)]
pub use server::{IpcServer, ServerConfig};

/// Bytes of randomness in the pipe name and in the handshake token.
pub const TOKEN_BYTES: usize = 16;

/// Builds the pipe path for a given instance id.
///
/// Example: `\\.\pipe\stagelink-service-9f2c...`.
#[must_use]
pub fn pipe_path(instance_id: &str) -> String {
    format!(r"\\.\pipe\stagelink-service-{instance_id}")
}

/// Generates a hex-encoded random value for a pipe instance id or a token.
#[must_use]
pub fn random_hex() -> String {
    // `rand::rng()` is the OS-seeded, cryptographically secure generator. A
    // predictable token here would make the handshake worthless.
    // `rand::rng()` is the OS-seeded thread generator and is `CryptoRng`; a
    // predictable token would make the handshake worthless.
    use rand::CryptoRng;
    fn fill(rng: &mut impl CryptoRng, bytes: &mut [u8]) {
        rng.fill_bytes(bytes);
    }
    let mut bytes = [0_u8; TOKEN_BYTES];
    fill(&mut rand::rng(), &mut bytes);
    hex::encode(bytes)
}

/// Compares two secrets without leaking their relationship through timing.
///
/// Length is compared first and non-secretly — the token length is fixed and
/// public, so there is nothing to hide there.
#[must_use]
pub fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_paths_use_the_windows_namespace() {
        let path = pipe_path("abc123");
        assert!(path.starts_with(r"\\.\pipe\"));
        assert!(path.ends_with("abc123"));
    }

    #[test]
    fn random_hex_is_the_expected_length_and_not_repeated() {
        let first = random_hex();
        let second = random_hex();
        assert_eq!(first.len(), TOKEN_BYTES * 2);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "tokens must not repeat between calls");
    }

    #[test]
    fn constant_time_comparison_agrees_with_equality() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn an_empty_token_never_authenticates_against_a_real_one() {
        let token = random_hex();
        assert!(!constant_time_eq(&token, ""));
        assert!(!constant_time_eq("", &token));
    }
}

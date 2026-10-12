//! Status codes.
//!
//! Distinct codes for distinct *instructions to a person*, not for distinct
//! internal causes. "Rescan the code", "update the app" and "check the
//! network" are three different things somebody has to do; six flavours of
//! parse failure are not.

use receiver_client::PayloadError;

/// Returned by every call.
///
/// `repr(i32)` and negative on failure, so the render functions can return
/// either a frame count or a status in one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SlrStatus {
    Ok = 0,
    /// A null pointer, a zero length, or a string that was not UTF-8.
    InvalidArgument = -1,
    /// The QR payload is not a usable StageLink join code. Rescan it.
    BadJoinCode = -2,
    /// The desktop speaks a different major protocol version. Update one of
    /// them. Deliberately separate from `BadJoinCode`: rescanning will not
    /// help and telling somebody to try again would waste their time.
    IncompatibleVersion = -3,
    /// The desktop did not answer. Check that both are on the same network.
    CouldNotConnect = -4,
    /// The desktop refused the code. Ask the operator for a fresh one.
    Refused = -5,
    /// The audio port could not be opened.
    AudioPortUnavailable = -6,
    /// Something went wrong inside the library.
    Internal = -7,
    /// A panic was caught at the boundary. Always a bug here, never the
    /// caller's fault — but reported rather than left to unwind into the app.
    Panicked = -8,
}

/// Maps a payload parse failure.
#[must_use]
pub fn from_payload(error: PayloadError) -> SlrStatus {
    match error {
        PayloadError::IncompatibleVersion { .. } => SlrStatus::IncompatibleVersion,
        _ => SlrStatus::BadJoinCode,
    }
}

/// Maps a join failure.
///
/// Matched on the message because the failures come from several layers — a
/// socket error, a WebSocket handshake error, a refusal with no body — and
/// wrapping each in its own error type would be more machinery than the three
/// distinct instructions warrant.
#[must_use]
pub fn from_join(error: &anyhow::Error) -> SlrStatus {
    let text = error.to_string().to_ascii_lowercase();
    let chain = error
        .chain()
        .map(|cause| cause.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    if text.contains("refused the join code") {
        SlrStatus::Refused
    } else if chain.contains("audio socket") {
        SlrStatus::AudioPortUnavailable
    } else if chain.contains("did not answer")
        || chain.contains("connection refused")
        || chain.contains("os error 10061")
        || chain.contains("os error 111")
        || chain.contains("timed out")
        || chain.contains("unreachable")
        || chain.contains("handshake")
    {
        SlrStatus::CouldNotConnect
    } else {
        SlrStatus::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_is_zero_and_every_failure_is_negative() {
        // The render functions return either a frame count or a status in one
        // value, which only works if failures are negative.
        assert_eq!(SlrStatus::Ok as i32, 0);
        for status in [
            SlrStatus::InvalidArgument,
            SlrStatus::BadJoinCode,
            SlrStatus::IncompatibleVersion,
            SlrStatus::CouldNotConnect,
            SlrStatus::Refused,
            SlrStatus::AudioPortUnavailable,
            SlrStatus::Internal,
            SlrStatus::Panicked,
        ] {
            let code = status as i32;
            assert!(code < 0, "{status:?} must be negative");
        }
    }

    #[test]
    fn a_version_mismatch_is_not_reported_as_a_bad_code() {
        // Rescanning will not help, so telling somebody to try again would
        // waste their time.
        assert_eq!(
            from_payload(PayloadError::IncompatibleVersion {
                theirs: "2.0.0".to_owned(),
                ours: "1.0.0".to_owned(),
            }),
            SlrStatus::IncompatibleVersion
        );
        assert_eq!(
            from_payload(PayloadError::NotAJoinCode),
            SlrStatus::BadJoinCode
        );
        assert_eq!(
            from_payload(PayloadError::Missing("token")),
            SlrStatus::BadJoinCode
        );
    }

    #[test]
    fn a_refusal_is_distinguished_from_a_network_failure() {
        let refused = anyhow::anyhow!(
            "The desktop refused the join code. Ask for a fresh QR code."
        );
        assert_eq!(from_join(&refused), SlrStatus::Refused);

        let unreachable = anyhow::anyhow!("the desktop did not answer");
        assert_eq!(from_join(&unreachable), SlrStatus::CouldNotConnect);
    }

    #[test]
    fn a_port_clash_is_reported_as_a_port_problem() {
        let error = anyhow::anyhow!("os error 10048").context(
            "could not bind the audio socket at 0.0.0.0:45872",
        );
        assert_eq!(from_join(&error), SlrStatus::AudioPortUnavailable);
    }

    #[test]
    fn an_unrecognised_failure_falls_back_to_internal_rather_than_guessing() {
        let error = anyhow::anyhow!("something nobody anticipated");
        assert_eq!(from_join(&error), SlrStatus::Internal);
    }
}

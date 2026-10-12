use audio_core::AudioCoreError;
use thiserror::Error;

/// Capture-layer failures.
///
/// These map one-to-one onto the `AUDIO_*` codes in the control protocol so the
/// UI can react specifically — "your interface was unplugged" needs different
/// handling from "that sample rate was refused".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CaptureError {
    #[error("no audio capture backend on this platform")]
    UnsupportedPlatform,

    #[error("COM initialisation failed: {0}")]
    ComInit(String),

    #[error("audio device \"{0}\" was not found")]
    DeviceNotFound(String),

    #[error("the audio device was disconnected")]
    DeviceDisconnected,

    #[error("device is not available for capture (state: {0})")]
    DeviceUnavailable(&'static str),

    #[error("channel {requested} does not exist on this device (it has {available})")]
    ChannelOutOfRange { requested: u16, available: u16 },

    #[error("the driver refused the requested format: {0}")]
    FormatUnsupported(String),

    #[error("capture is already running")]
    AlreadyRunning,

    #[error("capture is not running")]
    NotRunning,

    #[error("failed to start capture: {0}")]
    StartFailed(String),

    #[error("WASAPI call failed: {0}")]
    Wasapi(String),

    #[error(transparent)]
    Core(#[from] AudioCoreError),
}

impl CaptureError {
    /// Whether retrying the same operation could plausibly succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::DeviceDisconnected | Self::DeviceUnavailable(_) | Self::StartFailed(_)
        )
    }

    /// Stable code string, used to build the protocol's structured errors.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform | Self::ComInit(_) | Self::Wasapi(_) => "AUDIO_CAPTURE_FAILED",
            Self::DeviceNotFound(_) => "AUDIO_DEVICE_NOT_FOUND",
            Self::DeviceDisconnected => "AUDIO_DEVICE_DISCONNECTED",
            Self::DeviceUnavailable(_) => "AUDIO_DEVICE_NOT_FOUND",
            Self::ChannelOutOfRange { .. } | Self::Core(_) => "AUDIO_CHANNEL_OUT_OF_RANGE",
            Self::FormatUnsupported(_) => "AUDIO_FORMAT_UNSUPPORTED",
            Self::AlreadyRunning | Self::NotRunning => "INVALID_STATE_TRANSITION",
            Self::StartFailed(_) => "AUDIO_CAPTURE_FAILED",
        }
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for CaptureError {
    fn from(error: windows::core::Error) -> Self {
        use windows::Win32::Media::Audio::AUDCLNT_E_DEVICE_INVALIDATED;

        if error.code() == AUDCLNT_E_DEVICE_INVALIDATED {
            Self::DeviceDisconnected
        } else {
            Self::Wasapi(format!("{} (0x{:08X})", error.message(), error.code().0))
        }
    }
}

pub type Result<T> = core::result::Result<T, CaptureError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnection_is_retryable_but_a_bad_channel_is_not() {
        assert!(CaptureError::DeviceDisconnected.is_retryable());
        assert!(
            !CaptureError::ChannelOutOfRange { requested: 4, available: 2 }.is_retryable(),
            "asking again for a channel that does not exist cannot help"
        );
    }

    #[test]
    fn codes_match_the_control_protocol() {
        assert_eq!(
            CaptureError::DeviceNotFound("x".into()).code(),
            "AUDIO_DEVICE_NOT_FOUND"
        );
        assert_eq!(
            CaptureError::DeviceDisconnected.code(),
            "AUDIO_DEVICE_DISCONNECTED"
        );
        assert_eq!(
            CaptureError::FormatUnsupported("48k".into()).code(),
            "AUDIO_FORMAT_UNSUPPORTED"
        );
    }
}

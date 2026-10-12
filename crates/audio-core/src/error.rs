use thiserror::Error;

/// Errors raised by the core audio primitives.
///
/// Intentionally small: anything that can fail inside a real-time callback must
/// be representable without allocating, so the variants carry only `Copy` data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AudioCoreError {
    #[error("unsupported sample format")]
    UnsupportedFormat,

    #[error("channel index {requested} is out of range for a {available}-channel stream")]
    ChannelOutOfRange { requested: u16, available: u16 },

    #[error("buffer length {len} is not a whole number of {channels}-channel frames")]
    RaggedBuffer { len: usize, channels: u16 },

    #[error("ring buffer capacity must be greater than zero")]
    ZeroCapacity,

    #[error("sample rate {0} Hz is not supported")]
    UnsupportedSampleRate(u32),
}

pub type Result<T> = core::result::Result<T, AudioCoreError>;

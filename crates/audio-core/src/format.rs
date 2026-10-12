use serde::{Deserialize, Serialize};

use crate::error::{AudioCoreError, Result};

/// Sample formats a Windows audio endpoint may deliver.
///
/// StageLink converts all of these to `f32` immediately on capture; `f32` is
/// the only format the DSP path sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    /// 32-bit IEEE float, the usual WASAPI shared-mode format.
    F32,
    /// 32-bit signed integer.
    I32,
    /// 24-bit signed integer packed into 3 bytes.
    I24,
    /// 16-bit signed integer.
    I16,
}

impl SampleFormat {
    /// Bytes occupied by one sample in the device buffer.
    #[inline]
    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I24 => 3,
            Self::I16 => 2,
        }
    }

    /// Maps a WASAPI `(wBitsPerSample, is_float)` pair to a format.
    pub fn from_bits(bits: u16, is_float: bool) -> Result<Self> {
        match (bits, is_float) {
            (32, true) => Ok(Self::F32),
            (32, false) => Ok(Self::I32),
            (24, false) => Ok(Self::I24),
            (16, false) => Ok(Self::I16),
            _ => Err(AudioCoreError::UnsupportedFormat),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::I32 => "i32",
            Self::I24 => "i24",
            Self::I16 => "i16",
        }
    }
}

/// WASAPI sharing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareMode {
    /// Shares the endpoint with other applications. Higher latency, always works.
    #[default]
    Shared,
    /// Takes exclusive control. Lower latency, fails if anything else holds the
    /// device, and bypasses the Windows mixer.
    Exclusive,
}

impl ShareMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Exclusive => "exclusive",
        }
    }
}

/// A concrete, negotiated capture format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    /// Channels the device stream delivers. StageLink extracts one of them.
    pub channels: u16,
    pub sample_format: SampleFormat,
}

impl AudioFormat {
    #[must_use]
    pub const fn new(sample_rate: u32, channels: u16, sample_format: SampleFormat) -> Self {
        Self { sample_rate, channels, sample_format }
    }

    /// Bytes per interleaved frame across all channels.
    #[inline]
    #[must_use]
    pub const fn bytes_per_frame(&self) -> usize {
        self.sample_format.bytes_per_sample() * self.channels as usize
    }

    /// Duration of `frames` frames at this rate, in microseconds.
    #[inline]
    #[must_use]
    pub const fn frames_to_micros(&self, frames: u64) -> u64 {
        frames.saturating_mul(1_000_000) / self.sample_rate as u64
    }

    /// Frames in `millis` milliseconds at this rate.
    #[inline]
    #[must_use]
    pub fn millis_to_frames(&self, millis: f64) -> usize {
        ((f64::from(self.sample_rate) * millis) / 1000.0).round() as usize
    }

    /// Validates that `channel` exists in this stream.
    pub fn validate_channel(&self, channel: u16) -> Result<()> {
        if channel < self.channels {
            Ok(())
        } else {
            Err(AudioCoreError::ChannelOutOfRange {
                requested: channel,
                available: self.channels,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_frame_accounts_for_channels() {
        let format = AudioFormat::new(48_000, 2, SampleFormat::F32);
        assert_eq!(format.bytes_per_frame(), 8);
        let packed = AudioFormat::new(48_000, 4, SampleFormat::I24);
        assert_eq!(packed.bytes_per_frame(), 12);
    }

    #[test]
    fn channel_validation_rejects_out_of_range() {
        let format = AudioFormat::new(48_000, 2, SampleFormat::F32);
        assert!(format.validate_channel(0).is_ok());
        assert!(format.validate_channel(1).is_ok());
        assert_eq!(
            format.validate_channel(2),
            Err(AudioCoreError::ChannelOutOfRange { requested: 2, available: 2 })
        );
    }

    #[test]
    fn frame_duration_maths() {
        let format = AudioFormat::new(48_000, 1, SampleFormat::F32);
        assert_eq!(format.frames_to_micros(48_000), 1_000_000);
        assert_eq!(format.millis_to_frames(5.0), 240);
    }

    #[test]
    fn wasapi_bit_depths_map_to_formats() {
        assert_eq!(SampleFormat::from_bits(32, true).unwrap(), SampleFormat::F32);
        assert_eq!(SampleFormat::from_bits(32, false).unwrap(), SampleFormat::I32);
        assert_eq!(SampleFormat::from_bits(24, false).unwrap(), SampleFormat::I24);
        assert_eq!(SampleFormat::from_bits(16, false).unwrap(), SampleFormat::I16);
        assert!(SampleFormat::from_bits(8, false).is_err());
        assert!(SampleFormat::from_bits(16, true).is_err());
    }
}

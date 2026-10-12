//! Sample-format conversion and mono channel extraction.
//!
//! All functions here are allocation-free and write into caller-provided
//! buffers, so they are safe to call from the capture callback.

use crate::error::{AudioCoreError, Result};
use crate::format::{AudioFormat, SampleFormat};

/// Extracts one channel from an interleaved device buffer into `out` as `f32`.
///
/// `raw` is the byte buffer exactly as WASAPI handed it over. `out` must have
/// room for `raw.len() / format.bytes_per_frame()` samples; the number written
/// is returned.
///
/// This is the single point where a multi-channel interface becomes StageLink's
/// mono monitor mix. Channels other than `channel` are discarded — they are
/// other people's monitor sends, not instruments to be mixed.
pub fn extract_mono_channel(
    raw: &[u8],
    format: &AudioFormat,
    channel: u16,
    out: &mut [f32],
) -> Result<usize> {
    format.validate_channel(channel)?;

    let bytes_per_frame = format.bytes_per_frame();
    if bytes_per_frame == 0 {
        return Err(AudioCoreError::UnsupportedFormat);
    }
    if !raw.len().is_multiple_of(bytes_per_frame) {
        return Err(AudioCoreError::RaggedBuffer {
            len: raw.len(),
            channels: format.channels,
        });
    }

    let frames = (raw.len() / bytes_per_frame).min(out.len());
    let stride = format.sample_format.bytes_per_sample();
    let offset = stride * channel as usize;

    for frame in 0..frames {
        let at = frame * bytes_per_frame + offset;
        // SAFETY-adjacent: `at + stride <= raw.len()` holds because `frames` is
        // bounded by `raw.len() / bytes_per_frame` and `offset + stride <=
        // bytes_per_frame` follows from the channel check above. We still index
        // through slices so a logic error is a panic, never UB.
        out[frame] = decode_sample(&raw[at..at + stride], format.sample_format);
    }

    Ok(frames)
}

/// Decodes a single sample from its little-endian device representation.
#[inline]
fn decode_sample(bytes: &[u8], format: SampleFormat) -> f32 {
    match format {
        SampleFormat::F32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        SampleFormat::I32 => {
            let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            // Divide by 2^31 so -2^31 maps to exactly -1.0.
            value as f32 / 2_147_483_648.0
        }
        SampleFormat::I24 => {
            // Sign-extend 24 bits into an i32 by placing the sample in the high
            // three bytes and shifting back down.
            let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
            value as f32 / 8_388_608.0
        }
        SampleFormat::I16 => {
            let value = i16::from_le_bytes([bytes[0], bytes[1]]);
            value as f32 / 32_768.0
        }
    }
}

/// Converts `f32` samples to little-endian PCM S16 bytes, the network format.
///
/// Values outside `-1.0..=1.0` are hard-clamped rather than wrapped: a wrapped
/// sample is a loud click in somebody's ears.
///
/// `out` must be at least `samples.len() * 2` bytes. Returns bytes written.
pub fn f32_to_pcm_s16le(samples: &[f32], out: &mut [u8]) -> usize {
    let count = samples.len().min(out.len() / 2);
    for (index, &sample) in samples.iter().take(count).enumerate() {
        let bytes = f32_to_i16(sample).to_le_bytes();
        out[index * 2] = bytes[0];
        out[index * 2 + 1] = bytes[1];
    }
    count * 2
}

/// Scales and clamps one `f32` sample to `i16`.
#[inline]
#[must_use]
pub fn f32_to_i16(sample: f32) -> i16 {
    // NaN becomes silence rather than an arbitrary value.
    if sample.is_nan() {
        return 0;
    }
    // Scale by 32767 so +1.0 maps to i16::MAX and -1.0 to -32767, keeping the
    // conversion symmetric and avoiding a wrap at positive full scale.
    let scaled = sample.clamp(-1.0, 1.0) * 32_767.0;
    scaled.round() as i16
}

/// Decodes little-endian PCM S16 bytes back to `f32`. Used by the test receiver
/// and by the protocol round-trip tests.
pub fn pcm_s16le_to_f32(raw: &[u8], out: &mut [f32]) -> usize {
    let count = (raw.len() / 2).min(out.len());
    for index in 0..count {
        let value = i16::from_le_bytes([raw[index * 2], raw[index * 2 + 1]]);
        out[index] = f32::from(value) / 32_767.0;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_requested_channel_from_interleaved_f32() {
        // Two frames of stereo: L=0.25/0.5, R=-0.25/-0.5
        let mut raw = Vec::new();
        for sample in [0.25_f32, -0.25, 0.5, -0.5] {
            raw.extend_from_slice(&sample.to_le_bytes());
        }
        let format = AudioFormat::new(48_000, 2, SampleFormat::F32);

        let mut left = [0.0_f32; 2];
        assert_eq!(extract_mono_channel(&raw, &format, 0, &mut left).unwrap(), 2);
        assert_eq!(left, [0.25, 0.5]);

        let mut right = [0.0_f32; 2];
        assert_eq!(extract_mono_channel(&raw, &format, 1, &mut right).unwrap(), 2);
        assert_eq!(right, [-0.25, -0.5]);
    }

    #[test]
    fn rejects_out_of_range_channel() {
        let format = AudioFormat::new(48_000, 2, SampleFormat::F32);
        let mut out = [0.0_f32; 4];
        assert!(extract_mono_channel(&[0; 16], &format, 7, &mut out).is_err());
    }

    #[test]
    fn rejects_a_partial_frame() {
        let format = AudioFormat::new(48_000, 2, SampleFormat::F32);
        let mut out = [0.0_f32; 4];
        // 12 bytes is 1.5 stereo f32 frames.
        assert!(matches!(
            extract_mono_channel(&[0; 12], &format, 0, &mut out),
            Err(AudioCoreError::RaggedBuffer { .. })
        ));
    }

    #[test]
    fn extracts_i16_and_i24_channels() {
        // Stereo i16, one frame: L = 16384 (0.5), R = -16384 (-0.5)
        let raw: Vec<u8> = [16_384_i16, -16_384]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let format = AudioFormat::new(48_000, 2, SampleFormat::I16);
        let mut out = [0.0_f32; 1];
        extract_mono_channel(&raw, &format, 0, &mut out).unwrap();
        assert!((out[0] - 0.5).abs() < 1e-4);

        // Mono i24 at half scale: 0x400000 == 4194304 == 0.5
        let raw24 = [0x00_u8, 0x00, 0x40];
        let format24 = AudioFormat::new(48_000, 1, SampleFormat::I24);
        let mut out24 = [0.0_f32; 1];
        extract_mono_channel(&raw24, &format24, 0, &mut out24).unwrap();
        assert!((out24[0] - 0.5).abs() < 1e-4, "got {}", out24[0]);

        // And the negative side sign-extends correctly: 0xC00000 == -0.5
        let neg24 = [0x00_u8, 0x00, 0xC0];
        extract_mono_channel(&neg24, &format24, 0, &mut out24).unwrap();
        assert!((out24[0] + 0.5).abs() < 1e-4, "got {}", out24[0]);
    }

    #[test]
    fn s16_conversion_clamps_instead_of_wrapping() {
        assert_eq!(f32_to_i16(1.0), 32_767);
        assert_eq!(f32_to_i16(-1.0), -32_767);
        assert_eq!(f32_to_i16(4.2), 32_767);
        assert_eq!(f32_to_i16(-4.2), -32_767);
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(f32::NAN), 0);
    }

    #[test]
    fn pcm_round_trip_preserves_signal_within_quantisation_error() {
        let input = [0.0_f32, 0.5, -0.5, 0.999, -0.999, 0.123_45];
        let mut encoded = vec![0_u8; input.len() * 2];
        assert_eq!(f32_to_pcm_s16le(&input, &mut encoded), input.len() * 2);

        let mut decoded = vec![0.0_f32; input.len()];
        assert_eq!(pcm_s16le_to_f32(&encoded, &mut decoded), input.len());

        for (original, round_tripped) in input.iter().zip(decoded.iter()) {
            // One LSB of 16-bit is ~3.05e-5; allow a touch more for rounding.
            assert!(
                (original - round_tripped).abs() < 4e-5,
                "{original} -> {round_tripped}"
            );
        }
    }

    #[test]
    fn conversion_respects_short_output_buffers() {
        let input = [0.1_f32; 8];
        let mut out = [0_u8; 6]; // room for 3 samples only
        assert_eq!(f32_to_pcm_s16le(&input, &mut out), 6);
    }
}

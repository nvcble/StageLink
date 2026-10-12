//! Core audio primitives for StageLink.
//!
//! Everything in this crate is usable from a real-time audio callback:
//!
//! * no allocation after construction,
//! * no locks that a non-real-time thread can hold,
//! * no I/O, no logging, no system calls on the hot path.
//!
//! StageLink carries exactly one mono signal — the FOH monitor send — so the
//! types here are deliberately mono-centric. There is no mixer, no per-channel
//! gain and no panning, because the input contains no separable sources.

pub mod convert;
pub mod error;
pub mod format;
pub mod levels;
pub mod ring;

pub use error::{AudioCoreError, Result};
pub use format::{AudioFormat, SampleFormat, ShareMode};
pub use levels::{LevelAccumulator, LevelSnapshot, PeakHold};
pub use ring::{AudioRing, RingConsumer, RingProducer, RingStats};

/// StageLink's canonical sample rate. Everything negotiates towards this.
pub const CANONICAL_SAMPLE_RATE: u32 = 48_000;

/// Channel count on the wire and through the DSP path. Always mono.
pub const CHANNELS: u16 = 1;

/// Linear amplitude at or above which a sample counts as clipped.
///
/// Full scale rather than a hair below: a float capture path can legitimately
/// exceed 1.0, and anything that reaches 1.0 will clip once converted to S16.
pub const CLIP_THRESHOLD: f32 = 1.0;

/// dBFS value reported for digital silence. Serialised as `null` upstream.
pub const SILENCE_DBFS: f32 = f32::NEG_INFINITY;

/// Converts a linear amplitude (0.0..=1.0) to dBFS.
///
/// Returns [`SILENCE_DBFS`] for zero so callers can distinguish true silence
/// from a very quiet signal instead of rendering `-inf` as a huge negative.
#[inline]
#[must_use]
pub fn linear_to_dbfs(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        SILENCE_DBFS
    } else {
        20.0 * amplitude.log10()
    }
}

/// Converts dBFS to linear amplitude.
#[inline]
#[must_use]
pub fn dbfs_to_linear(db: f32) -> f32 {
    if db == f32::NEG_INFINITY {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

/// Number of mono frames in one packet for a rate and frame duration.
///
/// Mirrors `framesPerPacket` in `@stagelink/shared-protocol`; the protocol test
/// suite asserts the two agree.
#[inline]
#[must_use]
pub fn frames_per_packet(sample_rate: u32, frame_duration_ms: f64) -> usize {
    ((f64::from(sample_rate) * frame_duration_ms) / 1000.0).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_is_zero_dbfs() {
        assert!((linear_to_dbfs(1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn half_amplitude_is_about_minus_six_db() {
        assert!((linear_to_dbfs(0.5) + 6.0206).abs() < 1e-3);
    }

    #[test]
    fn silence_is_negative_infinity() {
        assert_eq!(linear_to_dbfs(0.0), f32::NEG_INFINITY);
        assert_eq!(linear_to_dbfs(-0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn db_round_trips_through_linear() {
        for db in [-60.0_f32, -24.0, -12.0, -3.0, 0.0] {
            let round_tripped = linear_to_dbfs(dbfs_to_linear(db));
            assert!((round_tripped - db).abs() < 1e-3, "db={db} got={round_tripped}");
        }
    }

    #[test]
    fn five_ms_at_48k_is_240_frames() {
        assert_eq!(frames_per_packet(48_000, 5.0), 240);
        assert_eq!(frames_per_packet(48_000, 2.5), 120);
        assert_eq!(frames_per_packet(48_000, 10.0), 480);
        assert_eq!(frames_per_packet(44_100, 5.0), 221);
    }
}

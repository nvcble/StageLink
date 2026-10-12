//! Level metering.
//!
//! The capture thread must never block to publish a meter reading, and the UI
//! must never be able to slow the capture thread down. The split here is:
//!
//! * [`LevelAccumulator`] is written by the capture thread with a handful of
//!   lock-free atomic operations per buffer (not per sample).
//! * A separate, ordinary thread calls [`LevelAccumulator::drain`] on a timer
//!   (25 Hz) and turns the accumulated figures into an event for the UI.
//!
//! Nothing here is ever derived from a simulation; if there is no signal the
//! meter reads silence.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::{CLIP_THRESHOLD, SILENCE_DBFS, linear_to_dbfs};

/// Lock-free meter accumulator shared between the capture thread and the
/// telemetry thread.
///
/// Peak is stored as the bit pattern of a non-negative `f32`; for non-negative
/// floats the IEEE-754 bit pattern orders identically to the numeric value, so
/// a compare-and-swap maximum on the bits is a correct maximum on the values.
#[derive(Debug, Default)]
pub struct LevelAccumulator {
    /// Bits of the running absolute peak (non-negative `f32`).
    peak_bits: AtomicU32,
    /// Bits of the running sum of squares, as an `f64`.
    sum_squares_bits: AtomicU64,
    /// Frames accumulated since the last drain.
    frames: AtomicU64,
    /// Samples at or beyond full scale since the last drain.
    window_clips: AtomicU64,
    /// Samples at or beyond full scale since capture started.
    total_clips: AtomicU64,
}

impl LevelAccumulator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulates one captured buffer.
    ///
    /// Real-time safe: the per-sample work is pure arithmetic in registers, and
    /// only four atomic read-modify-writes happen per call regardless of buffer
    /// size.
    pub fn push(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let mut peak = 0.0_f32;
        let mut sum_squares = 0.0_f64;
        let mut clips = 0_u64;

        for &sample in samples {
            // NaN would poison both peak and RMS; treat it as silence. A driver
            // producing NaN is a fault we surface via xruns, not via the meter.
            let value = if sample.is_nan() { 0.0 } else { sample };
            let magnitude = value.abs();
            if magnitude > peak {
                peak = magnitude;
            }
            if magnitude >= CLIP_THRESHOLD {
                clips += 1;
            }
            sum_squares += f64::from(value) * f64::from(value);
        }

        self.fetch_max_peak(peak);
        self.fetch_add_sum_squares(sum_squares);
        self.frames.fetch_add(samples.len() as u64, Ordering::Relaxed);
        if clips > 0 {
            self.window_clips.fetch_add(clips, Ordering::Relaxed);
            self.total_clips.fetch_add(clips, Ordering::Relaxed);
        }
    }

    /// Lock-free `max` on the stored peak.
    fn fetch_max_peak(&self, candidate: f32) {
        let candidate_bits = candidate.to_bits();
        let mut current = self.peak_bits.load(Ordering::Relaxed);
        while candidate_bits > current {
            match self.peak_bits.compare_exchange_weak(
                current,
                candidate_bits,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// Lock-free addition on an `f64` stored as bits.
    fn fetch_add_sum_squares(&self, delta: f64) {
        let mut current = self.sum_squares_bits.load(Ordering::Relaxed);
        loop {
            let updated = (f64::from_bits(current) + delta).to_bits();
            match self.sum_squares_bits.compare_exchange_weak(
                current,
                updated,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// Takes everything accumulated since the previous call and resets the
    /// window counters. `total_clips` keeps counting for the session.
    ///
    /// Returns `None` when no frames arrived — the caller should then report a
    /// silence/stalled window rather than a fabricated level.
    pub fn drain(&self) -> Option<LevelWindow> {
        let frames = self.frames.swap(0, Ordering::Relaxed);
        let peak_bits = self.peak_bits.swap(0, Ordering::Relaxed);
        let sum_squares = f64::from_bits(self.sum_squares_bits.swap(0, Ordering::Relaxed));
        let window_clips = self.window_clips.swap(0, Ordering::Relaxed);
        let total_clips = self.total_clips.load(Ordering::Relaxed);

        if frames == 0 {
            return None;
        }

        let peak = f32::from_bits(peak_bits);
        let rms = (sum_squares / frames as f64).sqrt() as f32;

        Some(LevelWindow {
            frames,
            peak,
            rms,
            window_clips,
            total_clips,
        })
    }

    /// Clears every counter, including the session clip total. Called when a
    /// new capture session opens.
    pub fn reset(&self) {
        self.peak_bits.store(0, Ordering::Relaxed);
        self.sum_squares_bits.store(0, Ordering::Relaxed);
        self.frames.store(0, Ordering::Relaxed);
        self.window_clips.store(0, Ordering::Relaxed);
        self.total_clips.store(0, Ordering::Relaxed);
    }

    /// Session clip total without disturbing the window.
    #[must_use]
    pub fn total_clips(&self) -> u64 {
        self.total_clips.load(Ordering::Relaxed)
    }
}

/// Raw accumulated figures for one reporting window, in linear amplitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelWindow {
    pub frames: u64,
    pub peak: f32,
    pub rms: f32,
    pub window_clips: u64,
    pub total_clips: u64,
}

/// Decaying peak-hold, owned by the (non-real-time) telemetry thread.
#[derive(Debug, Clone, Copy)]
pub struct PeakHold {
    held: f32,
    decay_db_per_sec: f32,
}

impl PeakHold {
    #[must_use]
    pub const fn new(decay_db_per_sec: f32) -> Self {
        Self { held: 0.0, decay_db_per_sec }
    }

    /// Applies `elapsed_secs` of decay, then raises the hold to `peak` if the
    /// new peak is higher. Returns the current held value, linear.
    pub fn update(&mut self, peak: f32, elapsed_secs: f32) -> f32 {
        if self.held > 0.0 {
            let decayed_db = linear_to_dbfs(self.held) - self.decay_db_per_sec * elapsed_secs;
            self.held = if decayed_db <= -120.0 {
                0.0
            } else {
                crate::dbfs_to_linear(decayed_db)
            };
        }
        if peak > self.held {
            self.held = peak;
        }
        self.held
    }

    pub fn reset(&mut self) {
        self.held = 0.0;
    }
}

/// A meter reading ready to be serialised to the UI.
///
/// Level fields are `Option<f32>` in dBFS: `None` means digital silence, which
/// serialises as JSON `null` rather than a misleading `-inf` or a floor value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LevelSnapshot {
    pub rms_db: Option<f32>,
    pub peak_db: Option<f32>,
    pub peak_hold_db: Option<f32>,
    pub clipping: bool,
    pub clip_count: u64,
    pub window_frames: u64,
}

impl LevelSnapshot {
    /// Builds a UI snapshot from a raw window plus the caller's peak-hold state.
    #[must_use]
    pub fn from_window(window: LevelWindow, hold: &mut PeakHold, elapsed_secs: f32) -> Self {
        let held = hold.update(window.peak, elapsed_secs);
        Self {
            rms_db: finite_db(window.rms),
            peak_db: finite_db(window.peak),
            peak_hold_db: finite_db(held),
            clipping: window.window_clips > 0,
            clip_count: window.total_clips,
            window_frames: window.frames,
        }
    }

    /// The reading to publish when a window produced no frames at all: the
    /// capture path has stalled, so report silence and let the peak hold decay
    /// rather than repeating the last real value.
    #[must_use]
    pub fn stalled(hold: &mut PeakHold, elapsed_secs: f32, clip_count: u64) -> Self {
        let held = hold.update(0.0, elapsed_secs);
        Self {
            rms_db: None,
            peak_db: None,
            peak_hold_db: finite_db(held),
            clipping: false,
            clip_count,
            window_frames: 0,
        }
    }
}

/// Converts linear amplitude to dBFS, mapping silence to `None`.
#[inline]
fn finite_db(amplitude: f32) -> Option<f32> {
    let db = linear_to_dbfs(amplitude);
    if db == SILENCE_DBFS || !db.is_finite() { None } else { Some(db) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_accumulator_drains_to_none() {
        let accumulator = LevelAccumulator::new();
        assert!(accumulator.drain().is_none());
    }

    #[test]
    fn peak_and_rms_match_a_known_signal() {
        let accumulator = LevelAccumulator::new();
        // Alternating +/-0.5 has peak 0.5 and RMS 0.5.
        let samples: Vec<f32> = (0..1_000)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        accumulator.push(&samples);

        let window = accumulator.drain().expect("frames were pushed");
        assert_eq!(window.frames, 1_000);
        assert!((window.peak - 0.5).abs() < 1e-6);
        assert!((window.rms - 0.5).abs() < 1e-6);
        assert_eq!(window.window_clips, 0);
    }

    #[test]
    fn silence_reads_as_silence_not_as_a_floor_value() {
        let accumulator = LevelAccumulator::new();
        accumulator.push(&[0.0; 256]);
        let window = accumulator.drain().unwrap();
        let mut hold = PeakHold::new(20.0);
        let snapshot = LevelSnapshot::from_window(window, &mut hold, 0.04);
        assert_eq!(snapshot.rms_db, None);
        assert_eq!(snapshot.peak_db, None);
        assert!(!snapshot.clipping);
    }

    #[test]
    fn clipping_is_counted_and_persists_for_the_session() {
        let accumulator = LevelAccumulator::new();
        accumulator.push(&[0.1, 1.0, -1.4, 0.2]);
        let window = accumulator.drain().unwrap();
        assert_eq!(window.window_clips, 2);
        assert_eq!(window.total_clips, 2);

        // A later quiet window no longer reports clipping, but the session
        // total stays — the operator needs to know it happened.
        accumulator.push(&[0.1; 4]);
        let next = accumulator.drain().unwrap();
        assert_eq!(next.window_clips, 0);
        assert_eq!(next.total_clips, 2);
    }

    #[test]
    fn nan_samples_do_not_poison_the_meter() {
        let accumulator = LevelAccumulator::new();
        accumulator.push(&[f32::NAN, 0.25, f32::NAN]);
        let window = accumulator.drain().unwrap();
        assert!((window.peak - 0.25).abs() < 1e-6);
        assert!(window.rms.is_finite());
    }

    #[test]
    fn drain_resets_the_window() {
        let accumulator = LevelAccumulator::new();
        accumulator.push(&[0.8; 64]);
        assert!(accumulator.drain().is_some());
        assert!(accumulator.drain().is_none(), "second drain should see no frames");
    }

    #[test]
    fn peak_hold_decays_over_time_and_resets_on_a_louder_peak() {
        let mut hold = PeakHold::new(20.0);
        // Hold at full scale, then let a second of decay pass: -20 dB => 0.1.
        assert!((hold.update(1.0, 0.0) - 1.0).abs() < 1e-6);
        let decayed = hold.update(0.0, 1.0);
        assert!((decayed - 0.1).abs() < 1e-3, "got {decayed}");
        // A louder peak lifts the hold immediately.
        assert!((hold.update(0.7, 0.04) - 0.7).abs() < 1e-3);
    }

    #[test]
    fn stalled_window_reports_null_levels() {
        let mut hold = PeakHold::new(20.0);
        hold.update(1.0, 0.0);
        let snapshot = LevelSnapshot::stalled(&mut hold, 0.04, 7);
        assert_eq!(snapshot.rms_db, None);
        assert_eq!(snapshot.peak_db, None);
        assert_eq!(snapshot.window_frames, 0);
        assert_eq!(snapshot.clip_count, 7);
        // The hold is still decaying rather than snapping to silence.
        assert!(snapshot.peak_hold_db.is_some());
    }

    #[test]
    fn concurrent_pushes_accumulate_every_frame() {
        use std::sync::Arc;
        use std::thread;

        let accumulator = Arc::new(LevelAccumulator::new());
        let mut handles = Vec::new();
        for _ in 0..4 {
            let accumulator = Arc::clone(&accumulator);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    accumulator.push(&[0.5; 16]);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let window = accumulator.drain().unwrap();
        assert_eq!(window.frames, 4 * 100 * 16);
        assert!((window.rms - 0.5).abs() < 1e-4);
    }
}

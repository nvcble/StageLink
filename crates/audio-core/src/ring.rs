//! Lock-free single-producer / single-consumer ring buffer for mono `f32`.
//!
//! This is the hand-off point between the WASAPI capture callback (producer)
//! and the packetiser (consumer). The capacity is fixed at construction, so the
//! producer never allocates and never blocks. When the consumer falls behind,
//! the producer *drops* the oldest-arriving samples it cannot fit and increments
//! an overrun counter — it must never wait, because waiting in the capture
//! callback means a glitch in every performer's ears.

use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

use crate::error::{AudioCoreError, Result};

/// Counters describing how the ring has behaved. Shared by both ends.
#[derive(Debug, Default)]
pub struct RingStats {
    /// Samples the producer had to discard because the ring was full.
    overrun_samples: AtomicU64,
    /// Times the producer hit a full ring (distinct from sample count).
    overrun_events: AtomicU64,
    /// Times the consumer asked for data and got less than it wanted.
    underrun_events: AtomicU64,
    /// Total samples successfully written.
    written: AtomicU64,
    /// Total samples successfully read.
    read: AtomicU64,
}

impl RingStats {
    #[must_use]
    pub fn overrun_samples(&self) -> u64 {
        self.overrun_samples.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn overrun_events(&self) -> u64 {
        self.overrun_events.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn underrun_events(&self) -> u64 {
        self.underrun_events.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.overrun_samples.store(0, Ordering::Relaxed);
        self.overrun_events.store(0, Ordering::Relaxed);
        self.underrun_events.store(0, Ordering::Relaxed);
        self.written.store(0, Ordering::Relaxed);
        self.read.store(0, Ordering::Relaxed);
    }
}

/// Factory that builds a ring sized in milliseconds of audio.
pub struct AudioRing;

impl AudioRing {
    /// Builds a ring holding `capacity_samples` mono samples.
    pub fn with_capacity(capacity_samples: usize) -> Result<(RingProducer, RingConsumer)> {
        if capacity_samples == 0 {
            return Err(AudioCoreError::ZeroCapacity);
        }
        let stats = Arc::new(RingStats::default());
        let (producer, consumer) = HeapRb::<f32>::new(capacity_samples).split();
        Ok((
            RingProducer { inner: producer, stats: Arc::clone(&stats) },
            RingConsumer { inner: consumer, stats },
        ))
    }

    /// Builds a ring sized to hold `millis` of audio at `sample_rate`.
    ///
    /// Pick this generously relative to the packetiser's wake-up interval: a
    /// ring that is too small turns a momentary scheduling hiccup into an
    /// audible dropout, and a few hundred milliseconds of `f32` is only a few
    /// tens of kilobytes.
    pub fn with_duration(
        sample_rate: u32,
        millis: f64,
    ) -> Result<(RingProducer, RingConsumer)> {
        if sample_rate == 0 {
            return Err(AudioCoreError::UnsupportedSampleRate(sample_rate));
        }
        let samples = ((f64::from(sample_rate) * millis) / 1000.0).ceil() as usize;
        Self::with_capacity(samples.max(1))
    }
}

/// Producer end. Owned by the capture thread.
pub struct RingProducer {
    inner: HeapProd<f32>,
    stats: Arc<RingStats>,
}

impl RingProducer {
    /// Writes as many samples as fit. Returns the number written.
    ///
    /// Real-time safe: no allocation, no locking, no blocking. Anything that
    /// does not fit is dropped and counted as an overrun — dropping audio is
    /// bad, but stalling the driver callback is worse.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let written = self.inner.push_slice(samples);
        self.stats.written.fetch_add(written as u64, Ordering::Relaxed);
        if written < samples.len() {
            let dropped = (samples.len() - written) as u64;
            self.stats.overrun_samples.fetch_add(dropped, Ordering::Relaxed);
            self.stats.overrun_events.fetch_add(1, Ordering::Relaxed);
        }
        written
    }

    /// Free space in samples.
    #[must_use]
    pub fn vacant(&self) -> usize {
        self.inner.vacant_len()
    }

    #[must_use]
    pub fn stats(&self) -> Arc<RingStats> {
        Arc::clone(&self.stats)
    }
}

/// Consumer end. Owned by the packetiser.
pub struct RingConsumer {
    inner: HeapCons<f32>,
    stats: Arc<RingStats>,
}

impl RingConsumer {
    /// Reads up to `out.len()` samples. Returns the number read.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        let read = self.inner.pop_slice(out);
        self.stats.read.fetch_add(read as u64, Ordering::Relaxed);
        if read < out.len() {
            self.stats.underrun_events.fetch_add(1, Ordering::Relaxed);
        }
        read
    }

    /// Reads exactly `out.len()` samples, or nothing at all.
    ///
    /// The packetiser uses this so it never emits a half-filled packet: a short
    /// packet would be interpreted by receivers as a timing discontinuity.
    pub fn pop_exact(&mut self, out: &mut [f32]) -> bool {
        if self.inner.occupied_len() < out.len() {
            self.stats.underrun_events.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let read = self.inner.pop_slice(out);
        self.stats.read.fetch_add(read as u64, Ordering::Relaxed);
        read == out.len()
    }

    /// Samples currently buffered.
    #[must_use]
    pub fn occupied(&self) -> usize {
        self.inner.occupied_len()
    }

    /// Buffered audio expressed as milliseconds at `sample_rate`.
    #[must_use]
    pub fn occupied_millis(&self, sample_rate: u32) -> f64 {
        if sample_rate == 0 {
            return 0.0;
        }
        (self.occupied() as f64 * 1000.0) / f64::from(sample_rate)
    }

    /// Discards everything buffered. Used when restarting a stream so stale
    /// audio is not sent out ahead of the live signal.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    #[must_use]
    pub fn stats(&self) -> Arc<RingStats> {
        Arc::clone(&self.stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_capacity() {
        assert_eq!(
            AudioRing::with_capacity(0).err(),
            Some(AudioCoreError::ZeroCapacity)
        );
    }

    #[test]
    fn duration_sizing_matches_the_sample_rate() {
        let (producer, _consumer) = AudioRing::with_duration(48_000, 100.0).unwrap();
        // 100 ms at 48 kHz is 4800 samples.
        assert_eq!(producer.vacant(), 4_800);
    }

    #[test]
    fn round_trips_samples_in_order() {
        let (mut producer, mut consumer) = AudioRing::with_capacity(16).unwrap();
        assert_eq!(producer.push(&[1.0, 2.0, 3.0]), 3);

        let mut out = [0.0_f32; 3];
        assert_eq!(consumer.pop(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn producer_drops_rather_than_blocking_when_full() {
        let (mut producer, consumer) = AudioRing::with_capacity(4).unwrap();
        assert_eq!(producer.push(&[1.0, 2.0, 3.0, 4.0]), 4);

        // The ring is full; this push must return immediately having written 0.
        assert_eq!(producer.push(&[5.0, 6.0]), 0);

        let stats = consumer.stats();
        assert_eq!(stats.overrun_samples(), 2);
        assert_eq!(stats.overrun_events(), 1);
        assert_eq!(stats.written(), 4);
    }

    #[test]
    fn partial_push_counts_only_the_dropped_tail() {
        let (mut producer, consumer) = AudioRing::with_capacity(4).unwrap();
        assert_eq!(producer.push(&[1.0, 2.0]), 2);
        // Room for 2 more, offered 5.
        assert_eq!(producer.push(&[3.0, 4.0, 5.0, 6.0, 7.0]), 2);
        assert_eq!(consumer.stats().overrun_samples(), 3);
    }

    #[test]
    fn pop_exact_refuses_a_short_read() {
        let (mut producer, mut consumer) = AudioRing::with_capacity(16).unwrap();
        producer.push(&[1.0, 2.0, 3.0]);

        let mut packet = [0.0_f32; 4];
        assert!(!consumer.pop_exact(&mut packet), "must not emit a short packet");
        // Nothing was consumed, so the samples are still available.
        assert_eq!(consumer.occupied(), 3);
        assert_eq!(packet, [0.0; 4]);

        producer.push(&[4.0]);
        assert!(consumer.pop_exact(&mut packet));
        assert_eq!(packet, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(consumer.occupied(), 0);
    }

    #[test]
    fn reports_buffered_duration() {
        let (mut producer, consumer) = AudioRing::with_capacity(4_800).unwrap();
        producer.push(&[0.0; 480]);
        // 480 samples at 48 kHz is 10 ms.
        assert!((consumer.occupied_millis(48_000) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn clear_discards_stale_audio() {
        let (mut producer, mut consumer) = AudioRing::with_capacity(16).unwrap();
        producer.push(&[1.0; 8]);
        consumer.clear();
        assert_eq!(consumer.occupied(), 0);
    }

    #[test]
    fn survives_a_concurrent_producer_and_consumer() {
        use std::thread;

        const TOTAL: usize = 100_000;
        let (mut producer, mut consumer) = AudioRing::with_capacity(1_024).unwrap();

        let writer = thread::spawn(move || {
            let mut sent = 0_usize;
            let chunk = [0.5_f32; 64];
            while sent < TOTAL {
                // A real capture callback never retries, but this test is about
                // the ring's integrity under contention, not its drop policy.
                let written = producer.push(&chunk);
                sent += written;
                if written == 0 {
                    std::hint::spin_loop();
                }
            }
        });

        let mut received = 0_usize;
        let mut buffer = [0.0_f32; 64];
        while received < TOTAL {
            let read = consumer.pop(&mut buffer);
            for sample in buffer.iter().take(read) {
                assert_eq!(*sample, 0.5, "ring corrupted a sample");
            }
            received += read;
        }
        writer.join().unwrap();
        assert!(received >= TOTAL);
    }
}

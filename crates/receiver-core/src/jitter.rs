//! The jitter buffer: sequence-indexed slots, written by the network thread and
//! read by the audio callback.
//!
//! ## Why not `audio_core::AudioRing`
//!
//! The capture path's ring is a FIFO, and a jitter buffer is not a FIFO. It
//! needs **random access by sequence number**, because packets arrive out of
//! order and a late packet has to land in its own place rather than at the end.
//! A FIFO would play it in the wrong position, which sounds worse than losing
//! it. Everything else in `audio-core` and `audio-protocol` is reused.
//!
//! ## Structure
//!
//! 256 slots, indexed `sequence & 255`. At the default 5 ms packet that is
//! 1.28 s of addressable space against a buffer that never exceeds 120 ms, so a
//! slot is never rewritten while the reader could still want it. Duplicate
//! rejection falls out for free: a slot already holding that sequence is simply
//! not written again.
//!
//! ## Why it is safe without `unsafe`
//!
//! Samples live in `AtomicU32` holding `f32` bit patterns, so a concurrent
//! read and write of the same slot is well-defined rather than a data race.
//! That turns a soundness problem into a correctness one, which a seqlock then
//! solves: the writer makes the slot's version odd before touching it and even
//! again afterwards, and the reader discards anything it read across a version
//! change. The alternative — `UnsafeCell` plus an argument that 256 slots is
//! "enough" — would be undefined behaviour whenever the argument failed, and
//! would fail on exactly the overloaded network this code exists to survive.
//!
//! The per-sample atomics compile to plain loads and stores on x86-64 and
//! AArch64; there is no contention because only one thread writes a slot.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Addressable slots. A power of two so the index is a mask.
pub const SLOTS: usize = 256;

/// What [`JitterBuffer::insert`] did with a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insert {
    /// Stored, and the buffer had not seen this sequence before.
    Stored,
    /// Already held. Counted, not stored again.
    Duplicate,
    /// Its slot holds a sequence the reader has not reached yet, so storing it
    /// would destroy audio that is still needed. The packet is too late to
    /// matter — more than `SLOTS` packets behind.
    TooLate,
    /// The packet belongs to a different session. The caller resets first.
    WrongSession,
    /// The packet's frame count does not match this buffer's geometry.
    WrongSize,
}

/// A packet handed to the audio callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ready {
    pub flags: u8,
    /// Arrival time, as the network thread recorded it.
    pub arrived_us: u64,
}

/// Slot state. One writer (network), one reader (audio callback).
struct Slot {
    /// Seqlock. Odd while being written; incremented twice per write.
    version: AtomicU32,
    /// Which reset generation this content belongs to. Lets a session change
    /// invalidate all 256 slots in one store rather than walking them.
    generation: AtomicU32,
    sequence: AtomicU32,
    flags: AtomicU32,
    arrived_us: AtomicU64,
    /// `f32` bit patterns.
    samples: Box<[AtomicU32]>,
}

impl Slot {
    fn new(frames: usize) -> Self {
        Self {
            version: AtomicU32::new(0),
            generation: AtomicU32::new(u32::MAX),
            sequence: AtomicU32::new(0),
            flags: AtomicU32::new(0),
            arrived_us: AtomicU64::new(0),
            samples: (0..frames).map(|_| AtomicU32::new(0)).collect(),
        }
    }
}

/// Sequence-indexed packet store.
///
/// `&self` on both sides on purpose: the network thread and the audio callback
/// hold the same `&JitterBuffer`, with no lock between them. A mutex here would
/// let a network thread stall the audio callback, which is the one thing a
/// real-time path must never allow.
pub struct JitterBuffer {
    slots: Box<[Slot]>,
    frames_per_packet: usize,
    /// Bumped on reset. Content from an earlier generation is ignored.
    generation: AtomicU32,
    duplicates: AtomicU64,
    too_late: AtomicU64,
    stored: AtomicU64,
}

impl JitterBuffer {
    #[must_use]
    pub fn new(frames_per_packet: usize) -> Self {
        Self {
            slots: (0..SLOTS).map(|_| Slot::new(frames_per_packet)).collect(),
            frames_per_packet,
            generation: AtomicU32::new(0),
            duplicates: AtomicU64::new(0),
            too_late: AtomicU64::new(0),
            stored: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub const fn frames_per_packet(&self) -> usize {
        self.frames_per_packet
    }

    /// Invalidates every slot.
    ///
    /// Called on a session change, a `DISCONTINUITY`, or a gap too long to
    /// conceal. One store, so it is safe to call from the network thread
    /// without stalling the reader — the reader simply stops matching.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    #[must_use]
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    fn slot_for(&self, sequence: u32) -> &Slot {
        // SLOTS is a power of two, so this is a mask rather than a division.
        &self.slots[sequence as usize % SLOTS]
    }

    /// Stores one packet's audio. Called from the network thread.
    ///
    /// `reader_at` is the sequence the audio callback will play next; a packet
    /// whose slot still holds audio at or after it must not overwrite it.
    pub fn insert(
        &self,
        sequence: u32,
        flags: u8,
        arrived_us: u64,
        samples: &[f32],
        reader_at: u32,
    ) -> Insert {
        if samples.len() != self.frames_per_packet {
            return Insert::WrongSize;
        }

        let generation = self.generation();
        let slot = self.slot_for(sequence);

        // A stable slot from this generation holding this exact sequence is a
        // duplicate. Reading the version first means a half-written slot is
        // treated as absent rather than as a match.
        let version = slot.version.load(Ordering::Acquire);
        if version % 2 == 0 && slot.generation.load(Ordering::Relaxed) == generation {
            let held = slot.sequence.load(Ordering::Relaxed);
            if held == sequence {
                self.duplicates.fetch_add(1, Ordering::Relaxed);
                return Insert::Duplicate;
            }
            // The slot holds a sequence the reader has not played yet. This
            // packet wrapped onto it from more than SLOTS packets behind, so it
            // is far too late to be of any use — and the audio in the slot is
            // not.
            if audio_protocol::sequence_delta(held, reader_at) <= 0
                && audio_protocol::sequence_delta(sequence, held) > 0
            {
                self.too_late.fetch_add(1, Ordering::Relaxed);
                return Insert::TooLate;
            }
        }

        // Odd version: anything the reader reads from here on is discarded
        // until the write completes.
        slot.version.store(version.wrapping_add(1), Ordering::Release);

        slot.sequence.store(sequence, Ordering::Relaxed);
        slot.flags.store(u32::from(flags), Ordering::Relaxed);
        slot.arrived_us.store(arrived_us, Ordering::Relaxed);
        for (cell, &sample) in slot.samples.iter().zip(samples) {
            cell.store(sample.to_bits(), Ordering::Relaxed);
        }
        slot.generation.store(generation, Ordering::Relaxed);

        slot.version.store(version.wrapping_add(2), Ordering::Release);
        self.stored.fetch_add(1, Ordering::Relaxed);
        Insert::Stored
    }

    /// Copies the packet with this sequence into `out`, if it is there.
    ///
    /// Called from the audio callback. Allocation-free, lock-free, and bounded:
    /// at most two passes over one packet's samples.
    pub fn read(&self, sequence: u32, out: &mut [f32]) -> Option<Ready> {
        if out.len() != self.frames_per_packet {
            return None;
        }

        let generation = self.generation();
        let slot = self.slot_for(sequence);

        let before = slot.version.load(Ordering::Acquire);
        if before % 2 != 0 {
            // Mid-write. The packet may be this one, but it is not readable
            // yet; the caller conceals rather than playing a half-written slot.
            return None;
        }
        if slot.generation.load(Ordering::Relaxed) != generation {
            return None;
        }
        if slot.sequence.load(Ordering::Relaxed) != sequence {
            return None;
        }

        let ready = Ready {
            flags: slot.flags.load(Ordering::Relaxed) as u8,
            arrived_us: slot.arrived_us.load(Ordering::Relaxed),
        };
        for (slot_sample, out_sample) in slot.samples.iter().zip(out.iter_mut()) {
            *out_sample = f32::from_bits(slot_sample.load(Ordering::Relaxed));
        }

        // The writer touched this slot while we were copying, so what we have
        // is a mixture of two packets. Discard it: concealment sounds like a
        // soft dropout, a torn packet sounds like a click.
        if slot.version.load(Ordering::Acquire) != before {
            return None;
        }

        Some(ready)
    }

    /// Whether a sequence is present and readable.
    #[must_use]
    pub fn contains(&self, sequence: u32) -> bool {
        let generation = self.generation();
        let slot = self.slot_for(sequence);
        slot.version.load(Ordering::Acquire) % 2 == 0
            && slot.generation.load(Ordering::Relaxed) == generation
            && slot.sequence.load(Ordering::Relaxed) == sequence
    }

    /// How many packets are ready in an unbroken run from `sequence`.
    ///
    /// This is the buffer's **playable** depth, which is the only depth worth
    /// adapting on: three packets with a hole in the middle will underrun on
    /// the hole no matter how full the buffer looks.
    #[must_use]
    pub fn contiguous_from(&self, sequence: u32) -> usize {
        let mut depth = 0;
        while depth < SLOTS && self.contains(sequence.wrapping_add(depth as u32)) {
            depth += 1;
        }
        depth
    }

    /// Highest sequence present within one wrap of `sequence`.
    ///
    /// Used to tell "the sender stopped" from "there is a hole": a buffer with
    /// nothing at all beyond the read point is a stalled stream.
    #[must_use]
    pub fn highest_from(&self, sequence: u32) -> Option<u32> {
        (0..SLOTS as u32)
            .rev()
            .map(|offset| sequence.wrapping_add(offset))
            .find(|candidate| self.contains(*candidate))
    }

    #[must_use]
    pub fn duplicates(&self) -> u64 {
        self.duplicates.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn too_late(&self) -> u64 {
        self.too_late.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn stored(&self) -> u64 {
        self.stored.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAMES: usize = 240;

    fn buffer() -> JitterBuffer {
        JitterBuffer::new(FRAMES)
    }

    fn samples(value: f32) -> Vec<f32> {
        vec![value; FRAMES]
    }

    #[test]
    fn a_stored_packet_reads_back_exactly() {
        let buffer = buffer();
        let written: Vec<f32> = (0..FRAMES).map(|i| i as f32 / FRAMES as f32).collect();
        assert_eq!(buffer.insert(7, 0, 1_000, &written, 7), Insert::Stored);

        let mut read = vec![0.0; FRAMES];
        let ready = buffer.read(7, &mut read).expect("the packet is there");
        assert_eq!(read, written, "audio must survive the round trip bit-exactly");
        assert_eq!(ready.arrived_us, 1_000);
    }

    #[test]
    fn an_absent_packet_reads_as_nothing_rather_than_as_silence() {
        let buffer = buffer();
        let mut read = vec![9.0; FRAMES];
        assert!(buffer.read(7, &mut read).is_none());
        // Deliberately unmodified: the caller conceals, and silence inserted
        // here would be indistinguishable from audio that really was silent.
        assert_eq!(read[0], 9.0);
    }

    #[test]
    fn a_duplicate_is_rejected_without_rewriting_the_slot() {
        let buffer = buffer();
        buffer.insert(7, 0, 1_000, &samples(0.5), 7);
        assert_eq!(buffer.insert(7, 0, 2_000, &samples(0.25), 7), Insert::Duplicate);

        let mut read = vec![0.0; FRAMES];
        let ready = buffer.read(7, &mut read).unwrap();
        assert_eq!(read[0], 0.5, "the original audio must be kept");
        assert_eq!(ready.arrived_us, 1_000, "and its original arrival time");
        assert_eq!(buffer.duplicates(), 1);
    }

    #[test]
    fn packets_land_in_their_own_slots_whatever_order_they_arrive_in() {
        let buffer = buffer();
        // 10, 13, 11, 12: the out-of-order case the whole structure exists for.
        for sequence in [10_u32, 13, 11, 12] {
            buffer.insert(sequence, 0, 0, &samples(sequence as f32), 10);
        }
        let mut read = vec![0.0; FRAMES];
        for sequence in 10..=13_u32 {
            buffer.read(sequence, &mut read).unwrap();
            assert_eq!(read[0], sequence as f32, "sequence {sequence} is misplaced");
        }
    }

    #[test]
    fn contiguous_depth_stops_at_a_hole() {
        let buffer = buffer();
        for sequence in [10_u32, 11, 12, 14, 15] {
            buffer.insert(sequence, 0, 0, &samples(0.1), 10);
        }
        // Three playable, then a hole at 13 — the depth that matters.
        assert_eq!(buffer.contiguous_from(10), 3);
        assert_eq!(buffer.contiguous_from(14), 2);
        assert_eq!(buffer.contiguous_from(13), 0);
    }

    #[test]
    fn the_highest_sequence_distinguishes_a_hole_from_a_stalled_sender() {
        let buffer = buffer();
        assert_eq!(buffer.highest_from(10), None, "nothing at all: stalled");

        for sequence in [10_u32, 11, 20] {
            buffer.insert(sequence, 0, 0, &samples(0.1), 10);
        }
        assert_eq!(buffer.highest_from(10), Some(20));
        assert_eq!(buffer.contiguous_from(10), 2, "but only two are playable");
    }

    /// A packet that wrapped onto a slot the reader has not reached is
    /// hopelessly late — more than 256 packets, 1.28 s at the default cadence.
    /// Storing it would delete audio that is about to be played.
    #[test]
    fn a_packet_more_than_a_wrap_late_cannot_overwrite_live_audio() {
        let buffer = buffer();
        buffer.insert(300, 0, 0, &samples(1.0), 300);

        // 300 and 44 share slot 44. The reader is at 300, so 44 is 256 behind.
        assert_eq!(buffer.insert(44, 0, 0, &samples(2.0), 300), Insert::TooLate);

        let mut read = vec![0.0; FRAMES];
        buffer.read(300, &mut read).unwrap();
        assert_eq!(read[0], 1.0, "the live packet must survive");
        assert_eq!(buffer.too_late(), 1);
    }

    /// The reader has moved past 300, so that slot is free and the newer
    /// packet is allowed to claim it. Without this, the buffer would wedge
    /// after one wrap.
    #[test]
    fn a_slot_the_reader_has_passed_is_reusable() {
        let buffer = buffer();
        buffer.insert(300, 0, 0, &samples(1.0), 300);
        assert_eq!(buffer.insert(556, 0, 0, &samples(2.0), 400), Insert::Stored);

        let mut read = vec![0.0; FRAMES];
        assert!(buffer.read(300, &mut read).is_none(), "300 was overwritten");
        buffer.read(556, &mut read).unwrap();
        assert_eq!(read[0], 2.0);
    }

    #[test]
    fn a_reset_invalidates_everything_in_one_step() {
        let buffer = buffer();
        for sequence in 0..20_u32 {
            buffer.insert(sequence, 0, 0, &samples(0.5), 0);
        }
        assert_eq!(buffer.contiguous_from(0), 20);

        buffer.reset();

        assert_eq!(
            buffer.contiguous_from(0),
            0,
            "audio from before a session change must never be played after it"
        );
        let mut read = vec![0.0; FRAMES];
        assert!(buffer.read(5, &mut read).is_none());

        // And the buffer is immediately usable again.
        assert_eq!(buffer.insert(0, 0, 0, &samples(0.75), 0), Insert::Stored);
        buffer.read(0, &mut read).unwrap();
        assert_eq!(read[0], 0.75);
    }

    #[test]
    fn sequence_numbers_work_across_the_32_bit_wrap() {
        let buffer = buffer();
        for offset in 0..4_u32 {
            // MAX-1, MAX, 0, 1 — straddling the wrap.
            let sequence = (u32::MAX - 1).wrapping_add(offset);
            buffer.insert(sequence, 0, 0, &samples(offset as f32), u32::MAX - 1);
        }
        assert_eq!(
            buffer.contiguous_from(u32::MAX - 1),
            4,
            "a wrap is four consecutive packets, not a four-billion-packet gap"
        );
        let mut read = vec![0.0; FRAMES];
        buffer.read(0, &mut read).unwrap();
        assert_eq!(read[0], 2.0);
    }

    #[test]
    fn flags_reach_the_reader() {
        let buffer = buffer();
        let flags = audio_protocol::PacketFlags::SILENCE | audio_protocol::PacketFlags::DISCONTINUITY;
        buffer.insert(1, flags, 0, &samples(0.0), 1);
        let mut read = vec![0.0; FRAMES];
        assert_eq!(buffer.read(1, &mut read).unwrap().flags, flags);
    }

    #[test]
    fn a_wrongly_sized_packet_is_refused_rather_than_truncated() {
        let buffer = buffer();
        assert_eq!(buffer.insert(1, 0, 0, &[0.0; 120], 1), Insert::WrongSize);
        assert!(!buffer.contains(1));

        // And the reader will not accept a mismatched destination either.
        let mut wrong = vec![0.0; 120];
        buffer.insert(1, 0, 0, &samples(0.5), 1);
        assert!(buffer.read(1, &mut wrong).is_none());
    }

    /// The seqlock's job. A reader that copies across a write gets a mixture of
    /// two packets, which sounds like a click; it must come away with nothing
    /// instead, so the caller conceals.
    #[test]
    fn a_torn_read_is_discarded_rather_than_played() {
        let buffer = buffer();
        buffer.insert(1, 0, 0, &samples(0.5), 1);

        // Simulate a writer that began a write and has not finished: the slot's
        // version is odd.
        let slot = buffer.slot_for(1);
        let version = slot.version.load(Ordering::Acquire);
        slot.version.store(version + 1, Ordering::Release);

        let mut read = vec![0.0; FRAMES];
        assert!(buffer.read(1, &mut read).is_none(), "a half-written slot is not readable");

        // Completing the write makes it readable again.
        slot.version.store(version + 2, Ordering::Release);
        assert!(buffer.read(1, &mut read).is_some());
    }

    #[test]
    fn two_threads_can_use_it_at_once_without_a_lock() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let buffer = Arc::new(JitterBuffer::new(FRAMES));
        let stop = Arc::new(AtomicBool::new(false));

        // Writer: the network thread.
        let writer = {
            let buffer = Arc::clone(&buffer);
            std::thread::spawn(move || {
                for sequence in 0..20_000_u32 {
                    let audio = vec![sequence as f32; FRAMES];
                    buffer.insert(sequence, 0, u64::from(sequence), &audio, sequence);
                }
            })
        };

        // Reader: the audio callback. It must never see a torn packet — every
        // sample in a packet is identical, so a mixture is detectable.
        let reader = {
            let buffer = Arc::clone(&buffer);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut out = vec![0.0_f32; FRAMES];
                let mut reads = 0_u64;
                while !stop.load(Ordering::Relaxed) {
                    for sequence in 0..20_000_u32 {
                        if buffer.read(sequence, &mut out).is_some() {
                            assert!(
                                out.iter().all(|&sample| sample == out[0]),
                                "a torn packet reached the audio callback"
                            );
                            reads += 1;
                        }
                    }
                    if reads > 0 {
                        break;
                    }
                }
                reads
            })
        };

        writer.join().unwrap();
        stop.store(true, Ordering::Relaxed);
        let reads = reader.join().unwrap();
        assert!(reads > 0, "the reader never saw a packet, so this proved nothing");
    }
}

//! Turns the mono capture ring into StageLink datagrams.
//!
//! Runs on its own thread, between the capture thread and the sender. It is
//! allocation-free after construction for the same reason the capture thread is:
//! a GC-style pause here shows up as a gap in everybody's monitor feed.
//!
//! The packetiser only ever emits *whole* packets. A short packet would be read
//! by a receiver as a timing discontinuity, so when the ring has not yet
//! accumulated a full frame it emits nothing and records an underrun instead.

use core::sync::atomic::{AtomicU64, Ordering};

use audio_core::{RingConsumer, convert};
use audio_protocol::{
    AudioPacketHeader, HEADER_BYTES, PacketFlags, encode_header,
};

/// Counters for the packetiser.
#[derive(Debug, Default)]
pub struct PacketiserStats {
    packets_built: AtomicU64,
    /// Times the ring did not hold a whole frame when we asked.
    underruns: AtomicU64,
    /// Packets whose audio was entirely digital silence.
    silent_packets: AtomicU64,
}

impl PacketiserStats {
    #[must_use]
    pub fn packets_built(&self) -> u64 {
        self.packets_built.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn silent_packets(&self) -> u64 {
        self.silent_packets.load(Ordering::Relaxed)
    }
    pub fn reset(&self) {
        self.packets_built.store(0, Ordering::Relaxed);
        self.underruns.store(0, Ordering::Relaxed);
        self.silent_packets.store(0, Ordering::Relaxed);
    }
}

/// Builds sequential audio packets for one session.
pub struct PacketBuilder {
    session_id: u32,
    sample_rate: u32,
    frames_per_packet: usize,
    sequence: u32,
    /// Frames emitted so far; drives the packet timestamp.
    frames_emitted: u64,
    /// Scratch space for samples pulled from the ring. Allocated once.
    samples: Vec<f32>,
    /// Scratch space for the encoded datagram. Allocated once.
    datagram: Vec<u8>,
    /// Set when the capture thread reported a glitch, flagged on the next packet.
    pending_discontinuity: bool,
}

impl PacketBuilder {
    /// Creates a builder for one session.
    #[must_use]
    pub fn new(session_id: u32, sample_rate: u32, frames_per_packet: usize) -> Self {
        Self {
            session_id,
            sample_rate,
            frames_per_packet,
            sequence: 0,
            frames_emitted: 0,
            samples: vec![0.0; frames_per_packet],
            // Header plus S16 payload. The AEAD tag is appended by the
            // transport, which owns the key material.
            datagram: vec![0; HEADER_BYTES + frames_per_packet * 2],
            pending_discontinuity: false,
        }
    }

    /// Records that the capture path glitched. The next packet carries the
    /// discontinuity flag so receivers can resynchronise rather than treating
    /// the gap as drift.
    pub fn note_discontinuity(&mut self) {
        self.pending_discontinuity = true;
    }

    /// Builds the next packet from `ring`, if a whole frame is available.
    ///
    /// Returns the encoded datagram, or `None` when the ring is short. The
    /// returned slice borrows internal scratch space and is valid until the
    /// next call — the caller copies it into a per-receiver queue.
    pub fn next_packet(
        &mut self,
        ring: &mut RingConsumer,
        stats: &PacketiserStats,
        encrypted: bool,
    ) -> Option<&[u8]> {
        if !ring.pop_exact(&mut self.samples) {
            stats.underruns.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let silent = self.samples.iter().all(|&sample| sample == 0.0);
        let mut flags = PacketFlags::empty();
        if silent {
            flags = flags.with(PacketFlags::SILENCE);
            stats.silent_packets.fetch_add(1, Ordering::Relaxed);
        }
        if encrypted {
            flags = flags.with(PacketFlags::ENCRYPTED);
        }
        if self.pending_discontinuity {
            flags = flags.with(PacketFlags::DISCONTINUITY);
            self.pending_discontinuity = false;
        }

        let timestamp_us = (self.frames_emitted * 1_000_000) / u64::from(self.sample_rate);
        let header = AudioPacketHeader::new(self.session_id, self.sequence, timestamp_us, flags);

        encode_header(&header, &mut self.datagram).ok()?;
        let written = convert::f32_to_pcm_s16le(&self.samples, &mut self.datagram[HEADER_BYTES..]);

        self.sequence = self.sequence.wrapping_add(1);
        self.frames_emitted += self.frames_per_packet as u64;
        stats.packets_built.fetch_add(1, Ordering::Relaxed);

        Some(&self.datagram[..HEADER_BYTES + written])
    }

    /// Builds a zero-payload end-of-stream marker so receivers stop cleanly
    /// instead of waiting out their jitter buffer on sudden silence.
    pub fn end_of_stream(&mut self, encrypted: bool) -> &[u8] {
        let mut flags = PacketFlags::empty().with(PacketFlags::END_OF_STREAM);
        if encrypted {
            flags = flags.with(PacketFlags::ENCRYPTED);
        }
        let timestamp_us = (self.frames_emitted * 1_000_000) / u64::from(self.sample_rate);
        let header = AudioPacketHeader::new(self.session_id, self.sequence, timestamp_us, flags);
        encode_header(&header, &mut self.datagram).expect("scratch buffer holds a header");
        self.sequence = self.sequence.wrapping_add(1);
        &self.datagram[..HEADER_BYTES]
    }

    #[must_use]
    pub const fn sequence(&self) -> u32 {
        self.sequence
    }

    #[must_use]
    pub const fn frames_emitted(&self) -> u64 {
        self.frames_emitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_core::AudioRing;
    use audio_protocol::parse_header;

    fn ring_with(samples: &[f32]) -> (audio_core::RingProducer, RingConsumer) {
        let (mut producer, consumer) = AudioRing::with_capacity(4_096).unwrap();
        producer.push(samples);
        (producer, consumer)
    }

    #[test]
    fn a_short_ring_yields_no_packet_and_counts_an_underrun() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.5; 100]);

        assert!(
            builder.next_packet(&mut consumer, &stats, false).is_none(),
            "a partial frame must not become a short packet"
        );
        assert_eq!(stats.underruns(), 1);
        assert_eq!(stats.packets_built(), 0);
        assert_eq!(consumer.occupied(), 100, "the samples must not be consumed");
    }

    #[test]
    fn a_full_frame_produces_a_well_formed_packet() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(0xABCD, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.5; 240]);

        let datagram = builder
            .next_packet(&mut consumer, &stats, false)
            .expect("a whole frame was available")
            .to_vec();

        assert_eq!(datagram.len(), HEADER_BYTES + 480);
        let (header, payload) = parse_header(&datagram).unwrap();
        assert_eq!(header.session_id, 0xABCD);
        assert_eq!(header.sequence, 0);
        assert_eq!(header.timestamp_us, 0);
        assert_eq!(header.channels, 1);
        assert!(!header.flags.encrypted());
        assert_eq!(payload.len(), 480);
        assert_eq!(stats.packets_built(), 1);
    }

    #[test]
    fn sequence_and_timestamp_advance_by_one_frame_each_packet() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.25; 240 * 3]);

        for expected_sequence in 0..3_u32 {
            let datagram = builder
                .next_packet(&mut consumer, &stats, false)
                .unwrap()
                .to_vec();
            let (header, _) = parse_header(&datagram).unwrap();
            assert_eq!(header.sequence, expected_sequence);
            // 240 frames at 48 kHz is 5000 microseconds.
            assert_eq!(header.timestamp_us, u64::from(expected_sequence) * 5_000);
        }
        assert_eq!(builder.frames_emitted(), 720);
    }

    #[test]
    fn the_audio_survives_the_round_trip_through_s16() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 4);
        let input = [0.0_f32, 0.5, -0.5, 0.25];
        let (_producer, mut consumer) = ring_with(&input);

        let datagram = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        let (_, payload) = parse_header(&datagram).unwrap();

        let mut decoded = [0.0_f32; 4];
        convert::pcm_s16le_to_f32(payload, &mut decoded);
        for (original, round_tripped) in input.iter().zip(decoded.iter()) {
            assert!((original - round_tripped).abs() < 4e-5);
        }
    }

    #[test]
    fn digital_silence_is_flagged_so_receivers_can_skip_decoding() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.0; 240]);

        let datagram = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        let (header, _) = parse_header(&datagram).unwrap();
        assert!(header.flags.has(PacketFlags::SILENCE));
        assert_eq!(stats.silent_packets(), 1);
    }

    #[test]
    fn a_quiet_but_non_zero_signal_is_not_flagged_as_silence() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let mut samples = [0.0_f32; 240];
        samples[100] = 1e-6;
        let (_producer, mut consumer) = ring_with(&samples);

        let datagram = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        let (header, _) = parse_header(&datagram).unwrap();
        assert!(!header.flags.has(PacketFlags::SILENCE));
    }

    #[test]
    fn a_capture_glitch_flags_exactly_one_following_packet() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.5; 480]);

        builder.note_discontinuity();

        let first = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        assert!(parse_header(&first).unwrap().0.flags.has(PacketFlags::DISCONTINUITY));

        let second = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        assert!(
            !parse_header(&second).unwrap().0.flags.has(PacketFlags::DISCONTINUITY),
            "the flag must not stick"
        );
    }

    #[test]
    fn the_encrypted_flag_follows_the_transport() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.5; 240]);

        let datagram = builder
            .next_packet(&mut consumer, &stats, true)
            .unwrap()
            .to_vec();
        assert!(parse_header(&datagram).unwrap().0.flags.encrypted());
    }

    #[test]
    fn end_of_stream_is_an_empty_flagged_packet() {
        let mut builder = PacketBuilder::new(7, 48_000, 240);
        let datagram = builder.end_of_stream(false).to_vec();
        assert_eq!(datagram.len(), HEADER_BYTES);

        let (header, payload) = parse_header(&datagram).unwrap();
        assert!(header.flags.end_of_stream());
        assert_eq!(header.session_id, 7);
        assert!(payload.is_empty());
    }

    #[test]
    fn the_sequence_number_wraps_rather_than_overflowing() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 4);
        builder.sequence = u32::MAX;
        let (_producer, mut consumer) = ring_with(&[0.1; 8]);

        let first = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        assert_eq!(parse_header(&first).unwrap().0.sequence, u32::MAX);

        let second = builder
            .next_packet(&mut consumer, &stats, false)
            .unwrap()
            .to_vec();
        assert_eq!(
            parse_header(&second).unwrap().0.sequence,
            0,
            "the sequence must wrap, not panic"
        );
    }

    #[test]
    fn stats_can_be_reset_between_sessions() {
        let stats = PacketiserStats::default();
        let mut builder = PacketBuilder::new(1, 48_000, 240);
        let (_producer, mut consumer) = ring_with(&[0.0; 240]);
        builder.next_packet(&mut consumer, &stats, false).unwrap();
        assert_eq!(stats.packets_built(), 1);

        stats.reset();
        assert_eq!(stats.packets_built(), 0);
        assert_eq!(stats.silent_packets(), 0);
    }
}

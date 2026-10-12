//! Packet production and pacing.
//!
//! Packets are built by the **real** packetiser, `stream_server::PacketBuilder`,
//! fed through the **real** lock-free ring. A fixture that encoded its own
//! headers would drift from the product and start passing tests the product
//! fails, which is the one thing a reference implementation must not do. The
//! impairment model operates on the finished datagrams, exactly where a network
//! operates.

use std::time::{Duration, Instant};

use audio_core::{AudioRing, RingConsumer, RingProducer, frames_per_packet};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use stream_server::packetiser::{PacketBuilder, PacketiserStats};

use crate::impairment::{Impairment, ImpairmentConfig, Verdict};
use crate::source::Source;

/// Where finished datagrams go.
///
/// A trait so the whole transmitter can be driven in a unit test with no socket
/// and no clock. The alternative — testing only against a real UDP socket —
/// would make the impairment and session logic unverifiable in CI.
pub trait Sink {
    fn send(&mut self, datagram: &[u8]) -> std::io::Result<()>;
}

/// Collects datagrams in memory. Used by the tests and by `--dry-run`.
#[derive(Debug, Default)]
pub struct CollectingSink {
    pub datagrams: Vec<Vec<u8>>,
}

impl Sink for CollectingSink {
    fn send(&mut self, datagram: &[u8]) -> std::io::Result<()> {
        self.datagrams.push(datagram.to_vec());
        Ok(())
    }
}

/// How a transmission is shaped.
#[derive(Debug, Clone)]
pub struct TransmitConfig {
    pub sample_rate: u32,
    pub frame_duration_ms: f64,
    pub session_id: u32,
    /// Stop after this many packets. `None` runs until the source ends.
    pub packet_limit: Option<u64>,
    /// Start a new session (new id, sequence back to zero) after this many
    /// packets. Receivers must treat it as a discontinuity and flush.
    pub session_change_after: Option<u64>,
    /// Capture-clock error in parts per million. Positive runs fast.
    pub clock_ppm: f64,
    /// Uniform random send delay, in milliseconds, on top of the cadence.
    pub jitter_ms: f64,
    /// Emit an end-of-stream marker when the source finishes.
    pub end_of_stream: bool,
    /// Set the ENCRYPTED flag and reserve the tag region.
    ///
    /// The transmitter holds no key material, so the payload is **not**
    /// actually encrypted — this exists to verify that a receiver parses the
    /// framing, not to test cryptography. A receiver that decrypts will fail
    /// authentication, which is the correct outcome for an unsealed packet.
    pub encrypted: bool,
    pub impairment: ImpairmentConfig,
    pub seed: u64,
}

impl TransmitConfig {
    #[must_use]
    pub fn frames_per_packet(&self) -> usize {
        frames_per_packet(self.sample_rate, self.frame_duration_ms)
    }

    /// Nominal time between packets, before clock error.
    #[must_use]
    pub fn packet_interval(&self) -> Duration {
        Duration::from_secs_f64(self.frame_duration_ms / 1_000.0)
    }

    /// Time between packets as the transmitter's clock sees it.
    ///
    /// A capture clock running `ppm` fast produces packets `ppm` sooner. Over a
    /// 90-minute service, 40 ppm is 216 ms of accumulated offset — enough to
    /// starve or overflow any fixed receiver buffer, which is why this is a
    /// first-class knob rather than something to be approximated by hand.
    #[must_use]
    pub fn skewed_interval(&self) -> Duration {
        let nominal = self.frame_duration_ms / 1_000.0;
        Duration::from_secs_f64(nominal / (1.0 + self.clock_ppm / 1_000_000.0))
    }
}

/// What a run actually did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransmitReport {
    /// Packets the packetiser produced.
    pub built: u64,
    /// Datagrams handed to the sink, counting duplicates.
    pub sent: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub sessions: u32,
    pub end_of_stream_sent: bool,
}

impl TransmitReport {
    /// Loss as it actually happened, which is what a receiver's own figures
    /// must be compared against. A short run at a configured 2% does not drop
    /// exactly 2%, and a receiver reporting the configured number rather than
    /// this one would look correct while being wrong.
    #[must_use]
    pub fn realised_loss_percent(&self) -> f64 {
        if self.built == 0 {
            return 0.0;
        }
        self.dropped as f64 * 100.0 / self.built as f64
    }
}

/// Drives a [`Source`] through the real packetiser into a [`Sink`].
pub struct Transmitter {
    config: TransmitConfig,
    source: Source,
    producer: RingProducer,
    consumer: RingConsumer,
    builder: PacketBuilder,
    stats: PacketiserStats,
    impairment: Impairment,
    /// Separate generator so adding `--jitter-ms` does not shift the loss
    /// pattern a previous run found a bug with.
    jitter_rng: StdRng,
    frames: Vec<f32>,
    /// A packet held back by a reorder verdict.
    held: Option<Vec<u8>>,
    /// Set once the source has produced its last frames. The packet it was set
    /// by is still sent — it is partly real audio — and the run ends after it.
    exhausted: bool,
    report: TransmitReport,
    session_id: u32,
}

impl Transmitter {
    pub fn new(config: TransmitConfig, source: Source) -> Self {
        let frames_per_packet = config.frames_per_packet();
        // Two packets of headroom: the producer writes one packet at a time
        // immediately before the consumer reads it, so this never fills.
        let (producer, consumer) = AudioRing::with_capacity(frames_per_packet * 4)
            .expect("a four-packet ring is a valid capacity");

        Self {
            builder: PacketBuilder::new(config.session_id, config.sample_rate, frames_per_packet),
            frames: vec![0.0; frames_per_packet],
            impairment: Impairment::new(config.impairment, config.seed),
            jitter_rng: StdRng::seed_from_u64(config.seed ^ 0x5A5A_5A5A),
            session_id: config.session_id,
            source,
            producer,
            consumer,
            stats: PacketiserStats::default(),
            held: None,
            exhausted: false,
            report: TransmitReport { sessions: 1, ..TransmitReport::default() },
            config,
        }
    }

    /// Builds the next datagram, or `None` if the ring was short — which
    /// cannot happen here, since one packet is written immediately before it
    /// is read, but is handled rather than unwrapped.
    fn build_next(&mut self) -> Option<Vec<u8>> {
        if !self.source.fill(&mut self.frames) {
            self.exhausted = true;
        }
        self.producer.push(&self.frames);
        let datagram = self
            .builder
            .next_packet(&mut self.consumer, &self.stats, self.config.encrypted)?
            .to_vec();
        self.report.built += 1;
        Some(datagram)
    }

    /// Starts a new session: a fresh id and the sequence back to zero.
    ///
    /// The id is derived from the old one rather than taken at random so a run
    /// stays reproducible, and it is guaranteed to differ — a "new session"
    /// that reused the id would not be one, and a receiver would keep playing
    /// its stale buffer.
    fn rotate_session(&mut self) {
        self.session_id = self.session_id.wrapping_add(1);
        self.builder = PacketBuilder::new(
            self.session_id,
            self.config.sample_rate,
            self.config.frames_per_packet(),
        );
        self.consumer.clear();
        self.report.sessions += 1;
    }

    /// Runs to completion, pacing with `sleep` between packets.
    ///
    /// `sleep` is injected so tests run instantly: a 90-minute drift scenario
    /// is not something a test suite can wait for.
    pub fn run<S: Sink>(
        &mut self,
        sink: &mut S,
        mut sleep: impl FnMut(Duration),
    ) -> std::io::Result<TransmitReport> {
        let interval = self.config.skewed_interval();
        // The schedule advances by exactly one interval per packet and jitter
        // is applied *around* it, never added to it. Adding it would lower the
        // mean packet rate — at `--jitter-ms 10` on a 5 ms cadence the stream
        // would run at two thirds speed — so every buffer-depth and drift
        // measurement taken under jitter would be measuring the wrong thing.
        let mut slot = Instant::now();

        loop {
            if self
                .config
                .packet_limit
                .is_some_and(|limit| self.report.built >= limit)
            {
                break;
            }
            if self
                .config
                .session_change_after
                .is_some_and(|after| self.report.built == after && self.report.sessions == 1)
            {
                self.rotate_session();
            }

            let Some(datagram) = self.build_next() else { break };

            // The outage clock advances with the audio, not with wall time, so
            // a dry run and a real run impair identically.
            self.impairment
                .advance(self.config.frame_duration_ms.round() as u64);

            match self.impairment.next_verdict() {
                Verdict::Send => {
                    self.deliver(sink, &datagram)?;
                    self.release_held(sink)?;
                }
                Verdict::Drop => {
                    self.report.dropped += 1;
                    // A held packet is still owed, even if this one is lost.
                    self.release_held(sink)?;
                }
                Verdict::Duplicate => {
                    self.deliver(sink, &datagram)?;
                    self.deliver(sink, &datagram)?;
                    self.report.duplicated += 1;
                    self.release_held(sink)?;
                }
                Verdict::Delay => {
                    // Held until after the next packet, which is what reorder
                    // means on the wire.
                    if let Some(previous) = self.held.replace(datagram) {
                        self.deliver(sink, &previous)?;
                    }
                    self.report.reordered += 1;
                }
            }

            if self.exhausted {
                break;
            }

            slot += interval;
            let target = slot + self.jitter();
            let now = Instant::now();
            if target > now {
                sleep(target - now);
            }
        }

        // Anything still held belongs on the wire before the stream ends.
        self.release_held(sink)?;

        if self.config.end_of_stream {
            let marker = self.builder.end_of_stream(self.config.encrypted).to_vec();
            self.deliver(sink, &marker)?;
            self.report.end_of_stream_sent = true;
        }

        debug_assert_eq!(
            self.report.dropped,
            self.impairment.stats().dropped,
            "the report and the impairment model must agree on what was dropped"
        );
        Ok(self.report)
    }

    fn deliver<S: Sink>(&mut self, sink: &mut S, datagram: &[u8]) -> std::io::Result<()> {
        sink.send(datagram)?;
        self.report.sent += 1;
        Ok(())
    }

    fn release_held<S: Sink>(&mut self, sink: &mut S) -> std::io::Result<()> {
        if let Some(held) = self.held.take() {
            self.deliver(sink, &held)?;
        }
        Ok(())
    }

    fn jitter(&mut self) -> Duration {
        if self.config.jitter_ms <= 0.0 {
            return Duration::ZERO;
        }
        let millis = self.jitter_rng.random::<f64>() * self.config.jitter_ms;
        Duration::from_secs_f64(millis / 1_000.0)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_protocol::{PacketFlags, parse_header};

    fn config() -> TransmitConfig {
        TransmitConfig {
            sample_rate: 48_000,
            frame_duration_ms: 5.0,
            session_id: 0x5EED,
            packet_limit: Some(100),
            session_change_after: None,
            clock_ppm: 0.0,
            jitter_ms: 0.0,
            end_of_stream: false,
            encrypted: false,
            impairment: ImpairmentConfig::default(),
            seed: 1,
        }
    }

    /// No sleeping: a test must not wait for audio to play out in real time.
    fn run(config: TransmitConfig) -> (CollectingSink, TransmitReport) {
        let source = Source::tone(1_000.0, 0.5, config.sample_rate);
        let mut transmitter = Transmitter::new(config, source);
        let mut sink = CollectingSink::default();
        let report = transmitter.run(&mut sink, |_| {}).unwrap();
        (sink, report)
    }

    #[test]
    fn a_clean_run_sends_every_packet_in_order() {
        let (sink, report) = run(config());
        assert_eq!(report.built, 100);
        assert_eq!(report.sent, 100);
        assert_eq!(report.dropped, 0);

        for (index, datagram) in sink.datagrams.iter().enumerate() {
            let (header, payload) = parse_header(datagram).expect("a real StageLink packet");
            assert_eq!(header.sequence, index as u32);
            assert_eq!(header.session_id, 0x5EED);
            assert_eq!(header.channels, 1, "the mix is mono");
            // 240 frames of S16 mono.
            assert_eq!(payload.len(), 480);
        }
    }

    /// Timestamps are what a receiver infers the sample rate from, so they have
    /// to advance by exactly one packet's worth.
    #[test]
    fn timestamps_advance_by_the_packet_duration() {
        let (sink, _) = run(config());
        let timestamp = |index: usize| parse_header(&sink.datagrams[index]).unwrap().0.timestamp_us;
        assert_eq!(timestamp(0), 0);
        assert_eq!(timestamp(1), 5_000);
        assert_eq!(timestamp(99), 495_000);

        // And the rate a receiver derives from them is the real one.
        let delta = timestamp(2) - timestamp(1);
        let derived = 240.0 / (delta as f64 / 1e6);
        assert!((derived - 48_000.0).abs() < 1.0, "derived {derived} Hz");
    }

    #[test]
    fn a_dropped_packet_leaves_a_sequence_gap_and_nothing_else() {
        let settings = TransmitConfig {
            impairment: ImpairmentConfig { loss_percent: 20.0, ..ImpairmentConfig::default() },
            ..config()
        };
        let (sink, report) = run(settings);
        assert!(report.dropped > 5, "dropped {}", report.dropped);
        assert_eq!(report.sent + report.dropped, 100);

        // Sequence numbers must still be strictly increasing: a gap, never a
        // renumbering, or a receiver's loss accounting would disagree with the
        // desktop's and obligation 2 in pairing.md would be unmeetable.
        let observed = sequences(&sink);
        assert!(observed.windows(2).all(|pair| pair[1] > pair[0]));
    }

    #[test]
    fn a_duplicate_is_byte_identical_so_a_receiver_can_discard_it() {
        let settings = TransmitConfig {
            impairment: ImpairmentConfig {
                duplicate_percent: 100.0,
                ..ImpairmentConfig::default()
            },
            packet_limit: Some(10),
            ..config()
        };
        let (sink, report) = run(settings);
        assert_eq!(report.sent, 20, "every packet sent twice");
        for pair in sink.datagrams.chunks(2) {
            assert_eq!(pair[0], pair[1]);
        }
    }

    fn sequences(sink: &CollectingSink) -> Vec<u32> {
        sink.datagrams
            .iter()
            .map(|datagram| parse_header(datagram).unwrap().0.sequence)
            .collect()
    }

    #[test]
    fn a_reordered_packet_arrives_after_its_successor() {
        let settings = TransmitConfig {
            impairment: ImpairmentConfig {
                reorder_percent: 30.0,
                ..ImpairmentConfig::default()
            },
            packet_limit: Some(40),
            ..config()
        };
        let (sink, report) = run(settings);
        assert_eq!(report.sent, 40, "a reorder delays a packet, it does not lose it");
        assert!(report.reordered > 5, "reordered {}", report.reordered);

        let observed = sequences(&sink);
        let mut sorted = observed.clone();
        sorted.sort_unstable();
        assert_ne!(observed, sorted, "arrival order must differ from send order");
        // And every packet still arrives exactly once: a late packet that a
        // receiver counts as lost *and* then plays is the bug this models.
        assert_eq!(sorted, (0..40).collect::<Vec<u32>>());
    }

    /// A packet is delayed behind the next packet that is *not* itself
    /// delayed. So at 100% there is no packet to swap with and the result is a
    /// uniform one-packet delay — latency, not reorder. Pinned because it is
    /// surprising, and because a reader reaching for "maximum chaos" would
    /// otherwise reach for exactly the wrong number.
    #[test]
    fn total_reorder_degenerates_to_a_uniform_delay() {
        let settings = TransmitConfig {
            impairment: ImpairmentConfig {
                reorder_percent: 100.0,
                ..ImpairmentConfig::default()
            },
            packet_limit: Some(6),
            ..config()
        };
        let (sink, report) = run(settings);
        assert_eq!(report.sent, 6);
        assert_eq!(sequences(&sink), vec![0, 1, 2, 3, 4, 5]);
    }

    /// The case receiver obligation 3 exists for: the id changes and the
    /// sequence restarts, so a receiver that kept playing its buffer would play
    /// audio from before the change.
    #[test]
    fn a_session_change_restarts_the_sequence_under_a_new_id() {
        let settings = TransmitConfig {
            session_change_after: Some(10),
            packet_limit: Some(20),
            ..config()
        };
        let (sink, report) = run(settings);
        assert_eq!(report.sessions, 2);

        let header = |index: usize| parse_header(&sink.datagrams[index]).unwrap().0;
        assert_eq!(header(9).sequence, 9);
        assert_eq!(header(9).session_id, 0x5EED);
        assert_eq!(header(10).sequence, 0, "the sequence restarts");
        assert_ne!(
            header(10).session_id, 0x5EED,
            "a new session must not reuse the id, or it is not a new session"
        );
    }

    #[test]
    fn an_end_of_stream_marker_closes_the_stream_and_parses() {
        let settings = TransmitConfig {
            end_of_stream: true,
            packet_limit: Some(5),
            ..config()
        };
        let (sink, report) = run(settings);
        assert!(report.end_of_stream_sent);

        let last = sink.datagrams.last().unwrap();
        let (header, payload) = parse_header(last).expect("the marker must parse");
        assert!(header.flags.end_of_stream());
        assert!(payload.is_empty());
        assert_eq!(header.sequence, 5, "it continues the sequence");
    }

    /// D1 was exactly this case failing. An encrypted marker with no tag region
    /// is rejected as malformed, and a receiver then waits out its jitter
    /// buffer on sudden silence instead of stopping.
    #[test]
    fn an_encrypted_run_stays_parseable_including_the_marker() {
        let settings = TransmitConfig {
            encrypted: true,
            end_of_stream: true,
            packet_limit: Some(5),
            ..config()
        };
        let (sink, _) = run(settings);
        for datagram in &sink.datagrams {
            let (header, _) = parse_header(datagram).expect("every packet must parse");
            assert!(header.flags.has(PacketFlags::ENCRYPTED));
        }
        let (header, _) = parse_header(sink.datagrams.last().unwrap()).unwrap();
        assert!(header.flags.end_of_stream());
    }

    #[test]
    fn digital_silence_is_flagged_so_a_receiver_can_skip_decoding() {
        let mut transmitter = Transmitter::new(config(), Source::Silence);
        let mut sink = CollectingSink::default();
        transmitter.run(&mut sink, |_| {}).unwrap();
        let (header, _) = parse_header(&sink.datagrams[0]).unwrap();
        assert!(header.flags.has(PacketFlags::SILENCE));
    }

    #[test]
    fn a_fast_clock_shortens_the_interval_by_the_stated_ppm() {
        let nominal = config().packet_interval().as_secs_f64();

        let fast = TransmitConfig { clock_ppm: 100.0, ..config() };
        let slow = TransmitConfig { clock_ppm: -100.0, ..config() };

        assert!(fast.skewed_interval().as_secs_f64() < nominal);
        assert!(slow.skewed_interval().as_secs_f64() > nominal);

        // 100 ppm over 90 minutes is 540 ms of accumulated offset. That is the
        // figure the receiver's drift controller has to absorb, so the skew has
        // to be accurate rather than merely in the right direction.
        let packets_in_90_minutes = 90.0 * 60.0 / nominal;
        let offset = (nominal - fast.skewed_interval().as_secs_f64()) * packets_in_90_minutes;
        assert!((offset - 0.540).abs() < 0.01, "accumulated {offset} s");
    }

    /// Jitter must perturb *when* a packet is sent without changing how many
    /// are sent per second. The first version of this added jitter to the
    /// interval, which at `--jitter-ms 10` on a 5 ms cadence ran the stream at
    /// two thirds speed — so a receiver starved for a reason that was not
    /// jitter, and every buffer measurement taken under it was meaningless.
    #[test]
    fn jitter_is_bounded_and_perturbs_rather_than_delays() {
        let settings = TransmitConfig { jitter_ms: 20.0, ..config() };
        let mut transmitter = Transmitter::new(settings, Source::Silence);

        let draws: Vec<f64> = (0..10_000)
            .map(|_| transmitter.jitter().as_secs_f64() * 1_000.0)
            .collect();
        assert!(
            draws.iter().all(|&ms| (0.0..=20.0).contains(&ms)),
            "jitter must stay inside the requested window"
        );
        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        assert!((mean - 10.0).abs() < 0.5, "mean jitter was {mean} ms");

        // Zero means zero, so a clean run is paced exactly.
        let mut clean = Transmitter::new(config(), Source::Silence);
        assert_eq!(clean.jitter(), Duration::ZERO);
    }

    /// Jitter must perturb *when* a packet is sent without changing how many
    /// are sent per second. The first version added jitter to the interval,
    /// which at 20 ms on a 5 ms cadence ran the stream at a fifth of its rate —
    /// so a receiver starved for a reason that was not jitter, and every buffer
    /// measurement taken under it was meaningless.
    ///
    /// Timed against the real clock, because the bug was in the schedule
    /// arithmetic and a virtual clock is exactly what would hide it. One
    /// second of nominal audio, with a tolerance wide enough for Windows
    /// timer granularity and narrow enough that a fifth-rate stream fails.
    #[test]
    fn jitter_does_not_change_the_mean_packet_rate() {
        let settings = TransmitConfig {
            jitter_ms: 20.0,
            packet_limit: Some(200),
            ..config()
        };
        let mut transmitter = Transmitter::new(settings, Source::tone(1_000.0, 0.5, 48_000));
        let mut sink = CollectingSink::default();

        let started = Instant::now();
        transmitter.run(&mut sink, std::thread::sleep).unwrap();
        let elapsed = started.elapsed().as_secs_f64();

        assert!(
            (0.9..2.0).contains(&elapsed),
            "200 packets at 5 ms is one second of audio; this run took {elapsed:.2} s"
        );
    }

    #[test]
    fn a_whole_run_is_reproducible_from_its_seed() {
        let settings = TransmitConfig {
            impairment: ImpairmentConfig {
                loss_percent: 8.0,
                burst_length: 4,
                reorder_percent: 2.0,
                duplicate_percent: 1.0,
                ..ImpairmentConfig::default()
            },
            jitter_ms: 3.0,
            ..config()
        };
        let (first, first_report) = run(settings.clone());
        let (second, second_report) = run(settings.clone());
        assert_eq!(first.datagrams, second.datagrams);
        assert_eq!(first_report, second_report);

        let (other, _) = run(TransmitConfig { seed: 2, ..settings });
        assert_ne!(first.datagrams, other.datagrams, "a new seed must differ");
    }

    #[test]
    fn a_finite_source_ends_the_run_without_a_packet_limit() {
        let settings = TransmitConfig { packet_limit: None, ..config() };
        // Two and a half packets' worth, so the last packet is partly silence.
        let source = Source::Wav {
            samples: vec![0.25; 600],
            position: 0,
            looping: false,
        };
        let mut transmitter = Transmitter::new(settings, source);
        let mut sink = CollectingSink::default();
        let report = transmitter.run(&mut sink, |_| {}).unwrap();
        assert_eq!(report.built, 3, "600 frames is three 240-frame packets");
        assert_eq!(report.sent, 3);
    }
}

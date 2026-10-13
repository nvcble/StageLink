//! The receiver, assembled.
//!
//! Split into two halves that correspond to the two threads that really exist
//! on a phone, rather than to a tidy object model:
//!
//! * [`NetworkInput`] — owned by the socket thread. Parses datagrams, keeps the
//!   sequence accounting, decodes PCM, and files packets in the jitter buffer.
//! * [`AudioOutput`] — owned by the audio callback. De-jitters, conceals,
//!   resamples, applies the monitor chain, and hands back frames.
//!
//! They share an [`Arc`] and never a lock. The split is the point: if these
//! were one object behind a mutex, a network thread doing a little too much
//! work would stall the audio callback, and a stalled audio callback is a gap
//! in somebody's monitor feed.
//!
//! ## De-jittering happens inside the render callback
//!
//! Not in a second queue feeding it. Two queues in series add their depths
//! together — latency the product cannot spare — and, worse, hide the real
//! buffer depth from the adaptation logic, which then steers on a number that
//! is not the one that underruns.
//!
//! ## Everything concealed, nothing skipped
//!
//! A missing packet is replaced with a packet's worth of concealment and the
//! read position advances by one, so the buffer keeps its depth. Jumping the
//! read position forward to the next packet that *is* present would discard
//! the gap's worth of depth permanently, and a buffer that loses depth on every
//! gap underruns on the one after it.
//!
//! The exceptions are the cases where continuing is worse than resyncing: a
//! gap too long to conceal, an explicit `DISCONTINUITY`, a session change, or a
//! backlog too large for the resampler to retire. Each resyncs with a fade.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use audio_protocol::{PacketFlags, SequenceTracker, SequenceVerdict, parse_header};

use crate::conceal::Concealer;
use crate::drift::{DriftConfig, DriftController};
use crate::dsp::{MonitorChain, MonitorSettings};
use crate::jitter::JitterBuffer;
use crate::resample::Resampler;

/// How deep the buffer is kept, as a trade between latency and resilience.
///
/// Three named choices rather than a millisecond field, because the number
/// means nothing to a performer and the trade does: "it dropped out" against
/// "it feels late".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preset {
    /// 20 ms. A clean, dedicated access point and a short distance.
    Tight,
    /// 40 ms. The default.
    #[default]
    Normal,
    /// 80 ms. A busy room, a shared network, or a phone at the back of a hall.
    Safe,
}

impl Preset {
    #[must_use]
    pub const fn target_ms(self) -> f32 {
        match self {
            Self::Tight => 20.0,
            Self::Normal => 40.0,
            Self::Safe => 80.0,
        }
    }
}

/// Default [`ReceiverConfig::startup_margin_ms`].
///
/// **Zero**, and that is a measured result rather than an omission.
///
/// Starting deeper than the target sounds like cheap insurance and is not. The
/// depth the drift controller steers on counts holes — see
/// `JitterBuffer::span_from` — so under packet loss it reads higher than the
/// audio that can actually be played. A start above target adds a standing
/// drain on top of that inflation, and the controller then drains real audio
/// to make up a surplus that was never there. Measured in the harness: a 10 ms
/// margin collapsed the buffer to 0.5 ms under 2% burst loss, and pushed the
/// reported clock estimate to 65 ppm when the true difference was 40.
///
/// Nothing is given up by starting level. Priming requires its depth as a
/// **gap-free** run (`contiguous_from`), which is a stronger guarantee than
/// the span-based depth the stream is held at afterwards — so a stream that
/// has just started has at least as much real runway as one that has been
/// playing for an hour. The margin was protecting against something that
/// contiguity already covers.
///
/// Raising this is the wrong half of the fix. If the buffer needs to start
/// deeper, make the depth signal immune to holes first: steer on a windowed
/// minimum, which is the adaptive-depth work `DriftController` already notes
/// it wants.
pub const DEFAULT_STARTUP_MARGIN_MS: f32 = 0.0;

/// Receiver configuration.
#[derive(Debug, Clone, Copy)]
pub struct ReceiverConfig {
    pub sample_rate: u32,
    pub frames_per_packet: usize,
    pub preset: Preset,
    /// Depth required before playback starts *beyond* the preset's target.
    ///
    /// Playback begins at the target plus this, so the preset's promise is true
    /// from the first packet rather than becoming true later. Defaults to zero;
    /// see [`DEFAULT_STARTUP_MARGIN_MS`] for why, and read it before raising
    /// this.
    ///
    /// What matters is that the startup depth is *relative to the target at
    /// all*. It was an absolute 30 ms against targets of 20/40/80 ms, so the
    /// controller corrected downwards for Tight and upwards for Normal and
    /// Safe — and Safe, chosen precisely because the room is bad, began 50 ms
    /// short of what it promised and spent its first minute with less
    /// protection than Normal gives.
    pub startup_margin_ms: f32,
}

impl ReceiverConfig {
    #[must_use]
    pub const fn new(sample_rate: u32, frames_per_packet: usize) -> Self {
        Self {
            sample_rate,
            frames_per_packet,
            preset: Preset::Normal,
            startup_margin_ms: DEFAULT_STARTUP_MARGIN_MS,
        }
    }

    fn frames_for_ms(&self, millis: f32) -> usize {
        ((millis / 1_000.0) * self.sample_rate as f32).round() as usize
    }

    fn target_frames(&self) -> usize {
        self.frames_for_ms(self.preset.target_ms())
    }

    /// Depth required before playback starts, in milliseconds.
    #[must_use]
    pub fn startup_ms(&self) -> f32 {
        self.preset.target_ms() + self.startup_margin_ms
    }

    fn startup_packets(&self) -> usize {
        // Rounded up: a part-filled packet is not a packet the reader can
        // play, so truncating would start below the target that the margin
        // exists to clear.
        self.frames_for_ms(self.startup_ms())
            .div_ceil(self.frames_per_packet.max(1))
            .max(1)
    }
}

/// What [`NetworkInput::accept`] made of a datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    /// Filed for playback.
    Stored,
    /// Already held.
    Duplicate,
    /// Arrived too late to be of use.
    TooLate,
    /// A new session started; the buffer was flushed.
    SessionStarted,
    /// The sender said it is finished.
    EndOfStream,
    /// Not a StageLink packet, or a malformed one. Counted and ignored.
    Rejected,
    /// Encrypted, and this receiver has no key.
    Encrypted,
    /// Failed authentication: altered, replayed, or sealed with another key.
    ///
    /// Distinct from `Rejected` because it means something different — a
    /// malformed packet is noise on the port, and this is somebody trying.
    Unauthentic,
}

/// Counters a receiver reports about itself.
///
/// Maps onto `receiverTelemetryParamsSchema` in the shared protocol. Most of it
/// comes straight out of the same `SequenceTracker` the desktop uses, which is
/// what makes `docs/protocol/pairing.md` obligation 2 — that the two agree —
/// achievable rather than aspirational.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Telemetry {
    pub packets_received: u64,
    pub packets_lost: u64,
    pub loss_percent: f64,
    pub duplicates: u64,
    pub reordered: u64,
    pub rejected: u64,
    /// Encrypted packets this receiver had no key for.
    ///
    /// Deliberately **not** counted as `rejected`: a malformed packet is noise
    /// on the port, and this is a stream we cannot open. A handful at the very
    /// start is normal, because the first packets can outrun the
    /// `stream.params` carrying the session id — but a figure that keeps
    /// climbing means the key never arrived and the performer hears nothing.
    pub keyless: u64,
    /// Packets that failed authentication. Non-zero means either a key
    /// mismatch or somebody on the network injecting packets.
    pub unauthentic: u64,
    /// Absolute peak of the arriving mix, linear, where 1.0 is full scale and
    /// 0.0 is digital silence.
    ///
    /// Linear rather than decibels so it crosses a C ABI without needing a
    /// sentinel for silence, which in dB is negative infinity.
    pub arrival_peak: f32,
    /// Playable depth, in milliseconds.
    pub buffer_health_ms: f64,
    /// Times the buffer had nothing to play and concealment ran.
    pub concealed_runs: u64,
    pub concealed_ms: f64,
    pub resyncs: u64,
    /// The clock difference the drift controller has **learned**, in parts per
    /// million.
    ///
    /// This is the integral term, not the instantaneous correction. The
    /// instantaneous value also carries the proportional response to normal
    /// buffer-depth variation, which swings by hundreds of ppm on a perfectly
    /// healthy stream — so reporting it would bury the signal this field
    /// exists for. A learned value pinned at its clamp for minutes means the
    /// two crystals are further apart than the budget allows, and that is the
    /// one symptom which shows up before a dropout does.
    pub resample_ratio_ppm: f64,
    /// The correction applied right now, including the proportional term.
    /// Useful to a harness; too noisy to report to an operator.
    pub instant_correction_ppm: f64,
    /// The drift controller's smoothed depth error, in frames. Its input.
    pub drift_error_frames: f64,
    /// Samples the safety clip had to catch. Should be zero.
    pub clipped: u64,
    pub playing: bool,
    pub end_of_stream: bool,
}

/// State shared by the two halves. Atomics only; no locks.
/// Release applied to the arrival meter between packets.
///
/// At 200 packets a second, a bare per-packet peak sampled by the app at 10 Hz
/// would show whichever five milliseconds happened to land last: it flickers
/// and it under-reads. This falls about 20 dB per second, which looks like a
/// meter — and because it only ever decays *between* packets, it can never
/// report more signal than actually arrived.
const ARRIVAL_METER_RELEASE: f32 = 0.9886;

struct Shared {
    buffer: JitterBuffer,
    /// Absolute peak of the audio arriving from the desktop, as `f32` bits.
    ///
    /// Measured on the network thread, before the performer's gain, mute and
    /// EQ, so a meter built on it answers "is the mix reaching this phone?"
    /// rather than "is this phone making a noise" — the two differ precisely
    /// when somebody has muted themselves and wants to know why it is quiet.
    arrival_peak_bits: AtomicU32,
    /// Sequence the audio callback will play next. Read by the network thread
    /// so it never overwrites audio that is still needed.
    read_sequence: AtomicU32,
    /// First sequence of the current stream, published by the network thread.
    start_sequence: AtomicU32,
    /// Bumped when a new stream starts. The audio callback resyncs on a change.
    stream_epoch: AtomicU32,
    end_of_stream: AtomicBool,

    // Network-side counters.
    received: AtomicU64,
    lost: AtomicU64,
    duplicates: AtomicU64,
    reordered: AtomicU64,
    rejected: AtomicU64,
    keyless: AtomicU64,
    unauthentic: AtomicU64,

    // Audio-side counters.
    concealed_runs: AtomicU64,
    concealed_samples: AtomicU64,
    resyncs: AtomicU64,
    clipped: AtomicU64,
    depth_frames: AtomicU64,
    correction_ppm_milli: AtomicU64,
    learned_ppm_milli: AtomicU64,
    drift_error_milliframes: AtomicU64,
    playing: AtomicBool,
}

/// A read-only view of the counters, usable from any thread.
///
/// Needed because the thing that *reports* telemetry is neither half: on a
/// phone the control channel sends it, and the control channel owns neither
/// the socket nor the audio callback. Handing it a clone of this is cheaper
/// and safer than routing the figures through one of the halves that is busy
/// doing something real-time.
#[derive(Clone)]
pub struct TelemetryHandle {
    shared: Arc<Shared>,
    config: ReceiverConfig,
}

impl TelemetryHandle {
    #[must_use]
    pub fn read(&self) -> Telemetry {
        telemetry(&self.shared, self.config)
    }
}

/// Builds a receiver, returning its two halves.
///
/// Returned as a pair rather than as one object with two sets of methods,
/// because the two halves are used from different threads and the type system
/// should say so.
impl Shared {
    /// Folds one arrived packet into the arrival meter.
    ///
    /// Pure arithmetic in registers plus two relaxed atomics, regardless of
    /// packet size — the network thread has a deadline too.
    fn meter_arrival(&self, samples: &[f32]) {
        let peak = samples.iter().fold(0.0_f32, |acc, &sample| acc.max(sample.abs()));
        let previous = f32::from_bits(self.arrival_peak_bits.load(Ordering::Relaxed));
        let decayed = previous * ARRIVAL_METER_RELEASE;
        self.arrival_peak_bits
            .store(peak.max(decayed).to_bits(), Ordering::Relaxed);
    }
}

#[must_use]
pub fn new(config: ReceiverConfig) -> (NetworkInput, AudioOutput) {
    let shared = Arc::new(Shared {
        buffer: JitterBuffer::new(config.frames_per_packet),
        arrival_peak_bits: AtomicU32::new(0),
        read_sequence: AtomicU32::new(0),
        start_sequence: AtomicU32::new(0),
        stream_epoch: AtomicU32::new(0),
        end_of_stream: AtomicBool::new(false),
        received: AtomicU64::new(0),
        lost: AtomicU64::new(0),
        duplicates: AtomicU64::new(0),
        reordered: AtomicU64::new(0),
        rejected: AtomicU64::new(0),
        keyless: AtomicU64::new(0),
        unauthentic: AtomicU64::new(0),
        concealed_runs: AtomicU64::new(0),
        concealed_samples: AtomicU64::new(0),
        resyncs: AtomicU64::new(0),
        clipped: AtomicU64::new(0),
        depth_frames: AtomicU64::new(0),
        correction_ppm_milli: AtomicU64::new(0),
        learned_ppm_milli: AtomicU64::new(0),
        drift_error_milliframes: AtomicU64::new(0),
        playing: AtomicBool::new(false),
    });

    (
        NetworkInput {
            shared: Arc::clone(&shared),
            tracker: SequenceTracker::new(),
            samples: vec![0.0; config.frames_per_packet],
            ciphertext: vec![0; config.frames_per_packet * 2 + audio_protocol::AEAD_TAG_BYTES],
            key: None,
            config,
        },
        AudioOutput::new(shared, config),
    )
}

// ---------------------------------------------------------------------------
// Network side
// ---------------------------------------------------------------------------

/// The socket thread's half.
pub struct NetworkInput {
    shared: Arc<Shared>,
    tracker: SequenceTracker,
    /// Decoded samples. Allocated once.
    samples: Vec<f32>,
    /// Scratch for opening a sealed payload in place. Allocated once: the
    /// network thread is not real-time, but 200 allocations a second is still
    /// 200 allocations a second.
    ciphertext: Vec<u8>,
    /// Opens sealed payloads. `None` means this receiver has no key, and
    /// encrypted packets are refused rather than played as noise.
    key: Option<audio_protocol::SessionKey>,
    config: ReceiverConfig,
}

impl NetworkInput {
    /// Files one datagram.
    ///
    /// `arrived_us` is a monotonic timestamp from the caller. Never fails and
    /// never panics: a receiver on a shared network sees stray traffic, and a
    /// performer's monitor must not depend on nobody else using the port.
    pub fn accept(&mut self, datagram: &[u8], arrived_us: u64) -> Accepted {
        let Ok((header, payload)) = parse_header(datagram) else {
            self.shared.rejected.fetch_add(1, Ordering::Relaxed);
            return Accepted::Rejected;
        };

        // An encrypted payload is opened here, on the network thread, so the
        // audio callback never does cryptography — and so a packet that fails
        // authentication is discarded before it can reach the jitter buffer.
        let payload = if header.flags.encrypted() {
            let Some(key) = &self.key else {
                // Honest refusal rather than playing ciphertext as audio,
                // which would be loud noise straight into somebody's ears.
                self.shared.keyless.fetch_add(1, Ordering::Relaxed);
                return Accepted::Encrypted;
            };
            if payload.len() > self.ciphertext.len() {
                self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                return Accepted::Rejected;
            }

            let scratch = &mut self.ciphertext[..payload.len()];
            scratch.copy_from_slice(payload);
            let header_bytes = &datagram[..audio_protocol::HEADER_BYTES];

            match key.open(header_bytes, scratch, header.sequence) {
                Ok(len) => &self.ciphertext[..len],
                Err(_) => {
                    // Altered, replayed under a different header, or sealed
                    // with another key. Counted and dropped: there is nothing
                    // safe to do with it, and on a venue network this is also
                    // what a forged end-of-stream marker looks like.
                    self.shared.unauthentic.fetch_add(1, Ordering::Relaxed);
                    self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                    return Accepted::Unauthentic;
                }
            }
        } else {
            payload
        };

        let verdict = self.tracker.observe(header.session_id, header.sequence);
        let new_stream = matches!(
            verdict,
            SequenceVerdict::First | SequenceVerdict::SessionChanged
        );

        if new_stream {
            // Audio from the previous session is not continuous with this one.
            self.shared.buffer.reset();
            self.shared.start_sequence.store(header.sequence, Ordering::Relaxed);
            self.shared.end_of_stream.store(false, Ordering::Relaxed);
            // Published last, with Release, so the audio callback cannot see
            // the new epoch before the start sequence that goes with it.
            self.shared.stream_epoch.fetch_add(1, Ordering::Release);
        }

        self.publish_sequence_stats();

        if header.flags.end_of_stream() {
            // Not stored: it carries no audio. The flag tells the audio
            // callback to drain and stop rather than wait out its buffer.
            self.shared.end_of_stream.store(true, Ordering::Release);
            return Accepted::EndOfStream;
        }

        if payload.len() / 2 != self.config.frames_per_packet {
            // A sender using a different packet duration. Refused rather than
            // played at the wrong length, which would be a timing glitch on
            // every packet.
            self.shared.rejected.fetch_add(1, Ordering::Relaxed);
            return Accepted::Rejected;
        }

        audio_core::convert::pcm_s16le_to_f32(payload, &mut self.samples);
        self.shared.meter_arrival(&self.samples);

        let reader_at = self.shared.read_sequence.load(Ordering::Relaxed);
        let outcome = self.shared.buffer.insert(
            header.sequence,
            header.flags.bits(),
            arrived_us,
            &self.samples,
            reader_at,
        );

        match outcome {
            crate::jitter::Insert::Stored if new_stream => Accepted::SessionStarted,
            crate::jitter::Insert::Stored => Accepted::Stored,
            crate::jitter::Insert::Duplicate => Accepted::Duplicate,
            crate::jitter::Insert::TooLate => Accepted::TooLate,
            crate::jitter::Insert::WrongSession | crate::jitter::Insert::WrongSize => {
                self.shared.rejected.fetch_add(1, Ordering::Relaxed);
                Accepted::Rejected
            }
        }
    }

    fn publish_sequence_stats(&self) {
        self.shared.received.store(self.tracker.received(), Ordering::Relaxed);
        self.shared.lost.store(self.tracker.lost(), Ordering::Relaxed);
        self.shared
            .duplicates
            .store(self.tracker.duplicates(), Ordering::Relaxed);
        self.shared
            .reordered
            .store(self.tracker.reordered(), Ordering::Relaxed);
    }

    /// Installs the key that opens sealed payloads.
    ///
    /// Called once the session id is known, because the key is derived from
    /// it: see `audio_protocol::SessionKey::derive`. Until it is set, an
    /// encrypted packet is refused rather than played as noise — which is
    /// what a receiver that joined before the stream started will see for its
    /// first few packets.
    pub fn set_session_key(&mut self, key: audio_protocol::SessionKey) {
        self.key = Some(key);
    }

    /// Whether this receiver can open sealed audio.
    #[must_use]
    pub const fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// A handle the control channel can keep, so it can report telemetry
    /// without borrowing either half.
    #[must_use]
    pub fn telemetry_handle(&self) -> TelemetryHandle {
        TelemetryHandle { shared: Arc::clone(&self.shared), config: self.config }
    }

    /// A snapshot for the telemetry report. Safe from any thread.
    #[must_use]
    pub fn telemetry(&self) -> Telemetry {
        telemetry(&self.shared, self.config)
    }
}

// ---------------------------------------------------------------------------
// Audio side
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Waiting for enough depth to start. Outputs silence.
    Priming,
    Playing,
    /// The sender sent an end-of-stream marker and the buffer has drained.
    Finished,
}

/// The audio callback's half.
pub struct AudioOutput {
    shared: Arc<Shared>,
    config: ReceiverConfig,
    state: State,
    /// Sequence to play next.
    sequence: u32,
    epoch: u32,
    concealer: Concealer,
    resampler: Resampler,
    drift: DriftController,
    chain: MonitorChain,
    /// One packet of scratch, for a real or concealed block.
    scratch: Vec<f32>,
    startup_packets: usize,
}

impl AudioOutput {
    fn new(shared: Arc<Shared>, config: ReceiverConfig) -> Self {
        let target = config.target_frames();
        Self {
            epoch: shared.stream_epoch.load(Ordering::Acquire),
            state: State::Priming,
            sequence: 0,
            concealer: Concealer::new(config.sample_rate),
            resampler: Resampler::new(config.frames_per_packet),
            drift: DriftController::new(DriftConfig::new(config.sample_rate, target)),
            chain: MonitorChain::new(config.sample_rate),
            scratch: vec![0.0; config.frames_per_packet],
            startup_packets: config.startup_packets(),
            shared,
            config,
        }
    }

    /// Installs monitor settings. Called from the application thread.
    pub fn apply_settings(&mut self, settings: MonitorSettings) {
        self.chain.apply(settings);
    }

    /// Changes the latency-against-resilience preset.
    ///
    /// The startup depth is recomputed, not just the drift target. It is
    /// derived from the preset and cached here, and the receiver returns to
    /// `Priming` on a new stream or a resync — so a stale value would prime
    /// the *next* stream to the depth of the previously selected preset.
    pub fn set_preset(&mut self, preset: Preset) {
        self.config.preset = preset;
        self.drift.set_target_frames(self.config.target_frames());
        self.startup_packets = self.config.startup_packets();
    }

    #[must_use]
    pub const fn preset(&self) -> Preset {
        self.config.preset
    }

    /// Whether audio is flowing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.state == State::Playing
    }

    /// Fills `out` with mono frames. **This is the audio callback.**
    ///
    /// Allocation-free, lock-free, and bounded: the work per call is
    /// proportional to `out.len()` with no data-dependent path that can take
    /// much longer than another. Nothing in here can block.
    pub fn render(&mut self, out: &mut [f32]) {
        self.check_for_new_stream();

        let dt = out.len() as f64 / f64::from(self.config.sample_rate.max(1));
        self.update_drift(dt);

        let mut produced = 0;
        while produced < out.len() {
            produced += self.resampler.fill(&mut out[produced..]);
            if produced == out.len() {
                break;
            }
            // The resampler is hungry and the output is not full: supply one
            // packet, real or concealed, and go round again.
            if !self.supply_packet() {
                // Nothing more will come. Silence the remainder rather than
                // looping: `Finished` is a stop, not a stall.
                out[produced..].fill(0.0);
                break;
            }
        }

        self.chain.process(out);
        self.publish();
    }

    /// Duplicates the mono mix to an interleaved stereo buffer.
    ///
    /// Both ears get the same signal. **Never synthesised stereo** — receiver
    /// obligation 5: the desktop sends one pre-mixed mono feed, and inventing a
    /// stereo image from it would place instruments where they are not.
    pub fn render_stereo(&mut self, interleaved: &mut [f32], mono_scratch: &mut Vec<f32>) {
        let frames = interleaved.len() / 2;
        if mono_scratch.len() != frames {
            mono_scratch.resize(frames, 0.0);
        }
        self.render(&mut mono_scratch[..frames]);
        for (frame, pair) in interleaved.chunks_exact_mut(2).enumerate() {
            pair[0] = mono_scratch[frame];
            pair[1] = mono_scratch[frame];
        }
    }

    /// Notices a new stream published by the network thread.
    fn check_for_new_stream(&mut self) {
        let epoch = self.shared.stream_epoch.load(Ordering::Acquire);
        if epoch == self.epoch {
            return;
        }
        self.epoch = epoch;
        self.sequence = self.shared.start_sequence.load(Ordering::Relaxed);
        self.state = State::Priming;
        self.resampler.reset();
        self.concealer.reset();
        self.drift.reset();
        self.chain.reset();
        self.shared.resyncs.fetch_add(1, Ordering::Relaxed);
    }

    fn update_drift(&mut self, dt: f64) {
        let depth = self.buffered_frames();
        if self.state == State::Playing {
            let ppm = self.drift.update(depth, dt);
            self.resampler.set_correction_ppm(ppm);
        }
        self.shared
            .depth_frames
            .store(depth as u64, Ordering::Relaxed);
    }

    /// Buffered depth: slots from the read position to the newest packet,
    /// holes included, plus whatever is already inside the resampler.
    ///
    /// Holes count because they are concealed rather than waited for, so each
    /// one still takes a packet interval to play. See
    /// [`JitterBuffer::span_from`].
    ///
    /// # Known limitation: arrival jitter biases this upwards
    ///
    /// The span reaches the *newest* packet present, so an early arrival
    /// raises it even though the audio in between has not arrived yet. Under
    /// 15 ms of jitter the harness measures the depth sitting about 7 ms above
    /// target, and the drift controller then holds roughly 460 ppm of
    /// correction trying to drain a buffer that is not really that deep —
    /// against clocks that are identical.
    ///
    /// The consequences are small but real: the pitch offset is 0.8 cents and
    /// inaudible, but the correction is pinned near its clamp with no headroom
    /// left for actual drift, and the rate the receiver reports becomes a
    /// false reading of ~90 ppm when the true difference is zero.
    ///
    /// The fix is to steer on a *windowed minimum* of the depth — how close
    /// the buffer came to starving over the last couple of seconds — which is
    /// immune to a single early packet. That belongs with the adaptive-depth
    /// work, which needs the same percentile machinery, rather than being
    /// bolted on here.
    fn buffered_frames(&self) -> usize {
        let packets = self.shared.buffer.span_from(self.sequence);
        packets * self.config.frames_per_packet + self.resampler.unconsumed()
    }

    /// Pushes one packet's worth of audio into the resampler.
    ///
    /// Returns false only when the stream has finished and there is nothing
    /// left to play.
    fn supply_packet(&mut self) -> bool {
        match self.state {
            State::Finished => return false,
            State::Priming => {
                let ready = self.shared.buffer.contiguous_from(self.sequence);
                if ready < self.startup_packets {
                    if self.drained_after_end_of_stream(ready) {
                        self.state = State::Finished;
                        return false;
                    }
                    // Not enough yet: silence, written directly rather than
                    // through the concealer. Waiting to start is not a
                    // dropout, and counting it as one would make every
                    // session look like it began with a fault.
                    self.scratch.fill(0.0);
                    self.resampler.push(&self.scratch);
                    return true;
                }
                self.state = State::Playing;
            }
            State::Playing => {}
        }

        if self.consider_resync() {
            // The read position moved, and everything downstream was reset.
            // This pass emits silence; the new position's audio fades in on
            // the next one.
            self.scratch.fill(0.0);
            self.resampler.push(&self.scratch);
            return true;
        }

        match self.shared.buffer.read(self.sequence, &mut self.scratch) {
            Some(ready) => {
                if PacketFlags::from_bits(ready.flags).has(PacketFlags::DISCONTINUITY) {
                    // The sender's capture glitched before this packet. Its
                    // audio is not continuous with the previous packet's, so
                    // the filters' memory of that audio is wrong.
                    self.concealer.reset();
                    self.chain.reset();
                    self.shared.resyncs.fetch_add(1, Ordering::Relaxed);
                }
                self.concealer.played(&mut self.scratch);
                self.resampler.push(&self.scratch);
                self.sequence = self.sequence.wrapping_add(1);
                true
            }
            None => {
                let ready = self.shared.buffer.contiguous_from(self.sequence);
                if self.drained_after_end_of_stream(ready) {
                    self.state = State::Finished;
                    return false;
                }

                self.concealer.conceal(&mut self.scratch);
                self.resampler.push(&self.scratch);

                // Advance **only** if there is audio beyond this point.
                //
                // With audio ahead, this packet is genuinely lost: conceal it
                // and move on, so the buffer keeps its depth rather than
                // losing a packet's worth on every gap.
                //
                // With nothing ahead, the sender has not got here yet — the
                // reader has caught up with the live edge, which is what a
                // drifting clock or an outage does. Advancing then would run
                // the read position away from the stream, and since the
                // position only ever moves forward it would never come back:
                // the buffer would read permanently empty and the receiver
                // would conceal for the rest of the session. Hold position and
                // wait instead; the next packet to arrive is the one wanted.
                if self.shared.buffer.highest_from(self.sequence).is_some() {
                    self.sequence = self.sequence.wrapping_add(1);
                }
                true
            }
        }
    }

    /// True when the sender has finished and nothing is left to play.
    fn drained_after_end_of_stream(&self, ready: usize) -> bool {
        ready == 0
            && self.shared.end_of_stream.load(Ordering::Acquire)
            && self.shared.buffer.highest_from(self.sequence).is_none()
    }

    /// Resyncs if continuing from the current position is worse than jumping.
    ///
    /// Returns true if the read position moved.
    fn consider_resync(&mut self) -> bool {
        let too_long = self.concealer.should_resync();
        let too_full = self.drift.excess_needs_skip();
        if !too_long && !too_full {
            return false;
        }

        let Some(highest) = self.shared.buffer.highest_from(self.sequence) else {
            // Nothing to jump to. Keep concealing; the stream may come back.
            return false;
        };

        // Land one *startup* depth behind the newest packet, not one target
        // depth. A resync drops back into `Priming`, so landing exactly where
        // priming is about to be satisfied lets playback resume at once;
        // landing on the target instead leaves it short by the startup margin
        // and it stalls until that much more audio arrives. Still close enough
        // to live that the latency is what the preset promised — the margin is
        // the same one every stream starts with, and it drains the same way.
        let startup_packets = self.startup_packets.max(1) as u32;
        let landing = highest.wrapping_sub(startup_packets.saturating_sub(1));

        if audio_protocol::sequence_delta(self.sequence, landing) <= 0 {
            // The newest packet is not ahead of us, so there is nothing to
            // gain. Concealing is still the right answer.
            return false;
        }

        self.sequence = landing;
        self.concealer.reset();
        self.resampler.reset();
        self.chain.reset();
        self.drift.resync();
        self.state = State::Priming;
        self.shared.resyncs.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn publish(&self) {
        self.shared
            .read_sequence
            .store(self.sequence, Ordering::Relaxed);
        self.shared
            .concealed_runs
            .store(self.concealer.concealed_runs(), Ordering::Relaxed);
        self.shared
            .concealed_samples
            .store(self.concealer.concealed_samples(), Ordering::Relaxed);
        self.shared
            .clipped
            .store(self.chain.clipped(), Ordering::Relaxed);
        self.shared
            .playing
            .store(self.state == State::Playing, Ordering::Relaxed);
        // Stored as milli-ppm in a u64 so the figures cross threads without a
        // lock and without an atomic float, which Rust does not have.
        let correction = (self.drift.correction_ppm() * 1_000.0) as i64;
        self.shared
            .correction_ppm_milli
            .store(correction as u64, Ordering::Relaxed);
        let learned = (self.drift.learned_ppm() * 1_000.0) as i64;
        self.shared
            .learned_ppm_milli
            .store(learned as u64, Ordering::Relaxed);
        let error = (self.drift.filtered_error_frames() * 1_000.0) as i64;
        self.shared
            .drift_error_milliframes
            .store(error as u64, Ordering::Relaxed);
    }

    /// A handle the control channel can keep, so it can report telemetry
    /// without borrowing either half.
    #[must_use]
    pub fn telemetry_handle(&self) -> TelemetryHandle {
        TelemetryHandle { shared: Arc::clone(&self.shared), config: self.config }
    }

    /// A snapshot for the telemetry report.
    #[must_use]
    pub fn telemetry(&self) -> Telemetry {
        telemetry(&self.shared, self.config)
    }
}

fn telemetry(shared: &Shared, config: ReceiverConfig) -> Telemetry {
    let received = shared.received.load(Ordering::Relaxed);
    let lost = shared.lost.load(Ordering::Relaxed);
    let offered = received + lost;
    let frames = shared.depth_frames.load(Ordering::Relaxed);
    let concealed = shared.concealed_samples.load(Ordering::Relaxed);
    let rate = f64::from(config.sample_rate.max(1));

    Telemetry {
        packets_received: received,
        packets_lost: lost,
        loss_percent: if offered == 0 {
            0.0
        } else {
            lost as f64 * 100.0 / offered as f64
        },
        duplicates: shared.duplicates.load(Ordering::Relaxed),
        reordered: shared.reordered.load(Ordering::Relaxed),
        rejected: shared.rejected.load(Ordering::Relaxed),
        keyless: shared.keyless.load(Ordering::Relaxed),
        unauthentic: shared.unauthentic.load(Ordering::Relaxed),
        arrival_peak: f32::from_bits(shared.arrival_peak_bits.load(Ordering::Relaxed)),
        buffer_health_ms: frames as f64 * 1_000.0 / rate,
        concealed_runs: shared.concealed_runs.load(Ordering::Relaxed),
        concealed_ms: concealed as f64 * 1_000.0 / rate,
        resyncs: shared.resyncs.load(Ordering::Relaxed),
        resample_ratio_ppm: shared.learned_ppm_milli.load(Ordering::Relaxed) as i64 as f64
            / 1_000.0,
        instant_correction_ppm: shared.correction_ppm_milli.load(Ordering::Relaxed) as i64 as f64
            / 1_000.0,
        drift_error_frames: shared.drift_error_milliframes.load(Ordering::Relaxed) as i64 as f64
            / 1_000.0,
        clipped: shared.clipped.load(Ordering::Relaxed),
        playing: shared.playing.load(Ordering::Relaxed),
        end_of_stream: shared.end_of_stream.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_protocol::{AudioPacketHeader, HEADER_BYTES, encode_header};

    const RATE: u32 = 48_000;
    const FRAMES: usize = 240;
    const SESSION: u32 = 0x5EED;

    fn config() -> ReceiverConfig {
        ReceiverConfig::new(RATE, FRAMES)
    }

    /// Builds a real StageLink datagram carrying a constant value, so a
    /// rendered block can be traced back to the packet it came from.
    fn packet(session: u32, sequence: u32, value: f32, flags: u8) -> Vec<u8> {
        let mut datagram = vec![0_u8; HEADER_BYTES + FRAMES * 2];
        let header = AudioPacketHeader::new(
            session,
            sequence,
            u64::from(sequence) * 5_000,
            PacketFlags::from_bits(flags),
        );
        encode_header(&header, &mut datagram).unwrap();
        let samples = vec![value; FRAMES];
        audio_core::convert::f32_to_pcm_s16le(&samples, &mut datagram[HEADER_BYTES..]);
        datagram
    }

    /// The meter reads the mix as it *arrives*, not as the performer hears it.
    ///
    /// That distinction is the whole point: muted, or with the volume at the
    /// bottom, somebody needs to be able to see that the desktop is still
    /// sending them audio.
    #[test]
    fn the_arrival_meter_follows_the_incoming_signal_not_the_output() {
        let (mut net, mut audio) = new(config());
        audio.apply_settings(MonitorSettings { muted: true, ..MonitorSettings::default() });

        assert_eq!(net.telemetry().arrival_peak, 0.0, "silence before anything arrives");

        for sequence in 0..20 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        let arriving = net.telemetry().arrival_peak;
        assert!(
            (arriving - 0.5).abs() < 0.01,
            "the meter should read the arriving 0.5, got {arriving}"
        );

        // Muted, so nothing is coming out — and the meter still reads the feed.
        let mut block = vec![0.0_f32; FRAMES];
        for _ in 0..20 {
            audio.render(&mut block);
        }
        assert_eq!(peak(&block), 0.0, "muted output must be silent");
        assert!(net.telemetry().arrival_peak > 0.4, "the meter must not follow the output");
    }

    /// It has to fall when the mix stops, or it reads like a stuck needle.
    #[test]
    fn the_arrival_meter_releases_when_the_mix_goes_quiet() {
        let (mut net, _audio) = new(config());
        for sequence in 0..20 {
            net.accept(&packet(SESSION, sequence, 0.9, 0), 0);
        }
        let loud = net.telemetry().arrival_peak;

        // A second of digital silence, at 200 packets per second.
        for sequence in 20..220 {
            net.accept(&packet(SESSION, sequence, 0.0, 0), 0);
        }
        let quiet = net.telemetry().arrival_peak;
        assert!(quiet < loud * 0.2, "expected a clear fall from {loud}, got {quiet}");
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()))
    }

    /// Renders `blocks` blocks of `FRAMES` frames, returning everything.
    fn render_blocks(audio: &mut AudioOutput, blocks: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut block = vec![0.0_f32; FRAMES];
        for _ in 0..blocks {
            audio.render(&mut block);
            out.extend_from_slice(&block);
        }
        out
    }

    #[test]
    fn nothing_plays_before_any_audio_arrives() {
        let (_net, mut audio) = new(config());
        let out = render_blocks(&mut audio, 10);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "silence is the only honest output with no packets"
        );
        assert!(!audio.is_playing());
        assert_eq!(audio.telemetry().packets_received, 0);
    }

    #[test]
    fn a_clean_stream_plays_through_at_its_own_level() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), u64::from(sequence) * 5_000);
        }

        let out = render_blocks(&mut audio, 30);
        assert!(audio.is_playing());

        // Past the startup fill and the fade-in, the signal is at its level.
        let steady = &out[2_000..];
        assert!(
            (peak(steady) - 0.5).abs() < 0.02,
            "level was {}, expected 0.5",
            peak(steady)
        );

        let telemetry = audio.telemetry();
        assert_eq!(telemetry.packets_lost, 0);
        assert_eq!(telemetry.clipped, 0);
        assert!(telemetry.playing);
    }

    #[test]
    fn playback_waits_for_the_startup_depth_rather_than_starting_on_one_packet() {
        let config = config();
        // Asserted against the configured depth rather than a hard-coded
        // packet count, so this keeps testing the behaviour and not a number
        // that has to be chased every time the presets are tuned.
        let needed = config.startup_packets();
        assert!(needed > 1, "the test proves nothing if one packet is enough");

        let (mut net, mut audio) = new(config);
        net.accept(&packet(SESSION, 0, 0.5, 0), 0);
        render_blocks(&mut audio, 2);
        assert!(!audio.is_playing(), "started on a single packet");

        // One short of the startup depth still must not start.
        for sequence in 1..needed as u32 - 1 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 2);
        assert!(
            !audio.is_playing(),
            "started on {} packets, one short of the {needed} required",
            needed - 1
        );

        net.accept(&packet(SESSION, needed as u32 - 1, 0.5, 0), 0);
        render_blocks(&mut audio, 2);
        assert!(audio.is_playing(), "{needed} packets of depth should start playback");
    }

    /// Every preset must start at the depth it promises — never below it, or
    /// the promise is false for as long as the drift controller takes to make
    /// it true, which at the steady clamp is tens of seconds. The old absolute
    /// 30 ms started Safe 50 ms short.
    #[test]
    fn every_preset_starts_at_the_depth_it_promises() {
        for preset in [Preset::Tight, Preset::Normal, Preset::Safe] {
            let config = ReceiverConfig { preset, ..config() };
            assert!(
                config.startup_ms() >= preset.target_ms(),
                "{preset:?} starts at {} ms against a {} ms target",
                config.startup_ms(),
                preset.target_ms()
            );
            // The relationship is the same for all three: whatever the margin
            // is, every preset gets the same one rather than the sign of the
            // correction depending on which preset was chosen.
            assert!(
                (config.startup_ms() - preset.target_ms() - DEFAULT_STARTUP_MARGIN_MS).abs()
                    < 1e-3,
                "{preset:?} does not use the common margin"
            );
        }
    }

    /// Priming demands its depth as a gap-free run, which is what makes a zero
    /// margin safe: a freshly started stream has at least as much real runway
    /// as one that has been playing for an hour, where the depth being held is
    /// the span and counts holes.
    #[test]
    fn the_startup_depth_is_required_gap_free() {
        let config = config();
        let needed = config.startup_packets();
        let (mut net, mut audio) = new(config);

        // One short of the requirement, with a hole in the middle. The span
        // reaches far enough; the contiguous run does not.
        for sequence in 0..needed as u32 + 4 {
            if sequence == 2 {
                continue;
            }
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 2);
        assert!(
            !audio.is_playing(),
            "started on a run with a hole in it, which conceals from the first moment"
        );
    }

    #[test]
    fn changing_the_preset_moves_the_startup_depth_with_it() {
        let (_net, mut audio) = new(ReceiverConfig { preset: Preset::Tight, ..config() });
        let tight = audio.startup_packets;

        audio.set_preset(Preset::Safe);
        let safe = audio.startup_packets;

        // Cached on the audio thread and consulted again whenever the receiver
        // re-primes, so a stale value would prime the next stream to the depth
        // of the preset the listener used to have.
        assert!(
            safe > tight,
            "Safe must prime deeper than Tight: got {safe} against {tight} packets"
        );
    }

    #[test]
    fn a_lost_packet_is_concealed_and_the_stream_stays_in_step() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            if sequence == 20 {
                continue; // lost in transit
            }
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }

        let out = render_blocks(&mut audio, 30);
        assert!(out.iter().all(|s| s.is_finite()));

        let telemetry = audio.telemetry();
        assert_eq!(telemetry.packets_lost, 1, "the gap must be counted");
        assert_eq!(telemetry.concealed_runs, 1);
        // And the audio after the gap is the real audio, at its real level:
        // the stream did not slip out of step.
        assert!((peak(&out[out.len() - 480..]) - 0.5).abs() < 0.05);
    }

    /// The property that makes a buffer survive repeated loss: concealing a
    /// gap must not cost depth. If each gap shortened the buffer, the stream
    /// would underrun shortly after a lossy patch.
    #[test]
    fn concealing_a_gap_does_not_consume_buffer_depth() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..60 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 5);
        let before = audio.telemetry().buffer_health_ms;
        assert!(before > 0.0, "nothing was buffered to begin with");

        // A later stretch with every third packet missing, rendered one block
        // per packet arriving — which is the real cadence, and the only one
        // under which "depth is preserved" means anything.
        let mut block = vec![0.0_f32; FRAMES];
        for sequence in 60..120 {
            if sequence % 3 != 0 {
                net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
            }
            audio.render(&mut block);
        }

        let after = audio.telemetry().buffer_health_ms;
        // 60 blocks rendered against 40 packets arriving, so depth is expected
        // to fall — but by the 20 packets that never came, not by the 20 gaps
        // *and* the concealment. If concealing cost depth, it would have
        // collapsed to nothing.
        assert!(
            after > 50.0,
            "depth collapsed from {before} ms to {after} ms across a lossy stretch"
        );
        assert!(audio.telemetry().concealed_runs > 0, "nothing was concealed");
    }

    #[test]
    fn a_duplicate_is_discarded_rather_than_played_twice() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
            // Every packet sent twice, as a retransmitting access point does.
            assert_eq!(
                net.accept(&packet(SESSION, sequence, 0.5, 0), 0),
                Accepted::Duplicate
            );
        }
        render_blocks(&mut audio, 20);
        assert!(audio.telemetry().duplicates >= 39);
        assert_eq!(audio.telemetry().packets_lost, 0);
    }

    #[test]
    fn a_reordered_packet_is_played_in_its_own_place() {
        let (mut net, mut audio) = new(config());
        // Values encode the sequence, so misplacement is visible in the audio.
        let order = [0_u32, 1, 2, 3, 4, 5, 6, 9, 7, 8, 10, 11, 12, 13, 14, 15];
        for sequence in order {
            net.accept(&packet(SESSION, sequence, sequence as f32 / 100.0, 0), 0);
        }

        let out = render_blocks(&mut audio, 16);
        // Find the block that played packet 7 and check 8 follows it.
        let value_at = |packet_index: usize| {
            let start = packet_index * FRAMES + FRAMES / 2;
            out.get(start).copied().unwrap_or(0.0)
        };
        let mut seen = Vec::new();
        for index in 0..12 {
            let value = value_at(index);
            if value > 0.0 {
                seen.push((value * 100.0).round() as u32);
            }
        }
        assert!(
            seen.windows(2).all(|pair| pair[1] >= pair[0]),
            "packets played out of order: {seen:?}"
        );
        assert_eq!(audio.telemetry().packets_lost, 0, "a late packet is not a loss");
    }

    #[test]
    fn a_session_change_flushes_the_old_audio() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 5);
        assert!(audio.is_playing());

        // A new session: different id, sequence back to zero, different audio.
        assert_eq!(
            net.accept(&packet(0xBEEF, 0, -0.25, 0), 0),
            Accepted::SessionStarted
        );
        for sequence in 1..40 {
            net.accept(&packet(0xBEEF, sequence, -0.25, 0), 0);
        }

        let out = render_blocks(&mut audio, 30);
        // Nothing from the old session may be heard after the change, and the
        // old audio was +0.5 while the new is -0.25.
        let tail = &out[out.len() - 480..];
        assert!(
            tail.iter().all(|&s| s <= 0.01),
            "audio from the previous session leaked through"
        );
        assert!((peak(tail) - 0.25).abs() < 0.02);
        assert!(audio.telemetry().resyncs >= 1);
    }

    #[test]
    fn an_end_of_stream_marker_stops_playback_once_the_buffer_drains() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..20 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        // The marker carries no audio, so it is not stored.
        let mut marker = vec![0_u8; HEADER_BYTES];
        let header = AudioPacketHeader::new(
            SESSION,
            20,
            100_000,
            PacketFlags::empty().with(PacketFlags::END_OF_STREAM),
        );
        encode_header(&header, &mut marker).unwrap();
        assert_eq!(net.accept(&marker, 0), Accepted::EndOfStream);

        // Rendering far past the buffered audio: it plays out and then stops,
        // rather than concealing indefinitely.
        let out = render_blocks(&mut audio, 60);
        assert!(audio.telemetry().end_of_stream);
        assert!(
            out[out.len() - 480..].iter().all(|&s| s == 0.0),
            "playback did not stop after the stream ended"
        );
        assert!(!audio.is_playing());
    }

    #[test]
    fn a_gap_too_long_to_conceal_resyncs_to_the_newest_audio() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 10);
        let resyncs_before = audio.telemetry().resyncs;

        // A 300 ms hole, then the stream resumes with different audio. 300 ms
        // is 60 packets at 5 ms.
        for sequence in 100..160 {
            net.accept(&packet(SESSION, sequence, -0.4, 0), 0);
        }

        let out = render_blocks(&mut audio, 120);
        assert!(
            audio.telemetry().resyncs > resyncs_before,
            "a 300 ms gap should have resynced rather than concealed through it"
        );

        // It reached the newer audio rather than staying stuck in the hole.
        // Measured as "somewhere in the run", not at the end: by block 120 the
        // 60 packets that arrived have all played out and the correct output is
        // silence again.
        let reached = out
            .chunks(FRAMES)
            .any(|block| (peak(block) - 0.4).abs() < 0.05);
        assert!(reached, "the audio after the gap was never played");

        // And none of the *old* audio was replayed after the resync: 0.5 only
        // appears before the gap.
        let last_old = out
            .chunks(FRAMES)
            .rposition(|block| peak(block) > 0.45)
            .unwrap_or(0);
        let first_new = out
            .chunks(FRAMES)
            .position(|block| (peak(block) - 0.4).abs() < 0.05)
            .unwrap_or(usize::MAX);
        assert!(
            last_old < first_new,
            "old audio at block {last_old} came after new audio at block {first_new}"
        );
    }

    /// A backlog this large cannot be resampled away in reasonable time, so
    /// the receiver must skip forward and get the latency back.
    #[test]
    fn a_large_backlog_is_skipped_rather_than_resampled_away() {
        let (mut net, mut audio) = new(config());
        // 500 ms of audio, far beyond the 40 ms target.
        for sequence in 0..100 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        // Render long enough for the 2 s drift filter to believe the depth.
        render_blocks(&mut audio, 500);

        let telemetry = audio.telemetry();
        assert!(
            telemetry.resyncs >= 1,
            "the backlog was never retired: depth {} ms",
            telemetry.buffer_health_ms
        );
        assert!(
            telemetry.buffer_health_ms < 200.0,
            "still {} ms of latency after the skip",
            telemetry.buffer_health_ms
        );
    }

    #[test]
    fn a_discontinuity_flag_resyncs_the_filters() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            let flags = if sequence == 20 {
                PacketFlags::DISCONTINUITY
            } else {
                0
            };
            net.accept(&packet(SESSION, sequence, 0.5, flags), 0);
        }
        let out = render_blocks(&mut audio, 30);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(
            audio.telemetry().resyncs >= 1,
            "a DISCONTINUITY must be acted on, not merely carried"
        );
    }

    #[test]
    fn stray_traffic_on_the_port_is_counted_and_ignored() {
        let (mut net, mut audio) = new(config());
        for junk in [
            b"GET / HTTP/1.1\r\n\r\n".to_vec(),
            vec![0_u8; 3],
            vec![0xFF_u8; 1_400],
            Vec::new(),
        ] {
            assert_eq!(net.accept(&junk, 0), Accepted::Rejected);
        }
        // And real audio still works afterwards.
        for sequence in 0..40 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        let out = render_blocks(&mut audio, 20);
        assert!(peak(&out[2_000..]) > 0.4);
        assert_eq!(audio.telemetry().rejected, 4);
    }

    #[test]
    fn an_encrypted_packet_is_refused_rather_than_played_as_noise() {
        let (mut net, mut audio) = new(config());
        let encrypted = packet(SESSION, 0, 0.5, PacketFlags::ENCRYPTED);
        assert_eq!(net.accept(&encrypted, 0), Accepted::Encrypted);

        let out = render_blocks(&mut audio, 10);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "ciphertext must never reach the DAC"
        );
    }

    #[test]
    fn a_packet_of_the_wrong_length_is_refused() {
        let (mut net, _audio) = new(config());
        // A sender using 2.5 ms packets against a 5 ms receiver.
        let mut datagram = vec![0_u8; HEADER_BYTES + 120 * 2];
        let header = AudioPacketHeader::new(SESSION, 0, 0, PacketFlags::empty());
        encode_header(&header, &mut datagram).unwrap();
        assert_eq!(net.accept(&datagram, 0), Accepted::Rejected);
    }

    #[test]
    fn the_mono_mix_is_duplicated_to_both_ears_never_widened() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..40 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        let mut interleaved = vec![0.0_f32; FRAMES * 2];
        let mut scratch = Vec::new();
        for _ in 0..20 {
            audio.render_stereo(&mut interleaved, &mut scratch);
        }
        for pair in interleaved.chunks_exact(2) {
            assert_eq!(pair[0], pair[1], "the two ears must receive the same signal");
        }
        assert!(peak(&interleaved) > 0.4);
    }

    #[test]
    fn monitor_settings_reach_the_output() {
        let (mut net, mut audio) = new(config());
        for sequence in 0..60 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 10);

        audio.apply_settings(MonitorSettings { muted: true, ..MonitorSettings::default() });
        let out = render_blocks(&mut audio, 10);
        assert!(
            peak(&out[1_000..]) < 1e-5,
            "mute did not reach the output: {}",
            peak(&out[1_000..])
        );
    }

    #[test]
    fn a_preset_change_moves_the_drift_target() {
        let (_net, mut audio) = new(config());
        assert_eq!(audio.preset(), Preset::Normal);
        audio.set_preset(Preset::Safe);
        assert_eq!(audio.preset(), Preset::Safe);
        // 80 ms at 48 kHz.
        assert_eq!(audio.drift.target_frames(), 3_840);
        audio.set_preset(Preset::Tight);
        assert_eq!(audio.drift.target_frames(), 960);
    }

    /// The two halves are used from two threads in production, so they must be
    /// usable from two threads here.
    #[test]
    fn the_two_halves_run_on_separate_threads() {
        let (mut net, mut audio) = new(config());

        // Enough packets to start playing, filed before either thread exists.
        //
        // Without this the test is a race: the audio thread is not throttled
        // by a real DAC, so it can finish its renders before the network
        // thread has filed anything and then hear nothing. That is a timing
        // artefact of the test, not a property of the code — and a test that
        // fails when an unrelated change shifts the timing by a microsecond
        // is worse than no test.
        for sequence in 0..40_u32 {
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }

        let network = std::thread::spawn(move || {
            for sequence in 40..2_000_u32 {
                net.accept(&packet(SESSION, sequence, 0.5, 0), u64::from(sequence) * 5_000);
            }
            net.telemetry()
        });

        let rendering = std::thread::spawn(move || {
            let mut block = vec![0.0_f32; FRAMES];
            let mut heard = 0;
            // Deliberately fewer renders than there are packets: the audio
            // thread is not throttled by a real DAC here, so it would
            // otherwise outrun the network thread and spend the test
            // concealing.
            for _ in 0..1_000 {
                audio.render(&mut block);
                assert!(
                    block.iter().all(|s| s.is_finite()),
                    "a non-finite sample reached the output"
                );
                assert!(
                    block.iter().all(|&s| s.abs() <= 1.0),
                    "a sample escaped the safety clip"
                );
                if peak(&block) > 0.4 {
                    heard += 1;
                }
            }
            (heard, audio.telemetry())
        });

        let net_telemetry = network.join().unwrap();
        let (heard, audio_telemetry) = rendering.join().unwrap();

        assert!(net_telemetry.packets_received > 1_900);
        // The bar is low on purpose: the two threads race, and how much the
        // audio thread hears depends on who wins. What must hold regardless is
        // that real audio got through at all and that nothing unsafe did.
        assert!(heard > 10, "the audio thread heard almost nothing: {heard} blocks");
        assert_eq!(audio_telemetry.clipped, 0);
    }

    /// Obligation 2: the receiver's loss figures must agree with the sender's.
    /// They do because both sides run the same `sequence_delta`.
    #[test]
    fn loss_accounting_matches_what_was_actually_dropped() {
        let (mut net, mut audio) = new(config());
        let dropped: Vec<u32> = (0..500).filter(|s| s % 17 == 0 && *s > 0).collect();
        for sequence in 0..500 {
            if dropped.contains(&sequence) {
                continue;
            }
            net.accept(&packet(SESSION, sequence, 0.5, 0), 0);
        }
        render_blocks(&mut audio, 200);

        let telemetry = audio.telemetry();
        assert_eq!(
            telemetry.packets_lost,
            dropped.len() as u64,
            "receiver counted {} losses, {} were really dropped",
            telemetry.packets_lost,
            dropped.len()
        );
        let expected = dropped.len() as f64 * 100.0 / 500.0;
        assert!((telemetry.loss_percent - expected).abs() < 0.01);
    }
}

#[cfg(test)]
mod crypto_tests {
    use super::*;
    use audio_protocol::{AudioPacketHeader, HEADER_BYTES, SessionKey, encode_header};

    const RATE: u32 = 48_000;
    const FRAMES: usize = 240;
    const SESSION: u32 = 0x5EED;
    const TOKEN: &str = "9f2c3d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f";

    /// A sealed packet, built exactly as the sender builds one.
    fn sealed(key: &SessionKey, sequence: u32, value: f32) -> Vec<u8> {
        let payload_len = FRAMES * 2;
        let mut datagram =
            vec![0_u8; HEADER_BYTES + payload_len + audio_protocol::AEAD_TAG_BYTES];
        let header = AudioPacketHeader::new(
            SESSION,
            sequence,
            u64::from(sequence) * 5_000,
            PacketFlags::empty().with(PacketFlags::ENCRYPTED),
        );
        encode_header(&header, &mut datagram).unwrap();
        let samples = vec![value; FRAMES];
        audio_core::convert::f32_to_pcm_s16le(&samples, &mut datagram[HEADER_BYTES..]);
        let total = key.seal(&mut datagram, payload_len, sequence).unwrap();
        datagram.truncate(total);
        datagram
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()))
    }

    #[test]
    fn an_encrypted_stream_plays_when_the_receiver_has_the_key() {
        let (mut net, mut audio) =
            new(ReceiverConfig::new(RATE, FRAMES));
        net.set_session_key(SessionKey::derive(TOKEN, SESSION));

        let key = SessionKey::derive(TOKEN, SESSION);
        for sequence in 0..40 {
            assert_eq!(
                net.accept(&sealed(&key, sequence, 0.5), 0),
                if sequence == 0 { Accepted::SessionStarted } else { Accepted::Stored }
            );
        }

        let mut block = vec![0.0_f32; FRAMES];
        let mut heard = 0.0_f32;
        for _ in 0..30 {
            audio.render(&mut block);
            heard = heard.max(peak(&block));
        }

        assert!((heard - 0.5).abs() < 0.02, "encrypted audio did not decode: {heard}");
        assert_eq!(audio.telemetry().unauthentic, 0);
        assert_eq!(audio.telemetry().packets_lost, 0);
    }

    /// Without a key, ciphertext must never reach the DAC: it would be loud
    /// noise straight into somebody's ears.
    #[test]
    fn without_a_key_encrypted_packets_are_refused_rather_than_played() {
        let (mut net, mut audio) = new(ReceiverConfig::new(RATE, FRAMES));
        assert!(!net.has_key());

        let key = SessionKey::derive(TOKEN, SESSION);
        for sequence in 0..20 {
            assert_eq!(net.accept(&sealed(&key, sequence, 0.5), 0), Accepted::Encrypted);
        }

        let mut block = vec![0.0_f32; FRAMES];
        for _ in 0..20 {
            audio.render(&mut block);
            assert!(block.iter().all(|&s| s == 0.0), "ciphertext reached the DAC");
        }
    }

    /// The forgery the plaintext transport allows: anyone on the venue
    /// network sends an end-of-stream marker and silences every performer.
    #[test]
    fn a_packet_sealed_with_the_wrong_key_is_counted_and_discarded() {
        let (mut net, mut audio) = new(ReceiverConfig::new(RATE, FRAMES));
        net.set_session_key(SessionKey::derive(TOKEN, SESSION));

        let attacker = SessionKey::derive(&"0".repeat(64), SESSION);
        for sequence in 0..10 {
            assert_eq!(
                net.accept(&sealed(&attacker, sequence, 1.0), 0),
                Accepted::Unauthentic
            );
        }

        let mut block = vec![0.0_f32; FRAMES];
        for _ in 0..10 {
            audio.render(&mut block);
            assert!(block.iter().all(|&s| s == 0.0), "forged audio was played");
        }
        assert_eq!(audio.telemetry().unauthentic, 10);
    }

    #[test]
    fn a_tampered_packet_is_rejected() {
        let (mut net, _audio) = new(ReceiverConfig::new(RATE, FRAMES));
        net.set_session_key(SessionKey::derive(TOKEN, SESSION));

        let key = SessionKey::derive(TOKEN, SESSION);
        let mut packet = sealed(&key, 0, 0.5);
        packet[HEADER_BYTES + 20] ^= 0x01;
        assert_eq!(net.accept(&packet, 0), Accepted::Unauthentic);
    }

    /// The header travels in the clear but is bound in as associated data, so
    /// renumbering a packet to play it in the wrong place is detected.
    #[test]
    fn a_renumbered_packet_is_rejected_even_though_the_header_is_readable() {
        let (mut net, _audio) = new(ReceiverConfig::new(RATE, FRAMES));
        net.set_session_key(SessionKey::derive(TOKEN, SESSION));

        let key = SessionKey::derive(TOKEN, SESSION);
        let mut packet = sealed(&key, 5, 0.5);
        // Rewrite the sequence number in the header.
        packet[12..16].copy_from_slice(&9_u32.to_le_bytes());
        assert_eq!(net.accept(&packet, 0), Accepted::Unauthentic);
    }

    #[test]
    fn a_key_for_another_session_does_not_open_this_one() {
        let (mut net, _audio) = new(ReceiverConfig::new(RATE, FRAMES));
        // Derived for the wrong session, which is what a receiver that missed
        // a session change would have.
        net.set_session_key(SessionKey::derive(TOKEN, SESSION + 1));

        let key = SessionKey::derive(TOKEN, SESSION);
        assert_eq!(net.accept(&sealed(&key, 0, 0.5), 0), Accepted::Unauthentic);
    }
}

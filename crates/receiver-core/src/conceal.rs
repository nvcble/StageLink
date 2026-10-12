//! Packet-loss concealment.
//!
//! When a packet does not arrive, something has to come out of the DAC anyway.
//! The choice is between silence, a repeat, and a click, and the click is what
//! you get by accident.
//!
//! ## What this does
//!
//! On a gap: replay the last few milliseconds of real audio **time-reversed**,
//! faded to zero over 5 ms, then silence. On the return of real audio: fade it
//! in over 5 ms.
//!
//! Time-reversal is the trick worth explaining. The tail buffer holds the last
//! 5 ms of audio, so its *final* sample is the last sample the DAC produced.
//! Playing the tail backwards therefore starts at exactly that value — the
//! splice is continuous, with no step and so no click — while preserving the
//! signal's spectrum, because reversing a waveform does not change its
//! magnitude spectrum. Replaying it forwards would start 5 ms back in time and
//! step the waveform by however much it had moved in between.
//!
//! The result sounds like a soft dropout. That is the intended outcome: a
//! dropout is information the performer can act on, where a click is just
//! painful.
//!
//! ## What this deliberately does not do
//!
//! Pitch-synchronous repetition (PSOLA) sounds markedly better for gaps longer
//! than one packet, and is the planned upgrade. It is not here yet because it
//! needs a pitch estimate, and choosing between implementations of it without a
//! harness that can measure the difference would be guessing.

/// Fade length, in milliseconds. Long enough to be inaudible as a click, short
/// enough that it is over before the next packet is due.
pub const FADE_MS: f32 = 5.0;

/// How long concealment may run before the caller should resync instead.
///
/// Beyond this the buffer has lost so much that continuing to conceal just
/// delays the inevitable, and a faded resync gets the stream back in step.
pub const MAX_CONCEAL_MS: f32 = 200.0;

/// Loss concealment for one stream.
pub struct Concealer {
    /// Last `fade` samples of real audio, oldest first.
    tail: Vec<f32>,
    /// Samples of fade.
    fade: usize,
    /// Samples of concealment produced since real audio stopped.
    concealed_run: usize,
    /// Samples of fade-in still owed to returning audio.
    fade_in_remaining: usize,
    /// Nothing real has been played yet, so there is no tail to replay.
    primed: bool,
    concealed_samples: u64,
    concealed_runs: u64,
    sample_rate: f32,
}

impl Concealer {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let fade = ((FADE_MS / 1_000.0) * sample_rate as f32).round().max(1.0) as usize;
        Self {
            tail: vec![0.0; fade],
            fade,
            concealed_run: 0,
            fade_in_remaining: 0,
            primed: false,
            concealed_samples: 0,
            concealed_runs: 0,
            sample_rate: sample_rate as f32,
        }
    }

    /// Real audio is being played. Applies any owed fade-in and remembers the
    /// tail.
    ///
    /// Must be called for every block of real audio, in order, or the tail will
    /// not be the audio that actually reached the DAC and the next concealment
    /// will splice onto the wrong place.
    pub fn played(&mut self, samples: &mut [f32]) {
        if self.fade_in_remaining > 0 {
            for sample in samples.iter_mut() {
                if self.fade_in_remaining == 0 {
                    break;
                }
                // Ramps 0 -> 1 as `remaining` counts down to zero.
                let gain = 1.0 - self.fade_in_remaining as f32 / self.fade as f32;
                *sample *= gain;
                self.fade_in_remaining -= 1;
            }
        }

        self.concealed_run = 0;
        self.remember(samples);
        self.primed = true;
    }

    /// Fills `out` with concealment audio.
    pub fn conceal(&mut self, out: &mut [f32]) {
        if self.concealed_run == 0 {
            self.concealed_runs += 1;
            // The returning audio must come back gently, however short the gap.
            self.fade_in_remaining = self.fade;
        }

        for (offset, sample) in out.iter_mut().enumerate() {
            let position = self.concealed_run + offset;
            *sample = if !self.primed || position >= self.fade {
                // Past the fade, or nothing real has ever played: silence is
                // the honest output. Repeating further would invent audio.
                0.0
            } else {
                // Reversed tail, so position 0 continues from the last real
                // sample exactly, with a linear fade to zero.
                let gain = 1.0 - position as f32 / self.fade as f32;
                self.tail[self.fade - 1 - position] * gain
            };
        }

        self.concealed_run += out.len();
        self.concealed_samples += out.len() as u64;
    }

    /// Discards history. Used on a session change or a resync, where the audio
    /// before and after are not continuous and splicing them would be wrong.
    pub fn reset(&mut self) {
        self.tail.fill(0.0);
        self.primed = false;
        self.concealed_run = 0;
        // Returning audio still fades in: a resync arrives from silence, and a
        // full-level start would click just as much as a bad splice.
        self.fade_in_remaining = self.fade;
    }

    /// Milliseconds of concealment in the current run.
    #[must_use]
    pub fn concealed_ms(&self) -> f32 {
        self.concealed_run as f32 * 1_000.0 / self.sample_rate
    }

    /// Whether the gap has gone on so long that a resync is the better answer.
    #[must_use]
    pub fn should_resync(&self) -> bool {
        self.concealed_ms() >= MAX_CONCEAL_MS
    }

    #[must_use]
    pub const fn concealed_samples(&self) -> u64 {
        self.concealed_samples
    }

    /// Number of separate gaps concealed. More informative than the sample
    /// count: fifty short gaps and one long one sound nothing alike.
    #[must_use]
    pub const fn concealed_runs(&self) -> u64 {
        self.concealed_runs
    }

    fn remember(&mut self, samples: &[f32]) {
        if samples.len() >= self.fade {
            self.tail.copy_from_slice(&samples[samples.len() - self.fade..]);
        } else {
            // Shift the existing tail down and append.
            self.tail.copy_within(samples.len().., 0);
            let keep = self.fade - samples.len();
            self.tail[keep..].copy_from_slice(samples);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const FADE: usize = 240; // 5 ms at 48 kHz

    fn tone(frequency: f32, frames: usize, offset: usize) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let t = (frame + offset) as f32 / RATE as f32;
                (core::f32::consts::TAU * frequency * t).sin() * 0.5
            })
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()))
    }

    /// The largest step between consecutive samples. A click is a step, so
    /// this is the measurement that matters at a splice.
    fn largest_step(samples: &[f32]) -> f32 {
        samples
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0_f32, f32::max)
    }

    #[test]
    fn the_fade_is_five_milliseconds() {
        let concealer = Concealer::new(RATE);
        assert_eq!(concealer.fade, FADE);
        assert_eq!(Concealer::new(44_100).fade, 221);
    }

    /// The whole reason for reversing the tail: the first concealed sample
    /// continues from the last real one, so there is no step and no click.
    #[test]
    fn concealment_splices_onto_the_last_real_sample_without_a_step() {
        let mut concealer = Concealer::new(RATE);
        let mut real = tone(1_000.0, 480, 0);
        concealer.played(&mut real);

        let mut concealed = vec![0.0_f32; 480];
        concealer.conceal(&mut concealed);

        let last_real = real[real.len() - 1];
        assert!(
            (concealed[0] - last_real).abs() < 1e-6,
            "the splice stepped from {last_real} to {}",
            concealed[0]
        );

        // And the step across the whole concealed block is no larger than the
        // steps within the real audio it continues.
        let within_real = largest_step(&real);
        let across = largest_step(&concealed);
        assert!(
            across <= within_real * 1.5,
            "concealment stepped by {across}, real audio by {within_real}"
        );
    }

    #[test]
    fn concealment_fades_to_silence_and_stays_there() {
        let mut concealer = Concealer::new(RATE);
        concealer.played(&mut tone(1_000.0, 480, 0));

        let mut first = vec![0.0_f32; 480];
        concealer.conceal(&mut first);
        assert!(peak(&first[..FADE]) > 0.1, "the fade should start from real audio");
        assert!(
            first[FADE..].iter().all(|&s| s == 0.0),
            "past the fade, concealment must be silent rather than invented"
        );

        // A second concealed packet is entirely silent: the fade is over.
        let mut second = vec![1.0_f32; 480];
        concealer.conceal(&mut second);
        assert!(second.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn returning_audio_fades_in_rather_than_jumping_to_full_level() {
        let mut concealer = Concealer::new(RATE);
        concealer.played(&mut tone(1_000.0, 480, 0));
        let mut concealed = vec![0.0_f32; 480];
        concealer.conceal(&mut concealed);

        let mut returning = tone(1_000.0, 480, 960);
        let before = returning.clone();
        concealer.played(&mut returning);

        assert!(returning[0].abs() < 1e-6, "returning audio started at full level");
        // Fully restored once the fade is done.
        assert!(
            (returning[FADE..].iter().zip(&before[FADE..]))
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "the fade-in outlasted its window"
        );
    }

    #[test]
    fn a_fade_in_spanning_several_blocks_still_completes() {
        let mut concealer = Concealer::new(RATE);
        concealer.played(&mut tone(1_000.0, 480, 0));
        concealer.conceal(&mut vec![0.0_f32; 480]);

        // Blocks shorter than the fade: the remaining fade must carry over.
        let mut gains = Vec::new();
        for block in 0..4 {
            let mut audio = vec![1.0_f32; 100];
            concealer.played(&mut audio);
            gains.push(audio[0]);
            assert!(audio.iter().all(|&s| (0.0..=1.0).contains(&s)), "block {block}");
        }
        assert!(
            gains.windows(2).all(|pair| pair[1] > pair[0]),
            "the fade-in must keep rising across blocks: {gains:?}"
        );
        assert!((gains[3] - 1.0).abs() < 0.3, "the fade-in never finished");
    }

    #[test]
    fn before_any_real_audio_concealment_is_silence_not_noise() {
        let mut concealer = Concealer::new(RATE);
        let mut out = vec![9.0_f32; 480];
        concealer.conceal(&mut out);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "with no history, concealment must not invent audio"
        );
    }

    #[test]
    fn a_long_gap_asks_for_a_resync_rather_than_concealing_forever() {
        let mut concealer = Concealer::new(RATE);
        concealer.played(&mut tone(1_000.0, 480, 0));
        assert!(!concealer.should_resync());

        // 200 ms of concealment at 5 ms per packet.
        for _ in 0..39 {
            concealer.conceal(&mut vec![0.0_f32; 240]);
        }
        assert!(!concealer.should_resync(), "195 ms is still concealable");
        concealer.conceal(&mut vec![0.0_f32; 240]);
        assert!(concealer.should_resync(), "200 ms should trigger a resync");

        // And real audio clears it.
        concealer.played(&mut tone(1_000.0, 240, 0));
        assert!(!concealer.should_resync());
        assert_eq!(concealer.concealed_ms(), 0.0);
    }

    #[test]
    fn separate_gaps_are_counted_separately() {
        let mut concealer = Concealer::new(RATE);
        let mut out = vec![0.0_f32; 240];

        for _ in 0..3 {
            concealer.played(&mut tone(1_000.0, 240, 0));
            // A two-packet gap is one run, not two.
            concealer.conceal(&mut out);
            concealer.conceal(&mut out);
        }
        assert_eq!(concealer.concealed_runs(), 3);
        assert_eq!(concealer.concealed_samples(), 6 * 240);
    }

    #[test]
    fn a_reset_forgets_the_old_audio_and_fades_the_new_in() {
        let mut concealer = Concealer::new(RATE);
        concealer.played(&mut tone(1_000.0, 480, 0));
        concealer.reset();

        // Nothing from before the resync may be replayed.
        let mut out = vec![9.0_f32; 480];
        concealer.conceal(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));

        // And the first audio of the new session fades in.
        let mut returning = vec![1.0_f32; 480];
        concealer.played(&mut returning);
        assert!(returning[0].abs() < 1e-6);
        assert!((returning[FADE] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_tail_tracks_blocks_shorter_than_the_fade() {
        let mut concealer = Concealer::new(RATE);
        // Ten 50-sample blocks: the tail must end up holding the last 240
        // samples across the block boundaries, not just the final block.
        for block in 0..10 {
            let mut audio: Vec<f32> = (0..50)
                .map(|i| (block * 50 + i) as f32 / 1_000.0)
                .collect();
            concealer.played(&mut audio);
        }
        let mut out = vec![0.0_f32; 240];
        concealer.conceal(&mut out);
        // The last real sample was 499/1000.
        assert!((out[0] - 0.499).abs() < 1e-5, "tail splice was {}", out[0]);
    }
}

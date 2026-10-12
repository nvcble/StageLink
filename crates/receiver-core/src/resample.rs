//! Fractional resampling, for clock drift.
//!
//! ## Why this exists at all
//!
//! The desktop's capture clock and the phone's DAC clock are different
//! crystals. At ±20 ppm each — ordinary consumer tolerance — the worst case is
//! 40 ppm, which over a 90-minute service is **216 ms** of accumulated offset.
//! No fixed buffer survives that: it either starves or overflows, several times
//! over. Something has to consume samples at a rate that is not quite the rate
//! they arrive at, and do it without anybody hearing it happen.
//!
//! The crude alternative — drop or insert a packet when the buffer drifts too
//! far — costs an audible 5 ms discontinuity every couple of minutes. This
//! costs a pitch shift of 0.0009 semitones, which nobody can hear.
//!
//! ## Why a polyphase sinc rather than cubic interpolation
//!
//! Cubic interpolation would be far simpler and its error at a ratio this
//! close to unity is small — but "small" there means roughly −40 dB of
//! broadband junk above 10 kHz, which on cymbals and sibilance is exactly
//! where a performer notices. 32 taps of windowed sinc puts it near −90 dB,
//! for about 1.5 MMAC/s mono. That is nothing on any phone that can run the
//! app at all.
//!
//! ## The detail that would otherwise cause a slow tremolo
//!
//! Every phase row is normalised to sum to exactly 1.0. Without that, the
//! windowing leaves each row with a slightly different DC gain, and since the
//! phase sweeps continuously as the clocks drift apart, the output level would
//! be modulated at the drift beat frequency — a tremolo of a fraction of a
//! decibel, slow, and maddening to listen to for an hour.

/// Taps per output sample. Even, so the kernel straddles the sample pair.
pub const TAPS: usize = 32;

/// Interpolated filter phases. 128 puts the phase-quantisation error below the
/// 16-bit noise floor without a table big enough to miss the cache.
pub const PHASES: usize = 128;

/// Samples of history the kernel needs behind the read position.
const HISTORY: usize = TAPS / 2;

/// Input staging capacity, in packets. Three is enough for the render callback
/// to pull ahead without ever reallocating.
const STAGING_PACKETS: usize = 3;

/// Largest drift correction allowed in steady state, in parts per million.
///
/// 500 ppm is 0.87 cents of pitch shift — comfortably inaudible — and two
/// orders of magnitude more than two consumer crystals can drift apart. A
/// controller asking for more than this has misdiagnosed something, and
/// obeying it would turn a measurement bug into a pitch error.
pub const MAX_STEADY_PPM: f64 = 500.0;

/// Largest correction allowed at all, for short bursts.
///
/// Used only to retire the backlog after a network outage refilled the buffer
/// past its target. 2000 ppm is 3.5 cents: detectable by a good ear on a
/// sustained note, which is why it is time-limited by the caller rather than
/// available as a steady state.
pub const MAX_BURST_PPM: f64 = 2_000.0;

/// A fractional-rate resampler with its own input staging.
///
/// Staging is owned here because the render callback needs to pull output
/// sample by sample while input arrives a packet at a time, and the alternative
/// — a second queue between the two — would add latency and hide the buffer
/// depth from the adaptation logic that needs it.
pub struct Resampler {
    /// `PHASES * TAPS`, row-major by phase.
    table: Box<[f32]>,
    /// History, then unconsumed input.
    input: Vec<f32>,
    /// Index in `input` of the integer part of the read position.
    read: usize,
    /// Fractional part of the read position, in `0.0..1.0`.
    frac: f64,
    /// Input samples advanced per output sample. 1.0 is no correction.
    step: f64,
    frames_per_packet: usize,
    consumed: u64,
    produced: u64,
}

impl Resampler {
    #[must_use]
    pub fn new(frames_per_packet: usize) -> Self {
        let mut input = Vec::with_capacity(HISTORY + frames_per_packet * (STAGING_PACKETS + 1));
        // Start with silent history so the first output sample is filtered the
        // same way every later one is, rather than against uninitialised data.
        input.resize(HISTORY, 0.0);

        Self {
            table: build_table(),
            input,
            read: HISTORY,
            frac: 0.0,
            step: 1.0,
            frames_per_packet,
            consumed: 0,
            produced: 0,
        }
    }

    /// Sets the correction, in parts per million.
    ///
    /// Positive consumes input *faster* than it is produced, which drains a
    /// buffer that is too full. Clamped: see [`MAX_BURST_PPM`].
    pub fn set_correction_ppm(&mut self, ppm: f64) {
        let ppm = if ppm.is_finite() {
            ppm.clamp(-MAX_BURST_PPM, MAX_BURST_PPM)
        } else {
            0.0
        };
        self.step = 1.0 + ppm / 1_000_000.0;
    }

    /// The correction currently applied, in parts per million.
    #[must_use]
    pub fn correction_ppm(&self) -> f64 {
        (self.step - 1.0) * 1_000_000.0
    }

    /// Room for another packet without reallocating.
    #[must_use]
    pub fn can_accept(&self) -> bool {
        self.unconsumed() <= self.frames_per_packet * (STAGING_PACKETS - 1)
    }

    /// Input samples not yet read.
    #[must_use]
    pub fn unconsumed(&self) -> usize {
        self.input.len().saturating_sub(self.read)
    }

    /// Appends a packet's samples.
    ///
    /// Compacts first when the history has grown, so the buffer never
    /// reallocates on the audio thread. The move is at most a few hundred
    /// floats and happens once per packet.
    pub fn push(&mut self, samples: &[f32]) {
        if self.read > HISTORY {
            let drop = self.read - HISTORY;
            self.input.copy_within(drop.., 0);
            self.input.truncate(self.input.len() - drop);
            self.read = HISTORY;
        }
        self.input.extend_from_slice(samples);
    }

    /// Produces the next output sample, or `None` if there is not enough input.
    ///
    /// Needs `HISTORY` samples of lookahead past the read position, because the
    /// kernel is centred. That lookahead is latency — 16 samples, a third of a
    /// millisecond at 48 kHz — and is the price of not using a cheaper filter.
    pub fn next_sample(&mut self) -> Option<f32> {
        if self.read + HISTORY >= self.input.len() {
            return None;
        }

        // Nearest phase. The residual error is below the 16-bit noise floor at
        // 128 phases, so interpolating between rows would buy nothing.
        let phase = ((self.frac * PHASES as f64) as usize).min(PHASES - 1);
        let row = &self.table[phase * TAPS..(phase + 1) * TAPS];
        let start = self.read + 1 - HISTORY;

        let mut sum = 0.0_f32;
        for (tap, &coefficient) in row.iter().enumerate() {
            sum += coefficient * self.input[start + tap];
        }

        self.frac += self.step;
        let whole = self.frac.floor();
        self.read += whole as usize;
        self.frac -= whole;
        self.consumed += whole as u64;
        self.produced += 1;

        Some(sum)
    }

    /// Fills `out`, returning how many samples were produced.
    ///
    /// A short return means the input ran out: the caller pushes another packet
    /// — or conceals — and calls again.
    pub fn fill(&mut self, out: &mut [f32]) -> usize {
        let mut produced = 0;
        for slot in out.iter_mut() {
            match self.next_sample() {
                Some(sample) => {
                    *slot = sample;
                    produced += 1;
                }
                None => break,
            }
        }
        produced
    }

    /// Discards all input and history. Used on a resync.
    pub fn reset(&mut self) {
        self.input.clear();
        self.input.resize(HISTORY, 0.0);
        self.read = HISTORY;
        self.frac = 0.0;
    }

    #[must_use]
    pub const fn consumed(&self) -> u64 {
        self.consumed
    }

    #[must_use]
    pub const fn produced(&self) -> u64 {
        self.produced
    }
}

/// Builds the polyphase coefficient table.
///
/// Run once at construction. Nothing here may happen on the audio thread.
fn build_table() -> Box<[f32]> {
    // Just below Nyquist: the resampling ratio is within 0.2% of unity, so
    // there is no need to sacrifice bandwidth, but leaving a little margin
    // keeps the transition band out of the audible top octave.
    const CUTOFF: f64 = 0.94;

    let mut table = vec![0.0_f32; PHASES * TAPS];
    for phase in 0..PHASES {
        let offset = phase as f64 / PHASES as f64;
        let row = &mut table[phase * TAPS..(phase + 1) * TAPS];

        for (tap, coefficient) in row.iter_mut().enumerate() {
            // Distance from the interpolation point to this tap's sample.
            let x = tap as f64 - (HISTORY as f64 - 1.0) - offset;
            let sinc = if x.abs() < 1e-9 {
                CUTOFF
            } else {
                let arg = core::f64::consts::PI * CUTOFF * x;
                CUTOFF * arg.sin() / arg
            };
            // Blackman window, over the whole kernel including the offset, so
            // the window follows the phase rather than sitting still under it.
            let position = (x + HISTORY as f64) / TAPS as f64;
            let window = 0.42
                - 0.5 * (core::f64::consts::TAU * position).cos()
                + 0.08 * (2.0 * core::f64::consts::TAU * position).cos();
            *coefficient = (sinc * window) as f32;
        }

        // Unity DC gain, exactly, for every phase. Without this the output
        // level would be modulated as the phase sweeps with the drift — a slow
        // tremolo, which is far more noticeable than the drift it corrects.
        let sum: f32 = row.iter().sum();
        if sum.abs() > f32::EPSILON {
            for coefficient in row.iter_mut() {
                *coefficient /= sum;
            }
        }
    }
    table.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAMES: usize = 240;
    const RATE: f64 = 48_000.0;

    fn tone(frequency: f64, amplitude: f64, frames: usize, phase_offset: f64) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let t = (frame as f64 + phase_offset) / RATE;
                (core::f64::consts::TAU * frequency * t).sin() as f32 * amplitude as f32
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    /// Drives a long tone through, returning the output after the filter's
    /// group delay has been skipped.
    fn run(ppm: f64, frequency: f64, packets: usize) -> Vec<f32> {
        let mut resampler = Resampler::new(FRAMES);
        resampler.set_correction_ppm(ppm);

        let mut out = Vec::new();
        let mut buffer = vec![0.0_f32; FRAMES];
        for packet in 0..packets {
            let input = tone(frequency, 0.5, FRAMES, (packet * FRAMES) as f64);
            resampler.push(&input);
            loop {
                let produced = resampler.fill(&mut buffer);
                out.extend_from_slice(&buffer[..produced]);
                if produced < buffer.len() {
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn every_phase_has_exactly_unity_gain() {
        // The tremolo guard. A phase row that does not sum to one changes the
        // level, and the phase sweeps continuously while drift is corrected.
        let table = build_table();
        for phase in 0..PHASES {
            let sum: f32 = table[phase * TAPS..(phase + 1) * TAPS].iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-6,
                "phase {phase} sums to {sum}, which would modulate the level"
            );
        }
    }

    #[test]
    fn at_no_correction_a_tone_passes_through_at_its_own_level() {
        let out = run(0.0, 1_000.0, 20);
        let steady = &out[1_000..];
        let level = rms(steady);
        // 0.5 amplitude sine is 0.3536 RMS.
        assert!(
            (level - 0.353_6).abs() < 0.005,
            "level was {level}, expected 0.354"
        );
    }

    #[test]
    fn the_passband_is_flat_across_the_audible_range() {
        for frequency in [100.0, 1_000.0, 5_000.0, 10_000.0, 15_000.0] {
            let out = run(0.0, frequency, 30);
            let level = rms(&out[2_000..]);
            assert!(
                (level - 0.353_6).abs() < 0.02,
                "{frequency} Hz came out at {level}, expected 0.354"
            );
        }
    }

    /// The point of the whole module: a correction consumes input at a
    /// different rate from the output, and the ratio has to be the one asked
    /// for — not merely in the right direction.
    ///
    /// `consumed` counts whole-sample advances of the read position, so it is
    /// up to one sample out at the ends of a run. The run is long enough
    /// (480 000 input samples) that one sample is 2 ppm, which is why the
    /// tolerance is 3 and not tighter.
    #[test]
    fn a_correction_changes_the_consumption_rate_by_the_stated_ppm() {
        for ppm in [-500.0, -100.0, 0.0, 100.0, 500.0] {
            let mut resampler = Resampler::new(FRAMES);
            resampler.set_correction_ppm(ppm);

            let mut buffer = vec![0.0_f32; FRAMES];
            for packet in 0..2_000 {
                resampler.push(&tone(1_000.0, 0.5, FRAMES, (packet * FRAMES) as f64));
                while resampler.fill(&mut buffer) == buffer.len() {}
            }

            let measured = (resampler.consumed() as f64 / resampler.produced() as f64 - 1.0)
                * 1_000_000.0;
            assert!(
                (measured - ppm).abs() < 3.0,
                "asked for {ppm} ppm, measured {measured:.1} ppm"
            );
        }
    }

    /// Rising zero crossings per second. Accurate enough to resolve a 0.5 Hz
    /// shift in 1 kHz over a one-second window, which is what a ppm-level
    /// correction amounts to.
    fn frequency_of(samples: &[f32]) -> f64 {
        let crossings = samples
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        crossings as f64 * RATE / samples.len() as f64
    }

    /// Correcting drift *is* a pitch shift — there is no way around that — so
    /// the claim worth testing is that the shift is the one asked for and that
    /// it is below what anyone can hear.
    #[test]
    fn a_correction_shifts_the_pitch_by_exactly_the_stated_ppm() {
        // Consuming input faster than it is produced plays the signal faster,
        // so a positive correction raises the pitch.
        let corrected = run(500.0, 1_000.0, 500);
        let measured = frequency_of(&corrected[2_000..]);

        let expected = 1_000.0 * (1.0 + 500.0 / 1e6);
        assert!(
            (measured - expected).abs() < 0.5,
            "expected {expected:.3} Hz, measured {measured:.3} Hz"
        );

        // And that shift, in cents, is an order of magnitude below the ~5
        // cents a trained ear can detect on a sustained note.
        let cents = 1_200.0 * (measured / 1_000.0).log2();
        assert!(cents.abs() < 1.0, "the shift was {cents:.2} cents");
    }

    #[test]
    fn no_correction_means_no_pitch_change_at_all() {
        let clean = run(0.0, 1_000.0, 500);
        let measured = frequency_of(&clean[2_000..]);
        assert!(
            (measured - 1_000.0).abs() < 0.2,
            "an uncorrected stream drifted to {measured:.3} Hz"
        );
    }

    #[test]
    fn a_correction_is_clamped_to_the_burst_maximum() {
        let mut resampler = Resampler::new(FRAMES);

        resampler.set_correction_ppm(1e9);
        assert!((resampler.correction_ppm() - MAX_BURST_PPM).abs() < 1e-6);

        resampler.set_correction_ppm(-1e9);
        assert!((resampler.correction_ppm() + MAX_BURST_PPM).abs() < 1e-6);

        // A controller producing NaN must not stop the audio.
        resampler.set_correction_ppm(f64::NAN);
        assert_eq!(resampler.correction_ppm(), 0.0);
    }

    #[test]
    fn the_steady_maximum_is_well_inside_what_is_audible() {
        // 500 ppm as cents: 1200 * log2(1.0005).
        let cents = 1_200.0 * (1.0 + MAX_STEADY_PPM / 1e6).log2();
        assert!(cents < 1.0, "the steady clamp is {cents:.2} cents");
        // And it leaves an order of magnitude of headroom over the 40 ppm two
        // ordinary ±20 ppm crystals can differ by.
        let worst_case_crystal_ppm = 40.0;
        assert!(MAX_STEADY_PPM / worst_case_crystal_ppm >= 10.0);
    }

    #[test]
    fn output_stops_cleanly_when_the_input_runs_out() {
        let mut resampler = Resampler::new(FRAMES);
        let mut out = vec![0.0_f32; FRAMES];

        // Nothing pushed: nothing produced, and no panic.
        assert_eq!(resampler.fill(&mut out), 0);
        assert!(resampler.next_sample().is_none());

        // One packet produces nearly a packet: the kernel needs HISTORY
        // samples of lookahead, so the tail waits for the next packet.
        resampler.push(&tone(1_000.0, 0.5, FRAMES, 0.0));
        let produced = resampler.fill(&mut out);
        assert_eq!(produced, FRAMES - HISTORY);
    }

    #[test]
    fn pushing_many_packets_never_reallocates() {
        let mut resampler = Resampler::new(FRAMES);
        let capacity = resampler.input.capacity();
        let mut out = vec![0.0_f32; FRAMES];

        for packet in 0..1_000 {
            if resampler.can_accept() {
                resampler.push(&tone(1_000.0, 0.5, FRAMES, (packet * FRAMES) as f64));
            }
            resampler.fill(&mut out);
            assert_eq!(
                resampler.input.capacity(),
                capacity,
                "the staging buffer grew, which means it allocated on the audio thread"
            );
        }
    }

    #[test]
    fn back_pressure_is_reported_rather_than_left_to_overflow() {
        let mut resampler = Resampler::new(FRAMES);
        assert!(resampler.can_accept());

        // Fill the staging without consuming anything.
        let mut pushes = 0;
        while resampler.can_accept() {
            resampler.push(&vec![0.1_f32; FRAMES]);
            pushes += 1;
            assert!(pushes < 10, "can_accept never went false");
        }
        assert!(resampler.unconsumed() >= FRAMES * (STAGING_PACKETS - 1));
    }

    #[test]
    fn a_reset_clears_the_staging_and_the_phase() {
        let mut resampler = Resampler::new(FRAMES);
        resampler.push(&tone(1_000.0, 0.5, FRAMES, 0.0));
        let mut out = vec![0.0_f32; FRAMES];
        resampler.fill(&mut out);

        resampler.reset();
        assert_eq!(resampler.unconsumed(), 0);
        assert!(resampler.next_sample().is_none());

        // And it works again, from silent history rather than stale audio.
        resampler.push(&vec![0.0_f32; FRAMES]);
        let produced = resampler.fill(&mut out);
        assert_eq!(produced, FRAMES - HISTORY);
        assert!(
            out[..produced].iter().all(|&s| s.abs() < 1e-9),
            "audio from before the reset leaked through"
        );
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut resampler = Resampler::new(FRAMES);
        let mut out = vec![1.0_f32; FRAMES];
        for _ in 0..5 {
            resampler.push(&vec![0.0_f32; FRAMES]);
            let produced = resampler.fill(&mut out);
            assert!(out[..produced].iter().all(|&s| s == 0.0));
        }
    }
}

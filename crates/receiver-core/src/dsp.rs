//! The personal monitor chain: gain, EQ, limiter, safety clip.
//!
//! Runs inside the audio callback, so it allocates nothing, locks nothing and
//! branches on nothing that depends on the data taking a slow path.
//!
//! ## Order, and why it is this order
//!
//! ```text
//!   sanitise -> gain (ramped) -> EQ -> limiter -> hard clip
//! ```
//!
//! * **Sanitise first.** One non-finite sample would poison the EQ's recursive
//!   state permanently — not for that block, for the rest of the session. A
//!   performer would lose their monitor until they restarted the app, from one
//!   corrupt packet.
//! * **Gain before EQ**, so the EQ's own limits are reached predictably, and
//!   ramped because a stepped fader move is an audible zipper.
//! * **Limiter after EQ**, because a boosted band is exactly what creates the
//!   peak worth limiting.
//! * **Hard clip last, unconditionally.** See below.
//!
//! ## Hearing safety
//!
//! The limiter's ceiling is clamped to a [`SafetyProfile`] maximum that no
//! setting can exceed, and a hard clip runs afterwards regardless. Three
//! reasons, and the third is not about safety at all:
//!
//! 1. This puts sound directly into someone's ears, louder than they would
//!    choose, for an hour at a time. A UI bug must not be able to injure
//!    somebody.
//! 2. The limiter has no lookahead — lookahead is latency, and latency is the
//!    product — so the very first sample of a transient passes through it
//!    unattenuated. Something has to catch that.
//! 3. `audio_core::convert::pcm_s16le_to_f32` divides by 32767, so a foreign
//!    `i16::MIN` decodes to −1.00003. That is out of range before any gain is
//!    applied at all.

/// How loud the receiver is permitted to get.
///
/// Deliberately coarse: three choices an operator or performer can reason
/// about, rather than a decibel field that invites being set to the maximum and
/// forgotten. Raising it is a single auditable call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SafetyProfile {
    /// −6 dBFS ceiling. The default, and what in-ear monitoring should use.
    #[default]
    Protected,
    /// −3 dBFS. For a performer who needs more and understands the trade.
    Standard,
    /// 0 dBFS. Full scale, still hard-clipped. Not for in-ear use.
    Unrestricted,
}

impl SafetyProfile {
    /// The highest ceiling this profile allows, as a linear amplitude.
    #[must_use]
    pub const fn max_ceiling(self) -> f32 {
        match self {
            // 10^(-6/20) and 10^(-3/20), written out so the constant is
            // visible rather than computed at a call site.
            Self::Protected => 0.501_187_2,
            Self::Standard => 0.707_945_8,
            Self::Unrestricted => 1.0,
        }
    }
}

/// One band of the EQ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSettings {
    pub frequency_hz: f32,
    pub gain_db: f32,
    /// Shelf slope or peak width. Clamped to a sane range.
    pub q: f32,
}

/// Everything a performer can change about their own monitor.
///
/// Changed from the application thread, never from the audio callback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorSettings {
    pub gain_db: f32,
    pub muted: bool,
    pub low: BandSettings,
    pub mid: BandSettings,
    pub high: BandSettings,
    /// Requested limiter ceiling, linear. Clamped by the safety profile.
    pub ceiling: f32,
    pub safety: SafetyProfile,
}

impl Default for MonitorSettings {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            muted: false,
            low: BandSettings { frequency_hz: 150.0, gain_db: 0.0, q: 0.707 },
            mid: BandSettings { frequency_hz: 1_000.0, gain_db: 0.0, q: 0.9 },
            high: BandSettings { frequency_hz: 4_000.0, gain_db: 0.0, q: 0.707 },
            ceiling: 1.0,
            safety: SafetyProfile::Protected,
        }
    }
}

/// Gain limits, as decibels.
///
/// The lower bound is silence; the upper bound exists because +40 dB on a
/// −20 dBFS mix is not a volume control, it is a hazard, and the limiter should
/// not be the only thing standing in the way.
pub const MIN_GAIN_DB: f32 = -60.0;
pub const MAX_GAIN_DB: f32 = 12.0;

/// Band gain limits.
pub const MIN_BAND_DB: f32 = -18.0;
pub const MAX_BAND_DB: f32 = 18.0;

/// A biquad in transposed direct form II.
///
/// Two state words rather than four, and better numerical behaviour at low
/// frequencies than direct form I — which matters for a 150 Hz shelf at
/// 48 kHz.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A pass-through.
    const fn identity() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }

    /// Normalises the Robert Bristow-Johnson coefficient set.
    fn from_rbj(b: [f32; 3], a: [f32; 3]) -> Self {
        // a[0] is never zero for any of the filter types below, but dividing by
        // a value derived from user input without checking would turn a bad
        // setting into NaN for the rest of the session.
        if a[0].abs() < f32::EPSILON || !a[0].is_finite() {
            return Self::identity();
        }
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn low_shelf(settings: BandSettings, sample_rate: f32) -> Self {
        let (amplitude, omega, alpha) = Self::common(settings, sample_rate);
        let (cos_w, sqrt_a) = (omega.cos(), amplitude.sqrt());
        let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
        Self::from_rbj(
            [
                amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cos_w + two_sqrt_a_alpha),
                2.0 * amplitude * ((amplitude - 1.0) - (amplitude + 1.0) * cos_w),
                amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cos_w - two_sqrt_a_alpha),
            ],
            [
                (amplitude + 1.0) + (amplitude - 1.0) * cos_w + two_sqrt_a_alpha,
                -2.0 * ((amplitude - 1.0) + (amplitude + 1.0) * cos_w),
                (amplitude + 1.0) + (amplitude - 1.0) * cos_w - two_sqrt_a_alpha,
            ],
        )
    }

    fn peaking(settings: BandSettings, sample_rate: f32) -> Self {
        let (amplitude, omega, alpha) = Self::common(settings, sample_rate);
        let cos_w = omega.cos();
        Self::from_rbj(
            [1.0 + alpha * amplitude, -2.0 * cos_w, 1.0 - alpha * amplitude],
            [1.0 + alpha / amplitude, -2.0 * cos_w, 1.0 - alpha / amplitude],
        )
    }

    fn high_shelf(settings: BandSettings, sample_rate: f32) -> Self {
        let (amplitude, omega, alpha) = Self::common(settings, sample_rate);
        let (cos_w, sqrt_a) = (omega.cos(), amplitude.sqrt());
        let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
        Self::from_rbj(
            [
                amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cos_w + two_sqrt_a_alpha),
                -2.0 * amplitude * ((amplitude - 1.0) + (amplitude + 1.0) * cos_w),
                amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cos_w - two_sqrt_a_alpha),
            ],
            [
                (amplitude + 1.0) - (amplitude - 1.0) * cos_w + two_sqrt_a_alpha,
                2.0 * ((amplitude - 1.0) - (amplitude + 1.0) * cos_w),
                (amplitude + 1.0) - (amplitude - 1.0) * cos_w - two_sqrt_a_alpha,
            ],
        )
    }

    /// Shared coefficient terms, with every input clamped.
    ///
    /// Clamped here rather than trusted: these numbers arrive from another
    /// language across an FFI boundary, and a frequency above Nyquist or a Q of
    /// zero produces coefficients that make the filter explode — in somebody's
    /// ears.
    fn common(settings: BandSettings, sample_rate: f32) -> (f32, f32, f32) {
        let gain_db = settings.gain_db.clamp(MIN_BAND_DB, MAX_BAND_DB);
        // RBJ shelving filters use A = 10^(dB/40), not /20.
        let amplitude = 10.0_f32.powf(gain_db / 40.0);
        let frequency = settings
            .frequency_hz
            .clamp(20.0, (sample_rate / 2.0) * 0.95);
        let q = settings.q.clamp(0.1, 10.0);
        let omega = core::f32::consts::TAU * frequency / sample_rate;
        let alpha = omega.sin() / (2.0 * q);
        (amplitude, omega, alpha)
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }

    /// Clears the recursive state.
    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Flushes state that has decayed into the denormal range.
    ///
    /// Denormals are an order of magnitude slower to process on some CPUs, and
    /// they appear exactly when the input goes quiet — which on stage is
    /// between songs, immediately before the audio matters most.
    #[inline]
    fn flush_denormals(&mut self) {
        const TINY: f32 = 1e-20;
        if self.z1.abs() < TINY {
            self.z1 = 0.0;
        }
        if self.z2.abs() < TINY {
            self.z2 = 0.0;
        }
    }
}

/// A feed-forward peak limiter with no lookahead.
///
/// No lookahead is a deliberate trade: lookahead costs latency, and latency is
/// what the product is for. The cost is that the first sample of a sharp
/// transient passes at full level, which is one of the reasons the hard clip
/// after it is unconditional.
#[derive(Debug, Clone, Copy)]
struct Limiter {
    ceiling: f32,
    /// Current gain reduction, 1.0 when inactive.
    gain: f32,
    release: f32,
}

impl Limiter {
    /// Instant attack, 100 ms release.
    ///
    /// A smoothed attack is gentler on the ear but overshoots — about 8% on a
    /// 1 kHz tone with a 1 ms time constant, which would put the output above
    /// the ceiling the safety profile exists to enforce. Since this limiter
    /// *is* the hearing-safety mechanism and has no lookahead to work with,
    /// it clamps immediately and recovers slowly. The audible cost is some
    /// harmonic distortion on material that is already far too hot; the
    /// alternative cost is exceeding the ceiling.
    fn new(ceiling: f32, sample_rate: f32) -> Self {
        Self {
            ceiling,
            gain: 1.0,
            release: Self::coefficient(100.0, sample_rate),
        }
    }

    /// One-pole smoothing coefficient for a time constant in milliseconds.
    fn coefficient(millis: f32, sample_rate: f32) -> f32 {
        if millis <= 0.0 || sample_rate <= 0.0 {
            return 0.0;
        }
        (-1.0 / (millis / 1_000.0 * sample_rate)).exp()
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let magnitude = input.abs();
        let target = if magnitude > self.ceiling {
            self.ceiling / magnitude
        } else {
            1.0
        };
        if target < self.gain {
            // Down immediately: this sample must not exceed the ceiling.
            self.gain = target;
        } else {
            self.gain = target + (self.gain - target) * self.release;
        }
        input * self.gain
    }
}

/// The complete monitor chain for one receiver.
pub struct MonitorChain {
    sample_rate: f32,
    settings: MonitorSettings,
    /// Linear gain the ramp is heading towards.
    target_gain: f32,
    /// Linear gain actually applied, moved towards the target per sample.
    current_gain: f32,
    gain_step: f32,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
    limiter: Limiter,
    /// Samples the hard clip had to catch.
    ///
    /// Should always be zero: the limiter in front of it clamps immediately,
    /// so nothing should reach the clip. A non-zero value in the field means
    /// the limiter is misbehaving, which is worth knowing about rather than
    /// absorbing silently.
    clipped: u64,
    /// Samples that arrived non-finite.
    sanitised: u64,
}

impl MonitorChain {
    /// How long a gain or mute change takes. Long enough to be inaudible, short
    /// enough that hitting mute feels immediate.
    pub const RAMP_MS: f32 = 5.0;

    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let mut chain = Self {
            sample_rate: sample_rate as f32,
            settings: MonitorSettings::default(),
            target_gain: 1.0,
            current_gain: 1.0,
            gain_step: 0.0,
            low: Biquad::identity(),
            mid: Biquad::identity(),
            high: Biquad::identity(),
            limiter: Limiter::new(
                SafetyProfile::default().max_ceiling(),
                sample_rate as f32,
            ),
            clipped: 0,
            sanitised: 0,
        };
        chain.apply(MonitorSettings::default());
        chain
    }

    /// Installs new settings. Called from the application thread.
    ///
    /// Coefficients are computed here — transcendental functions and a handful
    /// of divisions — precisely so the audio callback does none of that work.
    pub fn apply(&mut self, settings: MonitorSettings) {
        let gain_db = if settings.muted {
            MIN_GAIN_DB
        } else {
            settings.gain_db.clamp(MIN_GAIN_DB, MAX_GAIN_DB)
        };
        self.target_gain = if settings.muted || gain_db <= MIN_GAIN_DB {
            0.0
        } else {
            10.0_f32.powf(gain_db / 20.0)
        };

        let ramp_samples = (Self::RAMP_MS / 1_000.0 * self.sample_rate).max(1.0);
        self.gain_step = 1.0 / ramp_samples;

        self.low = Biquad::low_shelf(settings.low, self.sample_rate);
        self.mid = Biquad::peaking(settings.mid, self.sample_rate);
        self.high = Biquad::high_shelf(settings.high, self.sample_rate);

        // The requested ceiling, but never above what the profile permits. A
        // caller asking for more gets the maximum, not an error: refusing would
        // tempt the caller into a retry loop, and silently obeying would be the
        // bug this clamp exists to prevent.
        let ceiling = settings
            .ceiling
            .clamp(0.001, settings.safety.max_ceiling());
        self.limiter = Limiter { ceiling, ..self.limiter };

        self.settings = settings;
    }

    #[must_use]
    pub const fn settings(&self) -> MonitorSettings {
        self.settings
    }

    /// The ceiling actually in force, after the safety clamp.
    #[must_use]
    pub const fn effective_ceiling(&self) -> f32 {
        self.limiter.ceiling
    }

    /// Processes a block in place. The audio-callback path.
    pub fn process(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            // One non-finite sample would otherwise live in the EQ's state for
            // the rest of the session.
            let mut value = if sample.is_finite() {
                *sample
            } else {
                self.sanitised += 1;
                0.0
            };

            // Linear ramp towards the target, so a fader move or a mute does
            // not step the waveform.
            if self.current_gain != self.target_gain {
                let delta = self.target_gain - self.current_gain;
                self.current_gain += delta.signum() * self.gain_step.min(delta.abs());
            }
            value *= self.current_gain;

            value = self.low.process(value);
            value = self.mid.process(value);
            value = self.high.process(value);

            value = self.limiter.process(value);

            // Unconditional. Nothing above can switch this off.
            if value > 1.0 {
                value = 1.0;
                self.clipped += 1;
            } else if value < -1.0 {
                value = -1.0;
                self.clipped += 1;
            }

            *sample = value;
        }

        self.low.flush_denormals();
        self.mid.flush_denormals();
        self.high.flush_denormals();
    }

    /// Clears filter state. Used on a resync, where the previous audio is not
    /// continuous with the next and the filters' memory of it is wrong.
    pub fn reset(&mut self) {
        self.low.reset();
        self.mid.reset();
        self.high.reset();
        self.limiter.gain = 1.0;
    }

    #[must_use]
    pub const fn clipped(&self) -> u64 {
        self.clipped
    }

    #[must_use]
    pub const fn sanitised(&self) -> u64 {
        self.sanitised
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tone(frequency: f32, amplitude: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let t = frame as f32 / RATE as f32;
                (core::f32::consts::TAU * frequency * t).sin() * amplitude
            })
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()))
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|&s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// Settles the gain ramp so a measurement is of the filter, not the ramp.
    fn settled(chain: &mut MonitorChain, samples: &mut [f32]) {
        let mut warmup = samples.to_vec();
        chain.process(&mut warmup);
        chain.process(samples);
    }

    #[test]
    fn a_default_chain_passes_audio_through_at_its_own_level() {
        let mut chain = MonitorChain::new(RATE);
        let mut samples = tone(1_000.0, 0.25, 4_800);
        let before = rms(&samples);
        settled(&mut chain, &mut samples);
        // Within a quarter of a decibel: flat EQ, unity gain, below the ceiling.
        assert!(
            (rms(&samples) / before - 1.0).abs() < 0.03,
            "level changed by {:.3}x",
            rms(&samples) / before
        );
        assert_eq!(chain.clipped(), 0);
    }

    #[test]
    fn muting_reaches_silence_without_a_step() {
        let mut chain = MonitorChain::new(RATE);
        let mut samples = tone(1_000.0, 0.5, 4_800);
        chain.process(&mut samples);

        chain.apply(MonitorSettings { muted: true, ..MonitorSettings::default() });
        let mut muting = tone(1_000.0, 0.5, 4_800);
        chain.process(&mut muting);

        // The ramp is 5 ms, so the tail of a 100 ms block is silent.
        assert!(peak(&muting[1_000..]) < 1e-6, "mute did not reach silence");
        // And the first sample is not a discontinuity: it is close to where the
        // previous block left off, rather than zero.
        let step = (muting[0] - samples[samples.len() - 1]).abs();
        assert!(step < 0.1, "mute stepped the waveform by {step}");
    }

    #[test]
    fn unmuting_comes_back_up() {
        let mut chain = MonitorChain::new(RATE);
        chain.apply(MonitorSettings { muted: true, ..MonitorSettings::default() });
        let mut samples = tone(1_000.0, 0.5, 4_800);
        chain.process(&mut samples);
        assert!(peak(&samples[1_000..]) < 1e-6);

        chain.apply(MonitorSettings::default());
        let mut samples = tone(1_000.0, 0.5, 4_800);
        chain.process(&mut samples);
        assert!(peak(&samples[1_000..]) > 0.4, "audio did not return");
    }

    #[test]
    fn gain_is_clamped_at_both_ends() {
        let mut chain = MonitorChain::new(RATE);

        // A UI bug asking for +60 dB must not deliver it.
        chain.apply(MonitorSettings { gain_db: 60.0, ..MonitorSettings::default() });
        let mut samples = tone(1_000.0, 0.1, 4_800);
        settled(&mut chain, &mut samples);
        // +12 dB is the cap, and the Protected ceiling is -6 dBFS, so the peak
        // cannot approach full scale.
        assert!(peak(&samples) <= 0.52, "peak was {}", peak(&samples));

        // And the bottom of the range is silence rather than a tiny signal.
        chain.apply(MonitorSettings { gain_db: -200.0, ..MonitorSettings::default() });
        let mut samples = tone(1_000.0, 0.5, 4_800);
        chain.process(&mut samples);
        assert!(peak(&samples[1_000..]) < 1e-6);
    }

    #[test]
    fn a_band_boost_raises_its_own_frequency_and_leaves_others_alone() {
        let mut chain = MonitorChain::new(RATE);
        chain.apply(MonitorSettings {
            low: BandSettings { frequency_hz: 150.0, gain_db: 12.0, q: 0.707 },
            ..MonitorSettings::default()
        });

        let mut low = tone(100.0, 0.1, 9_600);
        let mut high = tone(8_000.0, 0.1, 9_600);
        settled(&mut chain, &mut low);
        chain.reset();
        settled(&mut chain, &mut high);

        // A 12 dB low shelf is about 4x on the amplitude at 100 Hz.
        assert!(rms(&low) > 0.1 / 1.414 * 2.5, "low shelf did not boost: {}", rms(&low));
        // And 8 kHz is two and a half octaves above the corner, so untouched.
        assert!(
            (rms(&high) / (0.1 / 1.414) - 1.0).abs() < 0.1,
            "the shelf leaked into 8 kHz: {}",
            rms(&high)
        );
    }

    #[test]
    fn band_gain_is_clamped() {
        let mut chain = MonitorChain::new(RATE);
        chain.apply(MonitorSettings {
            mid: BandSettings { frequency_hz: 1_000.0, gain_db: 400.0, q: 0.9 },
            ..MonitorSettings::default()
        });
        let mut samples = tone(1_000.0, 0.2, 9_600);
        settled(&mut chain, &mut samples);
        // +18 dB is the cap and the limiter catches the rest; what must not
        // happen is an unbounded filter.
        assert!(samples.iter().all(|s| s.is_finite()));
        assert!(peak(&samples) <= 1.0);
    }

    /// Coefficients come across an FFI boundary from another language. A
    /// frequency above Nyquist or a zero Q produces a filter that explodes, and
    /// it would explode in somebody's ears.
    #[test]
    fn hostile_band_settings_cannot_make_the_filter_explode() {
        for band in [
            BandSettings { frequency_hz: 1e9, gain_db: 18.0, q: 0.707 },
            BandSettings { frequency_hz: -5.0, gain_db: 18.0, q: 0.707 },
            BandSettings { frequency_hz: 1_000.0, gain_db: 18.0, q: 0.0 },
            BandSettings { frequency_hz: 1_000.0, gain_db: 18.0, q: -3.0 },
            BandSettings { frequency_hz: f32::NAN, gain_db: 0.0, q: 1.0 },
            BandSettings { frequency_hz: 1_000.0, gain_db: f32::INFINITY, q: 1.0 },
        ] {
            let mut chain = MonitorChain::new(RATE);
            chain.apply(MonitorSettings { mid: band, low: band, high: band, ..MonitorSettings::default() });
            let mut samples = tone(1_000.0, 0.3, 9_600);
            chain.process(&mut samples);
            assert!(
                samples.iter().all(|s| s.is_finite()),
                "non-finite output for {band:?}"
            );
            assert!(peak(&samples) <= 1.0, "unbounded output for {band:?}");
        }
    }

    #[test]
    fn the_limiter_holds_the_ceiling() {
        let mut chain = MonitorChain::new(RATE);
        // A hot input: +12 dB of gain on a full-scale tone.
        chain.apply(MonitorSettings { gain_db: 12.0, ..MonitorSettings::default() });
        let mut samples = tone(1_000.0, 1.0, 48_000);
        settled(&mut chain, &mut samples);

        // Not "nearly" the ceiling: the limiter clamps immediately, so no
        // sample is ever above it. That is the property the safety profile
        // promises, and an overshoot tolerance here would quietly give it away.
        assert!(
            peak(&samples) <= SafetyProfile::Protected.max_ceiling(),
            "peak {} above the -6 dBFS ceiling",
            peak(&samples)
        );
        assert!(peak(&samples) > 0.4, "the limiter should not have killed the signal");
        assert_eq!(chain.clipped(), 0, "the hard clip should not have been needed");
    }

    /// The safety invariant: no setting, however hostile, raises the ceiling
    /// above the profile's maximum.
    #[test]
    fn no_requested_ceiling_can_exceed_the_safety_profile() {
        for (profile, maximum) in [
            (SafetyProfile::Protected, 0.501_187_2_f32),
            (SafetyProfile::Standard, 0.707_945_8),
            (SafetyProfile::Unrestricted, 1.0),
        ] {
            let mut chain = MonitorChain::new(RATE);
            chain.apply(MonitorSettings {
                ceiling: 1_000.0,
                safety: profile,
                ..MonitorSettings::default()
            });
            assert_eq!(chain.effective_ceiling(), maximum, "for {profile:?}");

            chain.apply(MonitorSettings {
                ceiling: f32::INFINITY,
                safety: profile,
                ..MonitorSettings::default()
            });
            assert_eq!(chain.effective_ceiling(), maximum, "infinity, for {profile:?}");
        }
    }

    #[test]
    fn a_lower_requested_ceiling_is_honoured() {
        let mut chain = MonitorChain::new(RATE);
        chain.apply(MonitorSettings { ceiling: 0.1, ..MonitorSettings::default() });
        assert!((chain.effective_ceiling() - 0.1).abs() < 1e-6);
    }

    /// `pcm_s16le_to_f32` divides by 32767, so a foreign `i16::MIN` decodes to
    /// −1.00003 before any gain is applied. The clip is what makes that
    /// harmless.
    /// `pcm_s16le_to_f32` divides by 32767, so a foreign `i16::MIN` decodes to
    /// −1.00003 before any gain is applied. Whatever arrives, the output is in
    /// range.
    ///
    /// The clip counter staying at zero here is the *correct* result, and worth
    /// pinning: with an instant-attack limiter in front of it the clip should
    /// never be reached, so a non-zero `clipped()` in the field means the
    /// limiter itself is misbehaving. The clip is a backstop, not a
    /// participant.
    #[test]
    fn out_of_range_input_cannot_produce_out_of_range_output() {
        for profile in [
            SafetyProfile::Protected,
            SafetyProfile::Standard,
            SafetyProfile::Unrestricted,
        ] {
            let mut chain = MonitorChain::new(RATE);
            chain.apply(MonitorSettings { safety: profile, ..MonitorSettings::default() });

            let mut samples = vec![-1.000_031, 1.000_031, -4.0, 4.0, -40.0, 40.0];
            // Twice, so the gain ramp is at unity for the measured pass.
            chain.process(&mut samples.clone());
            chain.process(&mut samples);

            let ceiling = profile.max_ceiling();
            assert!(
                samples.iter().all(|&s| s.abs() <= ceiling),
                "{profile:?} let {} through a {ceiling} ceiling",
                peak(&samples)
            );
            assert_eq!(
                chain.clipped(),
                0,
                "the limiter should have handled this without the backstop"
            );
        }
    }

    /// The backstop itself, exercised by handing the clip something the
    /// limiter cannot have seen. Not reachable through the public API — which
    /// is the point — so it is driven directly.
    #[test]
    fn the_hard_clip_bounds_anything_the_limiter_misses() {
        let mut limiter = Limiter { ceiling: f32::NAN, gain: 1.0, release: 0.0 };
        // A NaN ceiling makes every comparison false, so the limiter passes
        // the sample through untouched: exactly the failure the clip is for.
        assert_eq!(limiter.process(4.0), 4.0);

        let mut chain = MonitorChain::new(RATE);
        chain.limiter = limiter;
        let mut samples = vec![4.0, -4.0, 1.5];
        chain.process(&mut samples.clone());
        chain.process(&mut samples);
        assert!(samples.iter().all(|&s| (-1.0..=1.0).contains(&s)));
        assert!(chain.clipped() > 0, "the clip must count what it caught");
    }

    #[test]
    fn a_non_finite_sample_is_replaced_rather_than_poisoning_the_filters() {
        let mut chain = MonitorChain::new(RATE);
        let mut samples = vec![0.1, f32::NAN, 0.1, f32::INFINITY, 0.1, f32::NEG_INFINITY];
        chain.process(&mut samples);
        assert!(samples.iter().all(|s| s.is_finite()));
        assert_eq!(chain.sanitised(), 3);

        // And the chain still works afterwards. Without the sanitiser the EQ's
        // recursive state would be NaN for the rest of the session.
        let mut good = tone(1_000.0, 0.25, 4_800);
        settled(&mut chain, &mut good);
        assert!(good.iter().all(|s| s.is_finite()));
        assert!(rms(&good) > 0.1, "the chain went silent after a NaN");
    }

    #[test]
    fn a_reset_clears_filter_memory() {
        let mut chain = MonitorChain::new(RATE);
        chain.apply(MonitorSettings {
            low: BandSettings { frequency_hz: 150.0, gain_db: 18.0, q: 0.707 },
            ..MonitorSettings::default()
        });
        let mut samples = tone(100.0, 0.5, 480);
        chain.process(&mut samples);
        chain.reset();

        // After a reset, a silent block stays exactly silent: no ringing from
        // the audio before the resync.
        let mut silence = vec![0.0_f32; 480];
        chain.process(&mut silence);
        assert!(silence.iter().all(|&s| s == 0.0), "the filters still rang");
    }

    #[test]
    fn processing_silence_stays_bit_exact_silence() {
        let mut chain = MonitorChain::new(RATE);
        let mut silence = vec![0.0_f32; 4_800];
        chain.process(&mut silence);
        // Denormal flushing is what makes this exact rather than merely small.
        assert!(silence.iter().all(|&s| s == 0.0));
    }
}

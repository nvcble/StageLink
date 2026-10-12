//! Where the transmitter's audio comes from.
//!
//! Two sources, and the distinction matters for what a test can claim:
//!
//! * [`Source::Tone`] is a generated sine. It is for verifying *mechanism* —
//!   that packets arrive, sequence correctly and decode to the right samples —
//!   because a pure tone makes a dropout, a click or a pitch error obvious to
//!   both an analyser and an ear.
//! * [`Source::Wav`] replays a real recording. It is for verifying
//!   *experience*. A jitter buffer that sounds fine on a sine can sound
//!   terrible on a vocal, and concealment quality cannot be judged on a tone
//!   at all.
//!
//! Neither is ever presented as captured audio. This crate is a test fixture;
//! it is not built into the installer and the desktop application cannot reach
//! it.

use std::f32::consts::TAU;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// A mono audio source, read in whole packets.
pub enum Source {
    Tone {
        /// Radians advanced per frame.
        phase_step: f32,
        phase: f32,
        amplitude: f32,
    },
    Wav {
        samples: Vec<f32>,
        position: usize,
        looping: bool,
    },
    Silence,
}

impl Source {
    /// A sine at `frequency_hz`, amplitude in `0.0..=1.0`.
    #[must_use]
    pub fn tone(frequency_hz: f32, amplitude: f32, sample_rate: u32) -> Self {
        Self::Tone {
            phase_step: TAU * frequency_hz / sample_rate as f32,
            phase: 0.0,
            amplitude: amplitude.clamp(0.0, 1.0),
        }
    }

    /// Reads a WAV file, downmixing to mono and resampling nothing.
    ///
    /// A rate mismatch is refused rather than resampled: silently replaying
    /// 44.1 kHz audio as 48 kHz would make every pitch and timing measurement
    /// taken against it wrong, which is the opposite of what a reference
    /// fixture is for.
    pub fn wav(path: &Path, expected_rate: u32, looping: bool) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let spec = reader.spec();

        if spec.sample_rate != expected_rate {
            bail!(
                "{} is {} Hz but the transmitter is configured for {} Hz. Re-sample the file, \
                 or pass --sample-rate {}.",
                path.display(),
                spec.sample_rate,
                expected_rate,
                spec.sample_rate
            );
        }

        let channels = usize::from(spec.channels.max(1));
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => {
                reader.samples::<f32>().collect::<Result<_, _>>()?
            }
            hound::SampleFormat::Int => {
                // Normalised by the positive full scale, matching
                // `audio_core::convert::pcm_s16le_to_f32`, so a round trip
                // through the transmitter is bit-exact for 16-bit input.
                let divisor = match spec.bits_per_sample {
                    8 => f32::from(i8::MAX),
                    16 => f32::from(i16::MAX),
                    24 => 8_388_607.0,
                    32 => i32::MAX as f32,
                    other => bail!("unsupported bit depth: {other}"),
                };
                reader
                    .samples::<i32>()
                    .map(|sample| sample.map(|value| value as f32 / divisor))
                    .collect::<Result<_, _>>()?
            }
        };

        if interleaved.is_empty() {
            bail!("{} contains no samples", path.display());
        }

        // Average rather than take the left channel: StageLink carries one
        // pre-mixed mono feed, and dropping a channel would silently discard
        // half of a stereo reference.
        let samples = if channels == 1 {
            interleaved
        } else {
            interleaved
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        };

        Ok(Self::Wav { samples, position: 0, looping })
    }

    /// Fills `out` with the next frames.
    ///
    /// Returns false when a non-looping source is exhausted, in which case
    /// `out` holds whatever remained, zero-padded.
    pub fn fill(&mut self, out: &mut [f32]) -> bool {
        match self {
            Self::Tone { phase_step, phase, amplitude } => {
                for slot in out.iter_mut() {
                    *slot = phase.sin() * *amplitude;
                    *phase += *phase_step;
                    // Wrapped every frame rather than left to grow: f32 loses
                    // phase resolution as the magnitude rises, which over a
                    // few minutes audibly distorts the reference tone.
                    if *phase >= TAU {
                        *phase -= TAU;
                    }
                }
                true
            }
            Self::Wav { samples, position, looping } => {
                for slot in out.iter_mut() {
                    if *position >= samples.len() {
                        if !*looping {
                            *slot = 0.0;
                            continue;
                        }
                        *position = 0;
                    }
                    *slot = samples[*position];
                    *position += 1;
                }
                *looping || *position < samples.len()
            }
            Self::Silence => {
                out.fill(0.0);
                true
            }
        }
    }

    /// Total frames available, or `None` for an endless source.
    #[must_use]
    pub fn frames(&self) -> Option<usize> {
        match self {
            Self::Wav { samples, looping: false, .. } => Some(samples.len()),
            _ => None,
        }
    }

    #[must_use]
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::Tone { .. } => "generated tone",
            Self::Wav { .. } => "WAV replay",
            Self::Silence => "digital silence",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_has_the_requested_amplitude_and_period() {
        // 1 kHz at 48 kHz is 48 frames per cycle.
        let mut source = Source::tone(1_000.0, 0.5, 48_000);
        let mut frames = vec![0.0_f32; 48 * 4];
        assert!(source.fill(&mut frames));

        let peak = frames.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak was {peak}");

        // Four whole cycles means four zero-crossings upward.
        let rising = frames
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        assert_eq!(rising, 4, "expected four cycles, got {rising}");
    }

    #[test]
    fn a_tone_stays_bounded_over_a_long_run() {
        // Phase wrapping is what keeps this true; without it the sine degrades
        // as the accumulated phase grows past f32's useful resolution.
        let mut source = Source::tone(1_000.0, 1.0, 48_000);
        let mut frames = vec![0.0_f32; 48_000];
        for _ in 0..600 {
            source.fill(&mut frames);
        }
        let peak = frames.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()));
        assert!((peak - 1.0).abs() < 0.01, "after ten minutes the peak was {peak}");
    }

    #[test]
    fn silence_is_exactly_zero_not_merely_quiet() {
        let mut source = Source::Silence;
        let mut frames = vec![1.0_f32; 16];
        source.fill(&mut frames);
        assert!(frames.iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn an_exhausted_source_reports_it_and_pads_with_silence() {
        let mut source = Source::Wav {
            samples: vec![0.5, -0.5, 0.25],
            position: 0,
            looping: false,
        };
        let mut frames = vec![9.0_f32; 5];
        assert!(!source.fill(&mut frames), "three samples cannot fill five");
        assert_eq!(frames, vec![0.5, -0.5, 0.25, 0.0, 0.0]);
    }

    #[test]
    fn a_looping_source_wraps_and_never_reports_exhaustion() {
        let mut source = Source::Wav {
            samples: vec![1.0, 2.0],
            position: 0,
            looping: true,
        };
        let mut frames = vec![0.0_f32; 5];
        assert!(source.fill(&mut frames));
        assert_eq!(frames, vec![1.0, 2.0, 1.0, 2.0, 1.0]);
    }
}

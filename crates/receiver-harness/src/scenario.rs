//! A whole receiver run, start to finish, on a virtual clock.
//!
//! The real audio path — the real packetiser, the real jitter buffer, the real
//! concealment, resampler, drift controller and monitor chain — driven by two
//! independent clocks with a configurable network in between. Nothing is
//! stubbed except the passage of time and the socket.

use anyhow::Result;
use receiver_core::receiver::{Preset, ReceiverConfig, Telemetry};
use receiver_core::dsp::MonitorSettings;
use reference_transmitter::impairment::{Impairment, ImpairmentConfig, Verdict};
use reference_transmitter::source::Source;
use reference_transmitter::transmit::{TransmitConfig, Transmitter};

use crate::clock::{Event, VirtualClock};

/// Everything a run needs.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub sample_rate: u32,
    pub frame_duration_ms: f64,
    /// Frames the DAC asks for per callback. Often larger than one packet.
    pub block_frames: usize,
    pub duration_secs: f64,
    /// Sender clock error, in parts per million.
    pub sender_ppm: f64,
    /// Receiver DAC clock error, in parts per million.
    pub dac_ppm: f64,
    /// Network delay, in milliseconds: a fixed base plus uniform jitter.
    pub base_delay_ms: f64,
    pub jitter_ms: f64,
    pub impairment: ImpairmentConfig,
    pub preset: Preset,
    pub tone_hz: f32,
    pub seed: u64,
    /// Start a new session after this many packets.
    pub session_change_after: Option<u64>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            frame_duration_ms: 5.0,
            block_frames: 240,
            duration_secs: 10.0,
            sender_ppm: 0.0,
            dac_ppm: 0.0,
            base_delay_ms: 2.0,
            jitter_ms: 0.0,
            impairment: ImpairmentConfig::default(),
            preset: Preset::Normal,
            tone_hz: 1_000.0,
            seed: 1,
            session_change_after: None,
        }
    }
}

impl Scenario {
    fn frames_per_packet(&self) -> usize {
        audio_core::frames_per_packet(self.sample_rate, self.frame_duration_ms)
    }
}

/// What a run produced.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Packets the sender built.
    pub sent: u64,
    /// Packets the network delivered, duplicates included.
    pub delivered: u64,
    /// Packets the impairment model dropped.
    pub dropped: u64,
    pub render_calls: u64,
    pub frames_rendered: u64,
    pub telemetry: Telemetry,
    /// Buffer depth, sampled once per render call.
    pub depth_ms: Vec<f64>,
    /// Drift correction, sampled once per render call.
    pub correction_ppm: Vec<f64>,
    /// Instantaneous correction, sampled once per render call.
    pub instant_ppm: Vec<f64>,
    /// The rendered audio.
    pub audio: Vec<f32>,
}

impl Outcome {
    /// Depth statistics over the steady state, which is where a drift or
    /// ratchet problem shows up.
    ///
    /// The first two seconds are excluded. The buffer is filling then, by
    /// design, so including them would report a minimum depth of one packet
    /// for every run — including a perfect one — and make the measurement say
    /// nothing.
    #[must_use]
    pub fn depth_summary(&self) -> (f64, f64, f64) {
        let settle = self.depth_ms.len().min(400);
        let playing: Vec<f64> = self.depth_ms[settle..]
            .iter()
            .copied()
            .filter(|&depth| depth > 0.0)
            .collect();
        if playing.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let min = playing.iter().copied().fold(f64::MAX, f64::min);
        let max = playing.iter().copied().fold(0.0, f64::max);
        let mean = playing.iter().sum::<f64>() / playing.len() as f64;
        (min, mean, max)
    }

    /// Mean correction actually applied, over the steady state.
    ///
    /// The robust figure for "what rate did the resampler run at". A single
    /// reading of the instantaneous correction is not: it follows ordinary
    /// buffer-depth variation, and a few milliseconds of depth becomes tens of
    /// ppm through the proportional term.
    #[must_use]
    pub fn mean_applied_ppm(&self) -> f64 {
        let settle = self.instant_ppm.len().min(400);
        let steady = &self.instant_ppm[settle..];
        if steady.is_empty() {
            return 0.0;
        }
        steady.iter().sum::<f64>() / steady.len() as f64
    }

    /// Peak absolute sample, over the audio after the startup fade.
    #[must_use]
    pub fn steady_peak(&self) -> f32 {
        let skip = self.audio.len().min(4_800);
        self.audio[skip..]
            .iter()
            .fold(0.0_f32, |acc, &s| acc.max(s.abs()))
    }

    /// Longest run of samples at digital silence, in milliseconds.
    ///
    /// The measurement a listener would describe as "it dropped out".
    #[must_use]
    pub fn longest_silence_ms(&self, sample_rate: u32) -> f64 {
        let mut longest = 0_usize;
        let mut current = 0_usize;
        // Skip the startup fill, which is silent by design.
        let skip = self.audio.len().min(9_600);
        for &sample in &self.audio[skip..] {
            if sample.abs() < 1e-4 {
                current += 1;
                longest = longest.max(current);
            } else {
                current = 0;
            }
        }
        longest as f64 * 1_000.0 / f64::from(sample_rate)
    }
}

/// Runs a scenario to completion.
pub fn run(scenario: &Scenario) -> Result<Outcome> {
    let frames_per_packet = scenario.frames_per_packet();

    let mut transmitter = Transmitter::new(
        TransmitConfig {
            sample_rate: scenario.sample_rate,
            frame_duration_ms: scenario.frame_duration_ms,
            session_id: 0x5EED,
            packet_limit: None,
            session_change_after: None,
            clock_ppm: 0.0,
            jitter_ms: 0.0,
            end_of_stream: false,
            encrypted: false,
            impairment: ImpairmentConfig::default(),
            seed: scenario.seed,
        },
        Source::tone(scenario.tone_hz, 0.5, scenario.sample_rate),
    );

    let mut impairment = Impairment::new(scenario.impairment, scenario.seed);
    // A separate stream of randomness for arrival jitter, so changing the
    // jitter does not shift the loss pattern a previous run found a bug with.
    let mut jitter = JitterSource::new(scenario.seed ^ 0xA5A5_A5A5);

    let mut config = ReceiverConfig::new(scenario.sample_rate, frames_per_packet);
    config.preset = scenario.preset;
    let (mut net, mut audio) = receiver_core::receiver::new(config);
    audio.apply_settings(MonitorSettings::default());

    let mut clock = VirtualClock::new(
        scenario.frame_duration_ms * 1_000.0,
        scenario.block_frames as f64 * 1e6 / f64::from(scenario.sample_rate),
        scenario.sender_ppm,
        scenario.dac_ppm,
        scenario.duration_secs,
    );

    let mut outcome = Outcome {
        sent: 0,
        delivered: 0,
        dropped: 0,
        render_calls: 0,
        frames_rendered: 0,
        telemetry: Telemetry::default(),
        depth_ms: Vec::new(),
        correction_ppm: Vec::new(),
        instant_ppm: Vec::new(),
        audio: Vec::new(),
    };

    let mut block = vec![0.0_f32; scenario.block_frames];
    let mut held: Option<Vec<u8>> = None;

    loop {
        match clock.next_event() {
            Event::SendPacket => {
                let Some(datagram) = transmitter.next_datagram() else {
                    // The source ran out. Close the stream properly so the
                    // receiver stops rather than concealing to the end.
                    let marker = transmitter.end_of_stream_datagram();
                    clock.deliver_after(marker, 0);
                    clock.finish_sending();
                    continue;
                };
                outcome.sent += 1;

                if scenario
                    .session_change_after
                    .is_some_and(|after| outcome.sent == after)
                {
                    transmitter.rotate_session_now();
                }

                impairment.advance(scenario.frame_duration_ms.round() as u64);

                // Each packet gets its own arrival delay, so two packets sent
                // together can still arrive out of order — which is how
                // reordering really happens on a network.
                let delay = |jitter: &mut JitterSource| {
                    ((scenario.base_delay_ms + jitter.next(scenario.jitter_ms)) * 1_000.0) as u64
                };

                match impairment.next_verdict() {
                    Verdict::Send => {
                        let at = delay(&mut jitter);
                        clock.deliver_after(datagram, at);
                        if let Some(previous) = held.take() {
                            let at = delay(&mut jitter);
                            clock.deliver_after(previous, at);
                        }
                    }
                    Verdict::Drop => {
                        outcome.dropped += 1;
                        if let Some(previous) = held.take() {
                            let at = delay(&mut jitter);
                            clock.deliver_after(previous, at);
                        }
                    }
                    Verdict::Duplicate => {
                        let first = delay(&mut jitter);
                        let second = delay(&mut jitter);
                        clock.deliver_after(datagram.clone(), first);
                        clock.deliver_after(datagram, second);
                    }
                    Verdict::Delay => {
                        if let Some(previous) = held.replace(datagram) {
                            let at = delay(&mut jitter);
                            clock.deliver_after(previous, at);
                        }
                    }
                }
            }
            Event::Deliver => {
                if let Some(datagram) = clock.take_delivery() {
                    outcome.delivered += 1;
                    net.accept(&datagram, clock.now_us());
                }
            }
            Event::RenderBlock => {
                audio.render(&mut block);
                outcome.render_calls += 1;
                outcome.frames_rendered += block.len() as u64;
                outcome.audio.extend_from_slice(&block);

                let telemetry = audio.telemetry();
                outcome.depth_ms.push(telemetry.buffer_health_ms);
                outcome.correction_ppm.push(telemetry.resample_ratio_ppm);
                outcome.instant_ppm.push(telemetry.instant_correction_ppm);
            }
            Event::Done => break,
        }
    }

    outcome.telemetry = audio.telemetry();
    Ok(outcome)
}

/// Uniform jitter from a small deterministic generator.
///
/// Its own implementation rather than a `rand` dependency: the harness needs
/// a second independent stream and nothing more, and keeping it here means the
/// numbers are reproducible across `rand` versions. A test fixture that
/// changes its own behaviour when a dependency updates is not reproducible.
struct JitterSource {
    state: u64,
}

impl JitterSource {
    const fn new(seed: u64) -> Self {
        // Never zero: xorshift is stuck at zero.
        Self { state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed } }
    }

    /// A value in `0.0..span`.
    fn next(&mut self, span: f64) -> f64 {
        if span <= 0.0 {
            return 0.0;
        }
        // xorshift64*, which is more than enough for arrival jitter.
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (value >> 11) as f64 / (1_u64 << 53) as f64 * span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jitter_source_stays_inside_its_span_and_is_reproducible() {
        let draws = |seed| {
            let mut source = JitterSource::new(seed);
            (0..1_000).map(|_| source.next(10.0)).collect::<Vec<_>>()
        };
        let first = draws(7);
        assert!(first.iter().all(|&value| (0.0..10.0).contains(&value)));
        assert_eq!(first, draws(7));
        assert_ne!(first, draws(8));

        let mean = first.iter().sum::<f64>() / first.len() as f64;
        assert!((mean - 5.0).abs() < 0.5, "mean was {mean}");

        // A zero span is no jitter at all, not a panic.
        assert_eq!(JitterSource::new(1).next(0.0), 0.0);
    }
}

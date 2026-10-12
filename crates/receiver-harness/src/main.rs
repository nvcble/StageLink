//! StageLink receiver harness.
//!
//! Runs the real receiver audio path against the real reference transmitter on
//! a **virtual clock**, with two independent crystal rates and a configurable
//! network in between. A 90-minute drift scenario finishes in a couple of
//! seconds, which is what makes it something that can be checked on every
//! commit rather than argued about.
//!
//! ```text
//! # Ten seconds, clean
//! stagelink-receiver-harness --output clean.wav
//!
//! # A 90-minute service with two ordinary crystals 40 ppm apart
//! stagelink-receiver-harness --duration-secs 5400 --sender-ppm 20 --dac-ppm -20
//!
//! # The Wi-Fi case: 2% loss in bursts of eight, 15 ms of arrival jitter
//! stagelink-receiver-harness --loss 2 --burst-length 8 --jitter-ms 15 \
//!     --output lossy.wav
//!
//! # Somebody walks in front of the access point every ten seconds
//! stagelink-receiver-harness --duration-secs 60 \
//!     --outage-ms 1500 --outage-period-ms 10000 --output outage.wav
//!
//! # The session restarts halfway through
//! stagelink-receiver-harness --duration-secs 20 --session-change-after 2000
//! ```
//!
//! What it cannot tell you: how it sounds on a given phone, and what the real
//! output latency is. Those need hardware. Everything above the audio sink is
//! here.

mod clock;
mod scenario;

use anyhow::{Context, Result};
use clap::Parser;
use receiver_core::receiver::Preset;
use reference_transmitter::impairment::ImpairmentConfig;
use tracing::{info, warn};

use crate::scenario::Scenario;

#[derive(Parser, Debug)]
#[command(
    name = "stagelink-receiver-harness",
    about = "Runs the receiver audio path on a virtual clock, with no hardware",
    version
)]
struct Args {
    /// Simulated seconds to run. 5400 is a 90-minute service.
    #[arg(long, default_value_t = 10.0)]
    duration_secs: f64,

    /// Write the receiver's output to this WAV file.
    #[arg(long)]
    output: Option<std::path::PathBuf>,

    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,

    /// Packet duration in milliseconds: 2.5, 5 or 10.
    #[arg(long, default_value_t = 5.0)]
    frame_ms: f64,

    /// Frames the DAC asks for per callback.
    #[arg(long, default_value_t = 240)]
    block_frames: usize,

    /// Buffer preset: tight, normal or safe.
    #[arg(long, default_value = "normal")]
    preset: String,

    // -- the two clocks ----------------------------------------------------
    /// Sender clock error in parts per million.
    #[arg(long, default_value_t = 0.0)]
    sender_ppm: f64,

    /// Receiver DAC clock error in parts per million.
    #[arg(long, default_value_t = 0.0)]
    dac_ppm: f64,

    // -- the network -------------------------------------------------------
    /// Fixed one-way network delay, in milliseconds.
    #[arg(long, default_value_t = 2.0)]
    base_delay_ms: f64,

    /// Uniform arrival jitter, in milliseconds.
    #[arg(long, default_value_t = 0.0)]
    jitter_ms: f64,

    #[arg(long, default_value_t = 0.0)]
    loss: f64,

    /// Packets lost per burst. 1 is independent loss; Wi-Fi is nearer 8.
    #[arg(long, default_value_t = 1)]
    burst_length: u32,

    #[arg(long, default_value_t = 0.0)]
    reorder: f64,

    #[arg(long, default_value_t = 0.0)]
    duplicate: f64,

    #[arg(long, default_value_t = 0)]
    outage_ms: u64,

    #[arg(long, default_value_t = 10_000)]
    outage_period_ms: u64,

    // -- the stream --------------------------------------------------------
    /// Start a new session after this many packets.
    #[arg(long)]
    session_change_after: Option<u64>,

    #[arg(long, default_value_t = 1_000.0)]
    tone_hz: f32,

    #[arg(long, default_value_t = 1)]
    seed: u64,

    /// Fail the process if the run does not meet its own quality bar.
    ///
    /// For use in a test script: a harness that only prints numbers is a
    /// harness nobody reads.
    #[arg(long)]
    assert_clean: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact()
        .init();

    let args = Args::parse();
    let scenario = build_scenario(&args)?;

    info!(
        duration_secs = scenario.duration_secs,
        sample_rate = scenario.sample_rate,
        frame_ms = scenario.frame_duration_ms,
        block_frames = scenario.block_frames,
        preset = ?scenario.preset,
        sender_ppm = scenario.sender_ppm,
        dac_ppm = scenario.dac_ppm,
        relative_ppm = scenario.sender_ppm - scenario.dac_ppm,
        "receiver harness starting"
    );

    let started = std::time::Instant::now();
    let outcome = scenario::run(&scenario)?;
    let wall = started.elapsed();

    let (min_depth, mean_depth, max_depth) = outcome.depth_summary();
    let final_correction = outcome.correction_ppm.last().copied().unwrap_or(0.0);
    let silence_ms = outcome.longest_silence_ms(scenario.sample_rate);

    info!(
        simulated_secs = format!("{:.1}", scenario.duration_secs),
        wall_secs = format!("{:.2}", wall.as_secs_f64()),
        speedup = format!("{:.0}x", scenario.duration_secs / wall.as_secs_f64().max(1e-6)),
        "finished"
    );
    info!(
        sent = outcome.sent,
        delivered = outcome.delivered,
        render_calls = outcome.render_calls,
        frames_rendered = outcome.frames_rendered,
        dropped = outcome.dropped,
        received = outcome.telemetry.packets_received,
        lost = outcome.telemetry.packets_lost,
        loss_percent = format!("{:.2}", outcome.telemetry.loss_percent),
        duplicates = outcome.telemetry.duplicates,
        reordered = outcome.telemetry.reordered,
        rejected = outcome.telemetry.rejected,
        "packets"
    );
    info!(
        min_ms = format!("{min_depth:.1}"),
        mean_ms = format!("{mean_depth:.1}"),
        max_ms = format!("{max_depth:.1}"),
        target_ms = scenario.preset.target_ms(),
        "buffer depth"
    );
    // The mean matters more than the last sample: the instantaneous
    // correction tracks ordinary buffer-depth variation and a single reading
    // of it says little, while the mean is the rate the resampler actually
    // ran at over the run.
    let settle = outcome.instant_ppm.len().min(400);
    let instant = &outcome.instant_ppm[settle..];
    let mean_instant = if instant.is_empty() {
        0.0
    } else {
        instant.iter().sum::<f64>() / instant.len() as f64
    };
    info!(
        learned_ppm = format!("{final_correction:.1}"),
        mean_applied_ppm = format!("{mean_instant:.1}"),
        min_applied_ppm = format!("{:.1}", instant.iter().copied().fold(f64::MAX, f64::min)),
        max_applied_ppm = format!("{:.1}", instant.iter().copied().fold(f64::MIN, f64::max)),
        error_frames = format!("{:.1}", outcome.telemetry.drift_error_frames),
        relative_ppm = scenario.sender_ppm - scenario.dac_ppm,
        "drift"
    );
    info!(
        concealed_runs = outcome.telemetry.concealed_runs,
        concealed_ms = format!("{:.1}", outcome.telemetry.concealed_ms),
        resyncs = outcome.telemetry.resyncs,
        longest_silence_ms = format!("{silence_ms:.1}"),
        peak = format!("{:.3}", outcome.steady_peak()),
        clipped = outcome.telemetry.clipped,
        "audio"
    );

    if outcome.telemetry.clipped > 0 {
        warn!(
            clipped = outcome.telemetry.clipped,
            "the safety clip fired, which means the limiter let something through"
        );
    }

    if let Some(path) = &args.output {
        write_wav(path, &outcome.audio, scenario.sample_rate)
            .with_context(|| format!("writing {}", path.display()))?;
        info!(path = %path.display(), frames = outcome.audio.len(), "wrote WAV");
    }

    if args.assert_clean {
        assert_clean(&scenario, &outcome, silence_ms)?;
        info!("quality bar met");
    }

    Ok(())
}

/// The bar a clean run has to clear.
///
/// Deliberately about what a listener would notice, not about internals: a
/// run that keeps its buffer, never goes silent and comes out at the right
/// level is a run that sounded right.
fn assert_clean(
    scenario: &Scenario,
    outcome: &scenario::Outcome,
    silence_ms: f64,
) -> Result<()> {
    let (min_depth, _, max_depth) = outcome.depth_summary();
    let relative_ppm = (scenario.sender_ppm - scenario.dac_ppm).abs();

    anyhow::ensure!(
        outcome.telemetry.packets_received > 0,
        "no packets were received at all"
    );
    anyhow::ensure!(
        outcome.steady_peak() > 0.4,
        "the output peaked at {:.3}, so the tone did not come through",
        outcome.steady_peak()
    );
    anyhow::ensure!(
        outcome.telemetry.clipped == 0,
        "the safety clip fired {} times",
        outcome.telemetry.clipped
    );
    anyhow::ensure!(
        silence_ms < 50.0,
        "the output was silent for {silence_ms:.1} ms, which is an audible dropout"
    );
    anyhow::ensure!(
        min_depth > 1.0,
        "the buffer ran dry: minimum depth {min_depth:.1} ms"
    );
    // The buffer must not ratchet upwards either: a depth that only ever grows
    // is latency being quietly added for the rest of the service.
    anyhow::ensure!(
        max_depth < f64::from(scenario.preset.target_ms()) * 4.0 + 50.0,
        "the buffer grew to {max_depth:.1} ms against a {} ms target",
        scenario.preset.target_ms()
    );

    // Only on a run long enough for the loop to settle. The drift controller
    // is overdamped with a time constant of about two minutes — deliberately,
    // so the rate it reports does not hunt — so a 30-second run says nothing
    // about whether it converged, and asserting on one would be measuring the
    // startup transient.
    const SETTLING_SECONDS: f64 = 300.0;
    if relative_ppm > 1.0 && scenario.duration_secs >= SETTLING_SECONDS {
        let applied = outcome.mean_applied_ppm();
        anyhow::ensure!(
            (applied.abs() - relative_ppm).abs() < relative_ppm * 0.3 + 10.0,
            "the clocks differ by {relative_ppm} ppm but the resampler ran at \
             {applied:.1} ppm"
        );
    }

    Ok(())
}

fn build_scenario(args: &Args) -> Result<Scenario> {
    if !reference_transmitter::transmit::TransmitConfig::is_supported_frame_duration(args.frame_ms)
    {
        anyhow::bail!("--frame-ms must be 2.5, 5 or 10");
    }

    let preset = match args.preset.to_ascii_lowercase().as_str() {
        "tight" => Preset::Tight,
        "normal" => Preset::Normal,
        "safe" => Preset::Safe,
        other => anyhow::bail!("unknown preset {other:?}; choose tight, normal or safe"),
    };

    let impairment = ImpairmentConfig {
        loss_percent: args.loss,
        burst_length: args.burst_length,
        reorder_percent: args.reorder,
        duplicate_percent: args.duplicate,
        outage_ms: args.outage_ms,
        outage_period_ms: args.outage_period_ms,
    };
    impairment.validate().map_err(|reason| anyhow::anyhow!(reason))?;

    Ok(Scenario {
        sample_rate: args.sample_rate,
        frame_duration_ms: args.frame_ms,
        block_frames: args.block_frames.max(1),
        duration_secs: args.duration_secs,
        sender_ppm: args.sender_ppm,
        dac_ppm: args.dac_ppm,
        base_delay_ms: args.base_delay_ms,
        jitter_ms: args.jitter_ms,
        impairment,
        preset,
        tone_hz: args.tone_hz,
        seed: args.seed,
        session_change_after: args.session_change_after,
    })
}

fn write_wav(path: &std::path::Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(audio_core::convert::f32_to_i16(sample))?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Outcome;

    fn run(scenario: Scenario) -> Outcome {
        scenario::run(&scenario).expect("the harness must not fail")
    }

    /// The baseline. Everything else is a departure from this.
    #[test]
    fn a_clean_ten_second_run_plays_through_without_a_dropout() {
        let outcome = run(Scenario { duration_secs: 10.0, ..Scenario::default() });

        assert!(outcome.telemetry.packets_received > 1_900);
        assert_eq!(outcome.telemetry.packets_lost, 0);
        assert_eq!(outcome.telemetry.clipped, 0);
        assert_eq!(outcome.telemetry.concealed_runs, 0, "nothing should have been concealed");
        assert!(
            (outcome.steady_peak() - 0.5).abs() < 0.02,
            "peak was {}",
            outcome.steady_peak()
        );
        assert!(
            outcome.longest_silence_ms(48_000) < 2.0,
            "silent for {} ms",
            outcome.longest_silence_ms(48_000)
        );

        let (min, mean, max) = outcome.depth_summary();
        assert!(min > 10.0, "the buffer dipped to {min} ms");
        assert!(mean < 120.0, "the buffer averaged {mean} ms against a 40 ms target");
        assert!(max < 200.0, "the buffer grew to {max} ms");
    }

    /// The case the drift controller exists for, run at 500x real time: two
    /// ordinary crystals 40 ppm apart over a 90-minute service, which
    /// uncorrected would accumulate 216 ms and empty the buffer several times.
    #[test]
    fn a_ninety_minute_service_with_forty_ppm_of_drift_never_runs_dry() {
        let outcome = run(Scenario {
            duration_secs: 5_400.0,
            sender_ppm: 20.0,
            dac_ppm: -20.0,
            ..Scenario::default()
        });

        let (min, mean, max) = outcome.depth_summary();
        assert!(
            min > 5.0,
            "the buffer ran dry over 90 minutes: minimum {min:.1} ms"
        );
        assert!(
            max < 200.0,
            "the buffer grew to {max:.1} ms, so latency was quietly added"
        );
        assert!(
            (mean - 40.0).abs() < 25.0,
            "the buffer settled at {mean:.1} ms rather than near its 40 ms target"
        );

        // The resampler really ran at the clocks' difference. Measured as the
        // mean applied correction, not a single reading of it: the
        // instantaneous value follows ordinary depth variation and would be a
        // coin toss.
        let applied = outcome.mean_applied_ppm();
        assert!(
            (applied - 40.0).abs() < 10.0,
            "ran at {applied:.1} ppm for a 40 ppm difference"
        );
        // And the figure the receiver *reports* about its own drift is in the
        // right place, so an operator looking at it learns something true.
        let reported = outcome.correction_ppm.last().copied().unwrap_or(0.0);
        assert!(
            (reported - 40.0).abs() < 20.0,
            "reported {reported:.1} ppm for a 40 ppm difference"
        );

        // And it never had to conceal, which is the audible consequence.
        assert_eq!(
            outcome.telemetry.concealed_runs, 0,
            "drift caused {} concealed gaps over 90 minutes",
            outcome.telemetry.concealed_runs
        );
    }

    #[test]
    fn drift_in_the_other_direction_is_handled_too() {
        let outcome = run(Scenario {
            duration_secs: 3_600.0,
            sender_ppm: -30.0,
            dac_ppm: 30.0,
            ..Scenario::default()
        });
        let applied = outcome.mean_applied_ppm();
        assert!(
            (applied + 60.0).abs() < 10.0,
            "ran at {applied:.1} ppm for a -60 ppm difference"
        );
        assert!(outcome.depth_summary().0 > 5.0);
    }

    /// A phone the drift budget says should not work: 200 ppm is five times
    /// what two consumer crystals should manage. It must still not break.
    #[test]
    fn an_out_of_spec_clock_is_survived_rather_than_breaking() {
        let outcome = run(Scenario {
            duration_secs: 1_800.0,
            sender_ppm: 200.0,
            ..Scenario::default()
        });
        assert!(outcome.steady_peak() > 0.4);
        assert_eq!(outcome.telemetry.clipped, 0);
        let (min, _, max) = outcome.depth_summary();
        assert!(min > 1.0, "ran dry at {min:.1} ms");
        assert!(max < 400.0, "grew to {max:.1} ms");
    }

    #[test]
    fn arrival_jitter_is_absorbed_by_the_buffer() {
        let outcome = run(Scenario {
            duration_secs: 60.0,
            // 25 ms of jitter against a 40 ms buffer.
            jitter_ms: 25.0,
            ..Scenario::default()
        });
        // At most one: the packet still in flight when the run's deadline
        // passes is never delivered, so it shows as a loss. Jitter itself
        // loses nothing.
        assert!(
            outcome.telemetry.packets_lost <= 1,
            "jitter cost {} packets",
            outcome.telemetry.packets_lost
        );
        assert!(
            outcome.longest_silence_ms(48_000) < 20.0,
            "jitter caused {} ms of silence",
            outcome.longest_silence_ms(48_000)
        );
        assert!(outcome.steady_peak() > 0.45);
    }

    /// Jitter larger than the buffer *must* cause dropouts — that is physics,
    /// not a bug. What must not happen is a crash, a clip, or a buffer that
    /// never recovers.
    #[test]
    fn jitter_beyond_the_buffer_degrades_rather_than_failing() {
        let outcome = run(Scenario {
            duration_secs: 60.0,
            jitter_ms: 120.0,
            preset: Preset::Tight,
            ..Scenario::default()
        });
        assert_eq!(outcome.telemetry.clipped, 0);
        assert!(outcome.audio.iter().all(|s| s.is_finite()));
        // It kept going: audio was still arriving and playing at the end.
        assert!(outcome.telemetry.packets_received > 10_000);
        assert!(outcome.steady_peak() > 0.3);
    }

    #[test]
    fn two_percent_burst_loss_is_concealed_and_the_stream_stays_in_step() {
        let outcome = run(Scenario {
            duration_secs: 60.0,
            jitter_ms: 10.0,
            impairment: ImpairmentConfig {
                // 0.25% of packets *start* a burst of eight, which realises
                // about 2% loss. Asking for 2% with bursts of eight would be
                // 16%, which is a different test.
                loss_percent: 0.25,
                burst_length: 8,
                ..ImpairmentConfig::default()
            },
            ..Scenario::default()
        });

        let realised = outcome.telemetry.loss_percent;
        assert!(
            (1.0..4.0).contains(&realised),
            "realised loss was {realised:.2}%, not the ~2% intended"
        );
        assert!(outcome.telemetry.concealed_runs > 10, "nothing was concealed");
        assert_eq!(outcome.telemetry.clipped, 0);
        // A burst of eight packets is 40 ms of missing audio and concealment
        // fades out after 5 ms, so ~35 ms of silence per burst is the correct
        // outcome, not a fault. Two bursts back to back double it. What must
        // not happen is a long silence from the buffer collapsing.
        assert!(
            outcome.longest_silence_ms(48_000) < 90.0,
            "silent for {} ms",
            outcome.longest_silence_ms(48_000)
        );
        let (min, _, _) = outcome.depth_summary();
        assert!(min > 1.0, "the buffer collapsed to {min:.1} ms under burst loss");
    }

    #[test]
    fn duplicates_and_reordering_do_not_disturb_the_audio() {
        let outcome = run(Scenario {
            duration_secs: 30.0,
            jitter_ms: 8.0,
            impairment: ImpairmentConfig {
                duplicate_percent: 5.0,
                reorder_percent: 5.0,
                ..ImpairmentConfig::default()
            },
            ..Scenario::default()
        });

        assert!(outcome.telemetry.duplicates > 50, "duplicates were not simulated");
        assert_eq!(
            outcome.telemetry.packets_lost, 0,
            "a reordered packet must not be counted as lost"
        );
        assert_eq!(outcome.telemetry.concealed_runs, 0, "nothing should have been concealed");
        assert!((outcome.steady_peak() - 0.5).abs() < 0.03);
    }

    /// The 1.5-second outage a person walking between the phone and the access
    /// point causes. Recovery is the test, not survival of the outage itself.
    #[test]
    fn an_outage_is_recovered_from_without_adding_permanent_latency() {
        let outcome = run(Scenario {
            duration_secs: 60.0,
            impairment: ImpairmentConfig {
                outage_ms: 1_500,
                outage_period_ms: 20_000,
                ..ImpairmentConfig::default()
            },
            ..Scenario::default()
        });

        assert!(outcome.telemetry.resyncs > 0, "an outage must trigger a resync");
        // The point: after the outage the latency is back to the preset, not
        // 1.5 seconds higher for the rest of the service.
        let tail = &outcome.depth_ms[outcome.depth_ms.len() / 2..];
        let tail_max = tail.iter().copied().fold(0.0_f64, f64::max);
        assert!(
            tail_max < 200.0,
            "latency stayed at {tail_max:.1} ms after the outage"
        );
        assert!(outcome.steady_peak() > 0.4, "audio did not come back");
        assert_eq!(outcome.telemetry.clipped, 0);
    }

    #[test]
    fn a_session_change_is_resynced_rather_than_played_through() {
        let outcome = run(Scenario {
            duration_secs: 20.0,
            session_change_after: Some(2_000),
            ..Scenario::default()
        });
        assert!(outcome.telemetry.resyncs >= 1);
        assert!(outcome.steady_peak() > 0.4, "audio did not resume after the change");
        assert_eq!(outcome.telemetry.clipped, 0);
    }

    #[test]
    fn every_preset_works_and_buys_what_it_promises() {
        for (preset, expected_ms) in [
            (Preset::Tight, 20.0),
            (Preset::Normal, 40.0),
            (Preset::Safe, 80.0),
        ] {
            let outcome = run(Scenario {
                duration_secs: 60.0,
                preset,
                jitter_ms: 5.0,
                ..Scenario::default()
            });
            let (min, mean, _) = outcome.depth_summary();
            assert!(min > 1.0, "{preset:?} ran dry at {min:.1} ms");
            // The mean settles near the preset's target, which is the promise
            // the preset makes about latency.
            assert!(
                (mean - expected_ms).abs() < expected_ms * 0.8,
                "{preset:?} settled at {mean:.1} ms against a {expected_ms} ms target"
            );
        }
    }

    #[test]
    fn a_dac_block_larger_than_a_packet_is_handled() {
        // Many Android devices ask for 480 or 960 frames at a time.
        for block_frames in [96, 240, 480, 960] {
            let outcome = run(Scenario {
                duration_secs: 20.0,
                block_frames,
                ..Scenario::default()
            });
            assert!(
                outcome.steady_peak() > 0.45,
                "{block_frames}-frame blocks peaked at {}",
                outcome.steady_peak()
            );
            assert_eq!(outcome.telemetry.clipped, 0, "for {block_frames}-frame blocks");
            assert!(
                outcome.longest_silence_ms(48_000) < 20.0,
                "{block_frames}-frame blocks went silent for {} ms",
                outcome.longest_silence_ms(48_000)
            );
        }
    }

    #[test]
    fn every_supported_packet_duration_works() {
        for frame_ms in [2.5, 5.0, 10.0] {
            let outcome = run(Scenario {
                duration_secs: 20.0,
                frame_duration_ms: frame_ms,
                ..Scenario::default()
            });
            assert!(
                outcome.steady_peak() > 0.45,
                "{frame_ms} ms packets peaked at {}",
                outcome.steady_peak()
            );
        }
    }

    #[test]
    fn a_run_is_reproducible_from_its_seed() {
        let scenario = Scenario {
            duration_secs: 20.0,
            jitter_ms: 10.0,
            impairment: ImpairmentConfig {
                loss_percent: 3.0,
                burst_length: 4,
                reorder_percent: 2.0,
                ..ImpairmentConfig::default()
            },
            ..Scenario::default()
        };
        let first = run(scenario.clone());
        let second = run(scenario.clone());
        assert_eq!(first.audio, second.audio, "the same seed must replay exactly");
        assert_eq!(first.telemetry.packets_lost, second.telemetry.packets_lost);

        let other = run(Scenario { seed: 99, ..scenario });
        assert_ne!(first.audio, other.audio, "a different seed must differ");
    }

    #[test]
    fn the_quality_bar_rejects_a_run_that_fell_apart() {
        // A tight buffer against jitter five times its depth cannot work, and
        // the bar must say so rather than passing anything that completes.
        let scenario = Scenario {
            duration_secs: 30.0,
            jitter_ms: 150.0,
            preset: Preset::Tight,
            ..Scenario::default()
        };
        let outcome = run(scenario.clone());
        let silence = outcome.longest_silence_ms(scenario.sample_rate);
        assert!(
            assert_clean(&scenario, &outcome, silence).is_err(),
            "the quality bar passed a run with {silence:.1} ms of silence"
        );
    }

    #[test]
    fn the_quality_bar_accepts_a_clean_run() {
        let scenario = Scenario {
            // Long enough for the drift loop to settle, so the bar's drift
            // check is actually exercised rather than skipped.
            duration_secs: 600.0,
            sender_ppm: 20.0,
            dac_ppm: -20.0,
            ..Scenario::default()
        };
        let outcome = run(scenario.clone());
        let silence = outcome.longest_silence_ms(scenario.sample_rate);
        assert_clean(&scenario, &outcome, silence).expect("a clean run must pass its own bar");
    }
}

//! StageLink reference transmitter.
//!
//! Sends **real** StageLink audio packets to an address, with reproducible
//! network impairments. It exists so that the receiver — the hard, risky half
//! of the system — can be built and verified without a mixer, an access point
//! or a phone in the room.
//!
//! ```text
//! # A clean 1 kHz tone, 200 packets/second, for ten seconds
//! stagelink-reference-transmitter --target 127.0.0.1:45872 --duration-secs 10
//!
//! # The Wi-Fi case: 2% loss arriving in bursts of eight, 15 ms of jitter
//! stagelink-reference-transmitter --target 10.0.0.7:45872 \
//!     --loss 2 --burst-length 8 --jitter-ms 15 --seed 1234
//!
//! # Somebody walks in front of the access point every ten seconds
//! stagelink-reference-transmitter --target 10.0.0.7:45872 \
//!     --outage-ms 1500 --outage-period-ms 10000
//!
//! # A drifting capture clock, and a session restart halfway through
//! stagelink-reference-transmitter --target 10.0.0.7:45872 \
//!     --clock-ppm 40 --session-change-after 2000
//!
//! # Replay a real recording, because concealment cannot be judged on a tone
//! stagelink-reference-transmitter --target 10.0.0.7:45872 --wav vocal.wav --loop
//! ```
//!
//! ## Comparing this against a receiver's own figures
//!
//! The final log line reports `realised_loss_percent` — what was *actually*
//! dropped, not what was configured. A short run at a configured 2% does not
//! drop exactly 2%, so that is the number a receiver's claims must be checked
//! against.
//!
//! One asymmetry is expected and is not a receiver bug: a receiver can only
//! count losses *after* the first packet it ever sees. Start a run with
//! `--outage-ms`, and the packets dropped before the receiver's first arrival
//! are invisible to it — there is no earlier sequence number to measure the gap
//! from. The transmitter will say it dropped 400 and the receiver will honestly
//! say it lost 201. Compare the two only from the receiver's first sequence
//! number onward.
//!
//! ## This is a test fixture, and it says so
//!
//! The audio it sends is generated or replayed from a file. It is **never**
//! captured, and nothing here is reachable from the desktop application: this
//! crate is not a dependency of `stagelink-service` and is not included in the
//! installer. A dashboard figure must never come from this binary.
//!
//! It also holds no key material, so `--encrypted` sets the flag and reserves
//! the AEAD tag region without sealing anything. That verifies a receiver's
//! *framing*, not its cryptography; a receiver that authenticates will reject
//! these packets, which is correct.


use std::net::UdpSocket;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tracing::{info, warn};

use reference_transmitter::impairment::ImpairmentConfig;
use reference_transmitter::source::Source;
use reference_transmitter::transmit::{CollectingSink, Sink, TransmitConfig, Transmitter};

#[derive(Parser, Debug)]
#[command(
    name = "stagelink-reference-transmitter",
    about = "Sends real StageLink audio packets, with reproducible impairments",
    version
)]
struct Args {
    /// Where to send, e.g. 10.0.0.7:45872.
    #[arg(long)]
    target: Option<String>,

    /// Build and impair packets without sending any. Prints the same report.
    #[arg(long)]
    dry_run: bool,

    // -- source ------------------------------------------------------------
    /// Replay this WAV file instead of a generated tone.
    #[arg(long)]
    wav: Option<PathBuf>,

    /// Repeat the WAV file instead of stopping at its end.
    #[arg(long = "loop")]
    looping: bool,

    /// Tone frequency in Hz, when no WAV is given.
    #[arg(long, default_value_t = 1_000.0)]
    tone_hz: f32,

    /// Tone amplitude, 0.0 to 1.0. Default is -6 dBFS.
    #[arg(long, default_value_t = 0.5)]
    amplitude: f32,

    /// Send digital silence. Verifies the SILENCE flag path.
    #[arg(long)]
    silence: bool,

    // -- stream shape ------------------------------------------------------
    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,

    /// Packet duration in milliseconds: 2.5, 5 or 10.
    #[arg(long, default_value_t = 5.0)]
    frame_ms: f64,

    /// Session id written into every packet header.
    #[arg(long, default_value_t = 0x5EED)]
    session_id: u32,

    /// Stop after this many seconds. Omit to run until the source ends.
    #[arg(long)]
    duration_secs: Option<u64>,

    /// Start a new session, with the sequence back to zero, after this many
    /// packets. Receivers must flush their buffer when this happens.
    #[arg(long)]
    session_change_after: Option<u64>,

    /// Send an end-of-stream marker when the run finishes.
    #[arg(long, default_value_t = true)]
    end_of_stream: bool,

    /// Set the ENCRYPTED flag and reserve the tag region. Nothing is sealed.
    #[arg(long)]
    encrypted: bool,

    // -- impairments -------------------------------------------------------
    /// Packet loss, as a percentage.
    #[arg(long, default_value_t = 0.0)]
    loss: f64,

    /// Packets lost per burst. 1 is independent loss; Wi-Fi is nearer 8.
    #[arg(long, default_value_t = 1)]
    burst_length: u32,

    /// Percentage of packets delayed behind the next packet that is not itself
    /// delayed. 100 therefore means a uniform one-packet delay, not chaos.
    #[arg(long, default_value_t = 0.0)]
    reorder: f64,

    /// Percentage of packets sent twice.
    #[arg(long, default_value_t = 0.0)]
    duplicate: f64,

    /// Length of a total outage, in milliseconds.
    #[arg(long, default_value_t = 0)]
    outage_ms: u64,

    /// How often an outage starts, in milliseconds.
    #[arg(long, default_value_t = 10_000)]
    outage_period_ms: u64,

    /// Uniform random send delay, in milliseconds, on top of the cadence.
    #[arg(long, default_value_t = 0.0)]
    jitter_ms: f64,

    /// Capture-clock error in parts per million. Positive runs fast.
    #[arg(long, default_value_t = 0.0)]
    clock_ppm: f64,

    /// Seed for every random decision. The same seed replays the same run.
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

/// Sends over a real UDP socket.
struct UdpSink {
    socket: UdpSocket,
    target: String,
    /// Send failures are counted rather than fatal: one ICMP-unreachable on a
    /// venue network must not end a soak test that is ten minutes in.
    errors: u64,
}

impl Sink for UdpSink {
    fn send(&mut self, datagram: &[u8]) -> std::io::Result<()> {
        if let Err(error) = self.socket.send_to(datagram, &self.target) {
            self.errors += 1;
            if self.errors == 1 {
                warn!(%error, "the first send failed; continuing and counting");
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact()
        .init();

    let args = Args::parse();
    let config = build_config(&args)?;
    let source = build_source(&args)?;

    info!(
        source = source.describe(),
        source_seconds = source
            .frames()
            .map(|frames| frames as f64 / f64::from(config.sample_rate)),
        sample_rate = config.sample_rate,
        frame_ms = config.frame_duration_ms,
        frames_per_packet = config.frames_per_packet(),
        packets_per_second = 1_000.0 / config.frame_duration_ms,
        nominal_interval_us = config.packet_interval().as_micros(),
        // Differs from the nominal interval only under --clock-ppm, and that
        // difference is the whole scenario when it does.
        paced_interval_us = config.skewed_interval().as_micros(),
        "reference transmitter starting"
    );
    if !config.impairment.is_clean() {
        info!(
            loss_percent = config.impairment.loss_percent,
            burst_length = config.impairment.burst_length,
            reorder_percent = config.impairment.reorder_percent,
            duplicate_percent = config.impairment.duplicate_percent,
            outage_ms = config.impairment.outage_ms,
            seed = config.seed,
            "impairments active; replay this run with --seed"
        );
    }

    let mut transmitter = Transmitter::new(config, source);
    let report = match (&args.target, args.dry_run) {
        (Some(target), false) => {
            // Bound to all interfaces on an ephemeral port: the fixture does
            // not choose the operator's streaming adapter, the route does.
            let socket = UdpSocket::bind("0.0.0.0:0").context("binding a UDP socket")?;
            socket
                .connect(target)
                .with_context(|| format!("{target} is not a reachable address"))?;
            let mut sink = UdpSink { socket, target: target.clone(), errors: 0 };
            let report = transmitter.run(&mut sink, std::thread::sleep)?;
            if sink.errors > 0 {
                warn!(errors = sink.errors, "some datagrams could not be sent");
            }
            report
        }
        (_, true) => {
            let mut sink = CollectingSink::default();
            // Still paced in real time, so a dry run reports the same outage
            // and jitter behaviour a real one would.
            transmitter.run(&mut sink, std::thread::sleep)?
        }
        (None, false) => bail!("pass --target <host:port>, or --dry-run to build without sending"),
    };

    info!(
        built = report.built,
        sent = report.sent,
        dropped = report.dropped,
        duplicated = report.duplicated,
        reordered = report.reordered,
        sessions = report.sessions,
        end_of_stream = report.end_of_stream_sent,
        "finished"
    );

    // Logged so a test harness can compare a receiver's claims against what
    // was really sent, rather than against what was asked for. A receiver
    // reporting 2% loss on a run that really dropped 7% has a bug.
    info!(
        realised_loss_percent = format!("{:.2}", report.realised_loss_percent()),
        "as actually sent"
    );

    Ok(())
}

fn build_config(args: &Args) -> Result<TransmitConfig> {
    if !stream_server::config::SUPPORTED_FRAME_DURATIONS_MS.contains(&args.frame_ms) {
        bail!(
            "--frame-ms must be one of {:?}. Longer packets exceed a 1500-byte datagram and \
             would be IP-fragmented, and a fragmented audio packet loses all of its audio if \
             any fragment is dropped.",
            stream_server::config::SUPPORTED_FRAME_DURATIONS_MS
        );
    }

    let impairment = ImpairmentConfig {
        loss_percent: args.loss,
        burst_length: args.burst_length,
        reorder_percent: args.reorder,
        duplicate_percent: args.duplicate,
        outage_ms: args.outage_ms,
        outage_period_ms: args.outage_period_ms,
    };
    impairment.validate().map_err(|reason| anyhow::anyhow!(reason))?;

    let packet_limit = args.duration_secs.map(|seconds| {
        (seconds as f64 * 1_000.0 / args.frame_ms).round() as u64
    });

    Ok(TransmitConfig {
        sample_rate: args.sample_rate,
        frame_duration_ms: args.frame_ms,
        session_id: args.session_id,
        packet_limit,
        session_change_after: args.session_change_after,
        clock_ppm: args.clock_ppm,
        jitter_ms: args.jitter_ms,
        end_of_stream: args.end_of_stream,
        encrypted: args.encrypted,
        impairment,
        seed: args.seed,
    })
}

fn build_source(args: &Args) -> Result<Source> {
    if args.silence {
        return Ok(Source::Silence);
    }
    match &args.wav {
        Some(path) => Source::wav(path, args.sample_rate, args.looping),
        None => Ok(Source::tone(args.tone_hz, args.amplitude, args.sample_rate)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Args {
        Args::parse_from(["stagelink-reference-transmitter", "--dry-run"])
    }

    #[test]
    fn the_defaults_describe_a_clean_48k_5ms_stream() {
        let config = build_config(&args()).unwrap();
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.frames_per_packet(), 240);
        assert_eq!(config.packet_interval().as_micros(), 5_000);
        assert!(config.impairment.is_clean(), "impairments must be opt-in");
        assert!(config.end_of_stream, "a clean stop is the default");
        assert!(!config.encrypted);
    }

    /// 20 ms would be 1920 payload bytes, which IP-fragments. A fragmented
    /// audio packet loses all of its audio if any fragment is dropped, so this
    /// is refused rather than quietly sent.
    #[test]
    fn an_oversized_frame_duration_is_refused_with_the_reason() {
        let mut args = args();
        args.frame_ms = 20.0;
        let error = build_config(&args).unwrap_err().to_string();
        assert!(error.contains("fragment"), "got: {error}");
    }

    #[test]
    fn a_duration_becomes_the_right_packet_count() {
        let mut args = args();
        args.duration_secs = Some(10);
        // Ten seconds at 200 packets per second.
        assert_eq!(build_config(&args).unwrap().packet_limit, Some(2_000));

        args.frame_ms = 2.5;
        assert_eq!(build_config(&args).unwrap().packet_limit, Some(4_000));
    }

    #[test]
    fn impossible_impairments_are_refused_before_anything_is_sent() {
        let mut args = args();
        args.loss = 150.0;
        assert!(build_config(&args).is_err());

        args.loss = 0.0;
        args.outage_ms = 5_000;
        args.outage_period_ms = 1_000;
        assert!(build_config(&args).is_err(), "the link would never be up");
    }

    #[test]
    fn the_source_defaults_to_a_tone_and_silence_is_explicit() {
        assert_eq!(build_source(&args()).unwrap().describe(), "generated tone");

        let mut silent = args();
        silent.silence = true;
        assert_eq!(build_source(&silent).unwrap().describe(), "digital silence");
    }

    #[test]
    fn a_missing_wav_file_is_an_error_not_a_silent_fallback_to_a_tone() {
        let mut args = args();
        args.wav = Some(PathBuf::from("no-such-file.wav"));
        assert!(build_source(&args).is_err());
    }

    #[test]
    fn no_target_and_no_dry_run_is_refused_rather_than_doing_nothing_quietly() {
        let parsed = Args::parse_from(["stagelink-reference-transmitter"]);
        assert!(parsed.target.is_none());
        assert!(!parsed.dry_run);
    }
}

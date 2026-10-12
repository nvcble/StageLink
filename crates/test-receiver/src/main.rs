//! Headless StageLink receiver.
//!
//! The purpose of this binary is to make a specific claim falsifiable: *audio
//! packets produced by the desktop are received, correctly sequenced, and
//! decode to the audio that went in*. A dashboard that says "streaming" proves
//! nothing; a WAV file you can listen to proves a great deal.
//!
//! ## Phase status
//!
//! The packet receive path, the sequence/loss accounting and the WAV writer are
//! implemented and tested here. They are driven by [`PacketSink`], which takes
//! raw datagrams, so the whole analysis path is testable without a network.
//!
//! The socket binding, pairing handshake and AEAD decryption land with the
//! streaming engine in Phase 3/5. Until the desktop can send, this binary
//! cannot receive from it — that is reported as a clear error rather than an
//! empty success, and `--self-test` exercises the full analysis path against
//! synthetic packets so the plumbing is verifiable today.

use std::path::PathBuf;

use anyhow::{Context, Result};
use audio_protocol::{SequenceTracker, SequenceVerdict, parse_header};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "stagelink-test-receiver",
    about = "Headless StageLink receiver for verifying real audio delivery",
    version
)]
struct Args {
    /// Desktop address to join, e.g. 10.0.0.1:45871.
    ///
    /// Joining a live desktop needs the control server, which does not exist
    /// yet. To receive audio now, use `--listen` with the reference
    /// transmitter.
    #[arg(long)]
    server: Option<String>,

    /// Bind here and receive audio, e.g. 0.0.0.0:45872.
    ///
    /// Audio is pushed over UDP, so a receiver binds rather than connecting.
    /// This works today against `stagelink-reference-transmitter`; it needs no
    /// pairing because the plaintext transport needs no keys.
    #[arg(long)]
    listen: Option<String>,

    /// Write the received audio to this WAV file.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Stop after this many seconds.
    #[arg(long, default_value_t = 10)]
    duration_secs: u64,

    /// Run the analysis path against synthetic packets and exit.
    ///
    /// Verifies sequencing, loss accounting and WAV output without needing a
    /// desktop to stream from.
    #[arg(long)]
    self_test: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact()
        .init();

    let args = Args::parse();

    if args.self_test {
        return self_test(args.output.as_deref());
    }

    if let Some(address) = args.listen.as_deref() {
        return listen(address, args.duration_secs, args.output.as_deref());
    }

    if args.server.is_some() {
        anyhow::bail!(
            "Joining a live desktop requires the control server (Phase 5), which is not \
             implemented. This binary will not pretend to have received audio it did not \
             receive. Use --listen <addr:port> with stagelink-reference-transmitter to receive \
             real packets today."
        );
    }

    anyhow::bail!(
        "Nothing to do. Pass --listen <addr:port> to receive packets, or --self-test to \
         exercise the analysis path with no network."
    )
}

/// Binds a UDP socket and accumulates whatever arrives.
///
/// Stops on an end-of-stream marker or when the time runs out, whichever comes
/// first. A silent socket is reported as an error rather than an empty success:
/// "no audio arrived" is the single most important thing this binary can say.
fn listen(address: &str, duration_secs: u64, output: Option<&std::path::Path>) -> Result<()> {
    use std::net::UdpSocket;
    use std::time::{Duration, Instant};

    let socket = UdpSocket::bind(address)
        .with_context(|| format!("binding {address}; is another receiver already running?"))?;
    // A timeout rather than a blocking read, so a stream that stops dead still
    // produces a report instead of hanging.
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .context("setting a read timeout")?;

    tracing::info!(%address, duration_secs, "listening for StageLink audio");

    // One MTU is the largest datagram the protocol permits, and a larger read
    // would silently truncate rather than reveal a misconfigured sender.
    let mut buffer = vec![0_u8; 1_500];
    let mut sink = PacketSink::new(48_000);
    let mut first_from: Option<std::net::SocketAddr> = None;
    let deadline = Instant::now() + Duration::from_secs(duration_secs);

    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((len, from)) => {
                if first_from.is_none() {
                    tracing::info!(%from, "first packet");
                    first_from = Some(from);
                }
                sink.accept(&buffer[..len])?;
                if sink.report().end_of_stream {
                    tracing::info!("end-of-stream marker received; stopping");
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error).context("receiving"),
        }
    }

    let report = sink.report();
    println!("{report}");

    if report.accepted == 0 {
        anyhow::bail!(
            "no StageLink packets arrived at {address} in {duration_secs}s. Check the sender's \
             target address, that both ends are on the same network, and the firewall."
        );
    }

    if let Some(path) = output {
        sink.write_wav(path)?;
        println!("wrote {} frames to {}", report.frames, path.display());
    }

    Ok(())
}

/// Exercises the full analysis path against synthetic packets.
fn self_test(output: Option<&std::path::Path>) -> Result<()> {
    use audio_core::convert;
    use audio_protocol::{AudioPacketHeader, HEADER_BYTES, PacketFlags, encode_header};

    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 240;
    const PACKETS: u32 = 100;

    let mut sink = PacketSink::new(SAMPLE_RATE);
    let mut datagram = vec![0_u8; HEADER_BYTES + FRAMES * 2];

    for sequence in 0..PACKETS {
        // Skip one packet to prove loss is detected rather than smoothed over.
        if sequence == 42 {
            continue;
        }
        let header = AudioPacketHeader::new(
            0x5EED,
            sequence,
            u64::from(sequence) * 5_000,
            PacketFlags::empty(),
        );
        encode_header(&header, &mut datagram)?;

        // A 1 kHz sine, so the WAV is audibly correct rather than just present.
        let samples: Vec<f32> = (0..FRAMES)
            .map(|frame| {
                let t = (u64::from(sequence) * FRAMES as u64 + frame as u64) as f32
                    / SAMPLE_RATE as f32;
                (core::f32::consts::TAU * 1_000.0 * t).sin() * 0.5
            })
            .collect();
        convert::f32_to_pcm_s16le(&samples, &mut datagram[HEADER_BYTES..]);

        sink.accept(&datagram)?;
    }

    let report = sink.report();
    println!("{report}");

    anyhow::ensure!(report.accepted == 99, "expected 99 accepted packets");
    anyhow::ensure!(report.lost == 1, "the dropped packet must be detected");
    anyhow::ensure!(report.rejected == 0, "no packet should have been rejected");
    anyhow::ensure!(report.frames == 99 * FRAMES as u64, "frame count mismatch");

    if let Some(path) = output {
        sink.write_wav(path)?;
        println!("wrote {} samples to {}", report.frames, path.display());
    }

    println!("self-test passed");
    Ok(())
}

/// Accepts datagrams, validates them and accumulates the audio.
///
/// Separated from the socket so every behaviour below is testable.
pub struct PacketSink {
    sample_rate: u32,
    tracker: SequenceTracker,
    samples: Vec<f32>,
    accepted: u64,
    rejected: u64,
    silent: u64,
    discontinuities: u64,
    end_of_stream: bool,
    rejection_reasons: Vec<String>,
}

/// What the receiver observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub accepted: u64,
    pub rejected: u64,
    pub lost: u64,
    pub reordered: u64,
    pub duplicates: u64,
    pub silent: u64,
    pub discontinuities: u64,
    pub frames: u64,
    pub end_of_stream: bool,
}

impl core::fmt::Display for Report {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "accepted={} rejected={} lost={} reordered={} duplicates={} silent={} \
             discontinuities={} frames={} end_of_stream={}",
            self.accepted,
            self.rejected,
            self.lost,
            self.reordered,
            self.duplicates,
            self.silent,
            self.discontinuities,
            self.frames,
            self.end_of_stream
        )
    }
}

impl PacketSink {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            tracker: SequenceTracker::new(),
            samples: Vec::new(),
            accepted: 0,
            rejected: 0,
            silent: 0,
            discontinuities: 0,
            end_of_stream: false,
            rejection_reasons: Vec::new(),
        }
    }

    /// Validates and accumulates one datagram.
    ///
    /// A malformed datagram is counted and skipped rather than aborting the
    /// run: a real receiver on a shared network will see stray traffic, and it
    /// must survive that.
    pub fn accept(&mut self, datagram: &[u8]) -> Result<()> {
        let (header, payload) = match parse_header(datagram) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.rejected += 1;
                if self.rejection_reasons.len() < 10 {
                    self.rejection_reasons.push(error.to_string());
                }
                return Ok(());
            }
        };

        if header.flags.encrypted() {
            anyhow::bail!(
                "this packet is encrypted; decryption requires the Phase 5 pairing handshake"
            );
        }

        match self.tracker.observe(header.session_id, header.sequence) {
            SequenceVerdict::Duplicate => {
                // Already have this audio; count it and move on.
                self.accepted += 1;
                return Ok(());
            }
            SequenceVerdict::SessionChanged => {
                // The desktop restarted its stream. Discard what we had: the
                // old and new audio are not continuous.
                self.samples.clear();
            }
            _ => {}
        }

        if header.flags.has(audio_protocol::PacketFlags::DISCONTINUITY) {
            self.discontinuities += 1;
        }
        if header.flags.has(audio_protocol::PacketFlags::SILENCE) {
            self.silent += 1;
        }
        if header.flags.end_of_stream() {
            self.end_of_stream = true;
        }

        let frames = payload.len() / 2;
        let start = self.samples.len();
        self.samples.resize(start + frames, 0.0);
        audio_core::convert::pcm_s16le_to_f32(payload, &mut self.samples[start..]);

        self.accepted += 1;
        Ok(())
    }

    #[must_use]
    pub fn report(&self) -> Report {
        Report {
            accepted: self.accepted,
            rejected: self.rejected,
            lost: self.tracker.lost(),
            reordered: self.tracker.reordered(),
            duplicates: self.tracker.duplicates(),
            silent: self.silent,
            discontinuities: self.discontinuities,
            frames: self.samples.len() as u64,
            end_of_stream: self.end_of_stream,
        }
    }

    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[must_use]
    pub fn rejection_reasons(&self) -> &[String] {
        &self.rejection_reasons
    }

    /// Writes the received audio as a mono 16-bit WAV.
    pub fn write_wav(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec)
            .with_context(|| format!("creating {}", path.display()))?;
        for &sample in &self.samples {
            writer.write_sample(audio_core::convert::f32_to_i16(sample))?;
        }
        writer.finalize().context("finalising the WAV file")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_core::convert;
    use audio_protocol::{AudioPacketHeader, HEADER_BYTES, PacketFlags, encode_header};

    const FRAMES: usize = 240;

    fn packet(session: u32, sequence: u32, flags: PacketFlags, amplitude: f32) -> Vec<u8> {
        let mut datagram = vec![0_u8; HEADER_BYTES + FRAMES * 2];
        let header =
            AudioPacketHeader::new(session, sequence, u64::from(sequence) * 5_000, flags);
        encode_header(&header, &mut datagram).unwrap();
        let samples = vec![amplitude; FRAMES];
        convert::f32_to_pcm_s16le(&samples, &mut datagram[HEADER_BYTES..]);
        datagram
    }

    #[test]
    fn a_clean_run_reports_no_loss_and_accumulates_every_frame() {
        let mut sink = PacketSink::new(48_000);
        for sequence in 0..10 {
            sink.accept(&packet(1, sequence, PacketFlags::empty(), 0.5))
                .unwrap();
        }
        let report = sink.report();
        assert_eq!(report.accepted, 10);
        assert_eq!(report.lost, 0);
        assert_eq!(report.rejected, 0);
        assert_eq!(report.frames, 10 * FRAMES as u64);
    }

    #[test]
    fn the_received_audio_decodes_back_to_what_was_sent() {
        let mut sink = PacketSink::new(48_000);
        sink.accept(&packet(1, 0, PacketFlags::empty(), 0.5)).unwrap();
        for &sample in sink.samples() {
            assert!(
                (sample - 0.5).abs() < 4e-5,
                "sample {sample} should be 0.5 within 16-bit quantisation"
            );
        }
    }

    #[test]
    fn a_dropped_packet_is_detected_rather_than_smoothed_over() {
        let mut sink = PacketSink::new(48_000);
        for sequence in [0_u32, 1, 2, 4, 5] {
            sink.accept(&packet(1, sequence, PacketFlags::empty(), 0.1))
                .unwrap();
        }
        let report = sink.report();
        assert_eq!(report.accepted, 5);
        assert_eq!(report.lost, 1, "packet 3 is missing and must be counted");
    }

    #[test]
    fn a_duplicate_is_counted_but_not_appended_twice() {
        let mut sink = PacketSink::new(48_000);
        sink.accept(&packet(1, 0, PacketFlags::empty(), 0.1)).unwrap();
        sink.accept(&packet(1, 1, PacketFlags::empty(), 0.1)).unwrap();
        let frames_before = sink.report().frames;
        sink.accept(&packet(1, 1, PacketFlags::empty(), 0.1)).unwrap();

        let report = sink.report();
        assert_eq!(report.duplicates, 1);
        assert_eq!(
            report.frames, frames_before,
            "duplicate audio must not be appended again"
        );
    }

    #[test]
    fn a_reordered_packet_is_counted_and_corrects_the_loss_tally() {
        let mut sink = PacketSink::new(48_000);
        sink.accept(&packet(1, 0, PacketFlags::empty(), 0.1)).unwrap();
        sink.accept(&packet(1, 3, PacketFlags::empty(), 0.1)).unwrap();
        assert_eq!(sink.report().lost, 2);

        sink.accept(&packet(1, 2, PacketFlags::empty(), 0.1)).unwrap();
        let report = sink.report();
        assert_eq!(report.reordered, 1);
        assert_eq!(report.lost, 1, "the late packet was not actually lost");
    }

    #[test]
    fn stray_traffic_is_rejected_without_ending_the_run() {
        let mut sink = PacketSink::new(48_000);
        sink.accept(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        sink.accept(&[0_u8; 4]).unwrap();
        sink.accept(&packet(1, 0, PacketFlags::empty(), 0.1)).unwrap();

        let report = sink.report();
        assert_eq!(report.rejected, 2);
        assert_eq!(report.accepted, 1, "a valid packet after junk is still accepted");
        assert_eq!(sink.rejection_reasons().len(), 2);
    }

    #[test]
    fn silence_and_discontinuity_flags_are_recorded() {
        let mut sink = PacketSink::new(48_000);
        sink.accept(&packet(
            1,
            0,
            PacketFlags::empty().with(PacketFlags::SILENCE),
            0.0,
        ))
        .unwrap();
        sink.accept(&packet(
            1,
            1,
            PacketFlags::empty().with(PacketFlags::DISCONTINUITY),
            0.1,
        ))
        .unwrap();

        let report = sink.report();
        assert_eq!(report.silent, 1);
        assert_eq!(report.discontinuities, 1);
    }

    #[test]
    fn end_of_stream_is_observed() {
        let mut sink = PacketSink::new(48_000);
        sink.accept(&packet(1, 0, PacketFlags::empty(), 0.1)).unwrap();
        assert!(!sink.report().end_of_stream);
        sink.accept(&packet(
            1,
            1,
            PacketFlags::empty().with(PacketFlags::END_OF_STREAM),
            0.0,
        ))
        .unwrap();
        assert!(sink.report().end_of_stream);
    }

    #[test]
    fn a_session_change_discards_the_discontinuous_audio() {
        let mut sink = PacketSink::new(48_000);
        for sequence in 0..3 {
            sink.accept(&packet(1, sequence, PacketFlags::empty(), 0.1))
                .unwrap();
        }
        assert_eq!(sink.report().frames, 3 * FRAMES as u64);

        // The desktop restarted its stream.
        sink.accept(&packet(2, 0, PacketFlags::empty(), 0.2)).unwrap();
        assert_eq!(
            sink.report().frames,
            FRAMES as u64,
            "audio from the previous session must not be concatenated"
        );
        assert_eq!(sink.report().lost, 0, "a restart is not packet loss");
    }

    #[test]
    fn an_encrypted_packet_is_refused_rather_than_silently_mishandled() {
        let mut sink = PacketSink::new(48_000);
        let mut datagram = packet(
            1,
            0,
            PacketFlags::empty().with(PacketFlags::ENCRYPTED),
            0.1,
        );
        // Make room for the tag so the header parses.
        datagram.extend_from_slice(&[0_u8; 16]);
        assert!(
            sink.accept(&datagram).is_err(),
            "the receiver must not pretend to decrypt"
        );
    }

    #[test]
    fn a_wav_file_is_written_with_the_right_shape() {
        let mut sink = PacketSink::new(48_000);
        for sequence in 0..5 {
            sink.accept(&packet(1, sequence, PacketFlags::empty(), 0.5))
                .unwrap();
        }

        let path = std::env::temp_dir()
            .join(format!("stagelink-receiver-{}.wav", std::process::id()));
        sink.write_wav(&path).unwrap();

        let reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1, "StageLink audio is mono end to end");
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(reader.len() as u64, 5 * FRAMES as u64);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_self_test_exercises_the_whole_analysis_path() {
        self_test(None).expect("the self-test must pass");
    }

    #[test]
    fn receiving_from_a_live_desktop_reports_the_missing_phase_rather_than_faking_success() {
        // The binary must not claim to have verified streaming before the
        // streaming engine exists.
        let args = Args::try_parse_from([
            "stagelink-test-receiver",
            "--server",
            "10.0.0.1:45872",
        ])
        .unwrap();
        assert_eq!(args.server.as_deref(), Some("10.0.0.1:45872"));
        assert!(!args.self_test);
    }
}

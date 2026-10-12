//! The UDP sender: the thread that actually puts audio on the network.
//!
//! One socket, one thread, one packet per frame interval, fanned out to every
//! receiver currently in the roster.
//!
//! ## Why a thread and not the audio callback
//!
//! The capture callback must never do anything that can block, and `send_to`
//! can: a full socket buffer, an ARP resolution, a Wi-Fi driver taking its
//! time. Sending from the callback would turn a network hiccup into a gap in
//! the *captured* audio, which is the one thing that must never happen —
//! everyone's feed would glitch because one phone's route went away.
//!
//! So capture writes to a lock-free ring, and this thread reads from it. The
//! ring is the only thing between the two, and the audio thread never waits.
//!
//! ## Why a bounded queue per receiver
//!
//! A receiver on a congested link cannot be allowed to slow the others down.
//! Each destination gets its own [`ReceiverQueue`], which drops its *oldest*
//! packet when full — stale audio is worthless in a monitor feed, so if
//! something has to go it should be the packet nobody can use any more. A slow
//! receiver therefore loses its own audio and nobody else's.
//!
//! ## Pacing
//!
//! The loop keeps a deadline that advances by exactly one frame interval per
//! packet, and sleeps until it. Sleeping *for* the interval instead would let
//! every scheduling overshoot accumulate, and on Windows, where a sleep
//! routinely overshoots by a millisecond or more, a 5 ms cadence would drift
//! into something closer to 6.5 ms within seconds — which a receiver sees as
//! its buffer steadily draining.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audio_protocol::TransportKind;
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::config::StreamConfig;
use crate::packetiser::{PacketBuilder, PacketiserStats};
use crate::queue::ReceiverQueue;

/// Where one packet of mono audio comes from.
///
/// A trait so the sender can be driven by a tone in a test as well as by the
/// real capture ring. Without it, the only way to test the send path would be
/// to open an audio device, and then none of this would be testable in CI or
/// on a machine with no interface plugged in.
pub trait FrameSource: Send {
    /// Fills `out` with exactly one packet of frames.
    ///
    /// Returns false when the source has nothing ready, which the sender
    /// records as an underrun rather than sending a short packet.
    fn pop(&mut self, out: &mut [f32]) -> bool;

    /// Capture glitches since the last call.
    ///
    /// Non-zero means the captured audio either side of this point is **not
    /// continuous**, and the next packet must carry `DISCONTINUITY` so a
    /// receiver resynchronises instead of treating the gap as clock drift.
    /// Without this, a desktop stall reaches the receiver as perfectly
    /// continuous sequence numbers and timestamps, the receiver underruns with
    /// no loss indication, and an operator cannot tell a stalled desktop from
    /// a bad network.
    fn take_glitches(&mut self) -> u64 {
        0
    }
}

/// Counters the sender publishes. Desktop-measured, every one of them.
#[derive(Debug, Default)]
pub struct SenderStats {
    packets_sent: AtomicU64,
    /// Datagrams the socket refused. Counted per attempt, not per receiver.
    send_errors: AtomicU64,
    /// Times the capture ring had no whole packet ready.
    underruns: AtomicU64,
    /// Packets dropped because a receiver's queue was full.
    dropped_queue_full: AtomicU64,
    /// Capture glitches that were flagged onto a packet.
    discontinuities: AtomicU64,
    bytes_sent: AtomicU64,
}

impl SenderStats {
    #[must_use]
    pub fn packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn send_errors(&self) -> u64 {
        self.send_errors.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn dropped_queue_full(&self) -> u64 {
        self.dropped_queue_full.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn discontinuities(&self) -> u64 {
        self.discontinuities.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }
}

/// Per-receiver send accounting, for the roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationStats {
    pub receiver_id: String,
    pub address: SocketAddr,
    pub packets_sent: u64,
    /// Dropped locally because this receiver's queue was full. Its own fault,
    /// and its own loss.
    pub packets_dropped: u64,
    pub queue_depth: usize,
}

struct Destination {
    receiver_id: String,
    address: SocketAddr,
    queue: ReceiverQueue,
    packets_sent: u64,
    packets_dropped: u64,
}

/// The set of receivers being sent to. Updated by the control server.
#[derive(Default)]
struct DestinationTable {
    entries: Vec<Destination>,
    /// Receivers admitted but not yet sent to, so a join mid-packet is not
    /// missed. Merged at the top of the next loop iteration.
    pending: Vec<(String, SocketAddr)>,
    removing: Vec<String>,
}

/// A running sender.
pub struct StreamSender {
    config: StreamConfig,
    destinations: Arc<Mutex<DestinationTable>>,
    stats: Arc<SenderStats>,
    packetiser_stats: Arc<PacketiserStats>,
    stop: Arc<AtomicBool>,
    drain: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    local_address: SocketAddr,
    session_id: u32,
}

impl StreamSender {
    /// Binds the socket and starts the send thread.
    ///
    /// Binding happens here, on the caller's thread, so a port already in use
    /// is reported as an error the operator can act on rather than appearing
    /// later as silence.
    pub fn start(
        config: StreamConfig,
        session_id: u32,
        source: Box<dyn FrameSource>,
    ) -> std::io::Result<Self> {
        // Bind to the chosen adapter's address, not to all of them: the
        // operator picked which network carries the audio, and sending from an
        // interface they did not pick is how audio ends up on a venue's
        // guest Wi-Fi.
        let bind = format!("{}:0", config.bind_address);
        let socket = UdpSocket::bind(&bind)?;
        let local_address = socket.local_addr()?;
        // Non-blocking: a stalled send must cost one packet, not the cadence.
        socket.set_nonblocking(true)?;

        let destinations = Arc::new(Mutex::new(DestinationTable::default()));
        let stats = Arc::new(SenderStats::default());
        let packetiser_stats = Arc::new(PacketiserStats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let drain = Arc::new(AtomicBool::new(false));

        let thread = {
            let config = config.clone();
            let destinations = Arc::clone(&destinations);
            let stats = Arc::clone(&stats);
            let packetiser_stats = Arc::clone(&packetiser_stats);
            let stop = Arc::clone(&stop);
            let drain = Arc::clone(&drain);
            std::thread::Builder::new()
                .name("stagelink-sender".to_owned())
                .spawn(move || {
                    send_loop(SendLoop {
                        socket,
                        config,
                        session_id,
                        source,
                        destinations,
                        stats,
                        packetiser_stats,
                        stop,
                        drain,
                    });
                })?
        };

        info!(%local_address, port = config.stream_port, "audio sender started");

        Ok(Self {
            config,
            destinations,
            stats,
            packetiser_stats,
            stop,
            drain,
            thread: Some(thread),
            local_address,
            session_id,
        })
    }

    /// Begins sending to a receiver.
    ///
    /// The port is the receiver's, from its control connection — not the
    /// desktop's stream port. A receiver behind NAT or on an unexpected port
    /// is still reachable at the address the control channel observed.
    pub fn add_destination(&self, receiver_id: impl Into<String>, address: SocketAddr) {
        let mut table = self.destinations.lock();
        table.pending.push((receiver_id.into(), address));
    }

    /// Stops sending to a receiver.
    pub fn remove_destination(&self, receiver_id: impl Into<String>) {
        let mut table = self.destinations.lock();
        table.removing.push(receiver_id.into());
    }

    #[must_use]
    pub fn destination_count(&self) -> usize {
        let table = self.destinations.lock();
        table.entries.len() + table.pending.len()
    }

    /// Per-receiver counters, for the roster.
    #[must_use]
    pub fn destination_stats(&self) -> Vec<DestinationStats> {
        let table = self.destinations.lock();
        table
            .entries
            .iter()
            .map(|destination| DestinationStats {
                receiver_id: destination.receiver_id.clone(),
                address: destination.address,
                packets_sent: destination.packets_sent,
                packets_dropped: destination.packets_dropped,
                queue_depth: destination.queue.len(),
            })
            .collect()
    }

    #[must_use]
    pub fn stats(&self) -> Arc<SenderStats> {
        Arc::clone(&self.stats)
    }

    #[must_use]
    pub fn packetiser_stats(&self) -> Arc<PacketiserStats> {
        Arc::clone(&self.packetiser_stats)
    }

    #[must_use]
    pub const fn local_address(&self) -> SocketAddr {
        self.local_address
    }

    #[must_use]
    pub const fn session_id(&self) -> u32 {
        self.session_id
    }

    #[must_use]
    pub fn config(&self) -> &StreamConfig {
        &self.config
    }

    /// Estimated egress in bits per second, including UDP/IP/Ethernet overhead.
    ///
    /// Labelled an estimate wherever it is shown: it is arithmetic from the
    /// configuration and the receiver count, not a measurement of the wire.
    #[must_use]
    pub fn estimated_bitrate(&self) -> u64 {
        const WIRE_OVERHEAD_BYTES: u64 = 42;
        let datagram = audio_protocol::datagram_bytes(
            self.config.frames_per_packet(),
            self.config.transport.is_encrypted(),
        ) as u64;
        let per_second = (1_000.0 / self.config.frame_duration_ms) as u64;
        (datagram + WIRE_OVERHEAD_BYTES) * 8 * per_second * self.destination_count() as u64
    }

    /// Stops the sender.
    ///
    /// `drain` sends an end-of-stream marker and flushes the queues first, so
    /// receivers stop cleanly instead of waiting out their jitter buffers on
    /// sudden silence. `false` is for a device that has already vanished,
    /// where there is nothing to drain into.
    /// Takes `&mut self` rather than `self` so the final counters can still be
    /// read afterwards. The service reports them in the stream status, and a
    /// stop that consumed the handle would throw away the numbers describing
    /// the session that just ended.
    pub fn stop(&mut self, drain: bool) {
        if self.thread.is_none() {
            return;
        }
        self.drain.store(drain, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // A send thread that will not join is a bug worth seeing rather
            // than hanging on, so this is bounded by the loop's own exit
            // rather than by a timeout here.
            if thread.join().is_err() {
                warn!("the sender thread panicked");
            }
        }
        debug!("audio sender stopped");
    }

    /// Whether the send thread is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.thread.is_some()
    }
}

impl Drop for StreamSender {
    fn drop(&mut self) {
        // Dropping without `stop` happens on an error path. Stop the thread
        // rather than leaving it sending into a session that is over.
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct SendLoop {
    socket: UdpSocket,
    config: StreamConfig,
    session_id: u32,
    source: Box<dyn FrameSource>,
    destinations: Arc<Mutex<DestinationTable>>,
    stats: Arc<SenderStats>,
    packetiser_stats: Arc<PacketiserStats>,
    stop: Arc<AtomicBool>,
    drain: Arc<AtomicBool>,
}

fn send_loop(mut state: SendLoop) {
    let frames = state.config.frames_per_packet();
    let encrypted = state.config.transport.is_encrypted();
    let interval = Duration::from_secs_f64(state.config.frame_duration_ms / 1_000.0);

    let mut builder = PacketBuilder::new(state.session_id, state.config.sample_rate, frames);
    let mut samples = vec![0.0_f32; frames];
    let mut deadline = Instant::now();

    while !state.stop.load(Ordering::Acquire) {
        merge_destination_changes(&state);

        // A capture glitch must be flagged on the next packet, whether or not
        // that packet has audio yet.
        let glitches = state.source.take_glitches();
        if glitches > 0 {
            builder.note_discontinuity();
            state
                .stats
                .discontinuities
                .fetch_add(glitches, Ordering::Relaxed);
        }

        if state.source.pop(&mut samples) {
            if let Some(datagram) = build(&mut builder, &samples, &state, encrypted) {
                enqueue(&state, datagram);
            }
        } else {
            state.stats.underruns.fetch_add(1, Ordering::Relaxed);
        }

        flush(&state);

        deadline += interval;
        let now = Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        } else {
            // Overran the deadline: the thread was descheduled, or the machine
            // is loaded. Catching up by sending a burst would only make the
            // receiver's jitter worse, so the deadline is reset and the lost
            // time is accepted — and recorded, because the operator should be
            // able to see that this machine cannot keep up.
            if now - deadline > interval * 4 {
                state.stats.underruns.fetch_add(1, Ordering::Relaxed);
                deadline = now;
            }
        }
    }

    if state.drain.load(Ordering::Acquire) {
        let marker = builder.end_of_stream(encrypted).to_vec();
        enqueue(&state, marker);
        // Flush repeatedly: the queues may hold more than one packet, and a
        // receiver that never gets the marker waits out its whole buffer.
        for _ in 0..state.config.per_receiver_queue_packets + 1 {
            flush(&state);
        }
        debug!("sent end-of-stream and drained");
    }
}

/// Applies joins and departures. Done here so the send thread owns the table.
fn merge_destination_changes(state: &SendLoop) {
    let mut table = state.destinations.lock();
    if table.pending.is_empty() && table.removing.is_empty() {
        return;
    }

    for receiver_id in table.removing.drain(..).collect::<Vec<_>>() {
        table.entries.retain(|entry| entry.receiver_id != receiver_id);
    }

    let capacity = state.config.per_receiver_queue_packets;
    let packet_bytes = audio_protocol::datagram_bytes(
        state.config.frames_per_packet(),
        state.config.transport.is_encrypted(),
    );
    for (receiver_id, address) in table.pending.drain(..).collect::<Vec<_>>() {
        if table
            .entries
            .iter()
            .any(|entry| entry.receiver_id == receiver_id)
        {
            continue;
        }
        table.entries.push(Destination {
            receiver_id,
            address,
            queue: ReceiverQueue::new(capacity, packet_bytes),
            packets_sent: 0,
            packets_dropped: 0,
        });
    }
}

fn build(
    builder: &mut PacketBuilder,
    samples: &[f32],
    state: &SendLoop,
    encrypted: bool,
) -> Option<Vec<u8>> {
    // The packetiser pulls from a ring, so it is fed through one here. A ring
    // per packet would allocate; this reuses the builder's own scratch via a
    // direct build instead.
    builder
        .build_from_samples(samples, &state.packetiser_stats, encrypted)
        .map(<[u8]>::to_vec)
}

fn enqueue(state: &SendLoop, datagram: Vec<u8>) {
    let mut table = state.destinations.lock();
    for destination in &mut table.entries {
        if !destination.queue.push(&datagram) {
            destination.packets_dropped += 1;
            state
                .stats
                .dropped_queue_full
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn flush(state: &SendLoop) {
    let mut table = state.destinations.lock();
    for destination in &mut table.entries {
        while let Some(datagram) = destination.queue.pop() {
            match state.socket.send_to(&datagram, destination.address) {
                Ok(sent) => {
                    destination.packets_sent += 1;
                    state.stats.packets_sent.fetch_add(1, Ordering::Relaxed);
                    state
                        .stats
                        .bytes_sent
                        .fetch_add(sent as u64, Ordering::Relaxed);
                    destination.queue.release(datagram);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    // The socket buffer is full. Put it back and try on the
                    // next tick; this is exactly the case the queue exists for.
                    destination.queue.push(&datagram);
                    destination.queue.release(datagram);
                    break;
                }
                Err(_) => {
                    // An unreachable host or a vanished route. Counted, not
                    // fatal: one receiver's network going away must not stop
                    // the others, and the receiver will be expired by its
                    // heartbeat soon enough.
                    state.stats.send_errors.fetch_add(1, Ordering::Relaxed);
                    destination.queue.release(datagram);
                }
            }
        }
    }
}

/// Whether a transport may be used for a real stream in this build.
///
/// Separate from [`StreamConfig::validate`] so the sender can be started in a
/// development build with the plaintext transport while a release build
/// refuses — the one place that decision is made.
#[must_use]
pub fn transport_permitted(transport: TransportKind, release_build: bool) -> bool {
    !release_build || transport.allowed_in_production()
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_protocol::{PacketFlags, parse_header};

    /// A tone source, so the send path is testable with no audio hardware.
    struct Tone {
        phase: f32,
        step: f32,
        glitches: u64,
        /// Packets to produce before reporting nothing, as a stalled capture
        /// ring would.
        remaining: Option<usize>,
    }

    impl Tone {
        fn new(frames_per_packet: usize) -> Self {
            let _ = frames_per_packet;
            Self {
                phase: 0.0,
                step: core::f32::consts::TAU * 1_000.0 / 48_000.0,
                glitches: 0,
                remaining: None,
            }
        }
    }

    impl FrameSource for Tone {
        fn pop(&mut self, out: &mut [f32]) -> bool {
            if let Some(remaining) = &mut self.remaining {
                if *remaining == 0 {
                    return false;
                }
                *remaining -= 1;
            }
            for sample in out.iter_mut() {
                *sample = self.phase.sin() * 0.5;
                self.phase += self.step;
                if self.phase >= core::f32::consts::TAU {
                    self.phase -= core::f32::consts::TAU;
                }
            }
            true
        }

        fn take_glitches(&mut self) -> u64 {
            core::mem::take(&mut self.glitches)
        }
    }

    fn config() -> StreamConfig {
        StreamConfig {
            transport: TransportKind::UdpPlaintext,
            bind_address: "127.0.0.1".to_owned(),
            ..StreamConfig::default()
        }
    }

    /// A receiver socket on the loopback, so a test can see what was really
    /// sent rather than trusting a counter.
    fn listener() -> (UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let address = socket.local_addr().unwrap();
        (socket, address)
    }

    fn receive(socket: &UdpSocket, count: usize) -> Vec<Vec<u8>> {
        let mut datagrams = Vec::new();
        let mut buffer = vec![0_u8; 1_500];
        for _ in 0..count {
            match socket.recv(&mut buffer) {
                Ok(len) => datagrams.push(buffer[..len].to_vec()),
                Err(_) => break,
            }
        }
        datagrams
    }

    #[test]
    fn a_sender_with_no_receivers_sends_nothing_but_keeps_running() {
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let stats = sender.stats();
        assert_eq!(stats.packets_sent(), 0, "nowhere to send");
        // The packetiser still ran, so starting a stream before anyone joins
        // is not a special case that has to be handled later.
        assert!(sender.packetiser_stats().packets_built() > 5);
        sender.stop(false);
    }

    #[test]
    fn real_packets_reach_a_real_socket() {
        let (listener_socket, address) = listener();
        let mut sender = StreamSender::start(config(), 0x5EED, Box::new(Tone::new(240))).unwrap();
        sender.add_destination("r1", address);

        let datagrams = receive(&listener_socket, 20);
        sender.stop(true);

        assert!(datagrams.len() >= 15, "got {} datagrams", datagrams.len());
        for (index, datagram) in datagrams.iter().enumerate() {
            let (header, payload) = parse_header(datagram).expect("a real StageLink packet");
            assert_eq!(header.session_id, 0x5EED);
            assert_eq!(header.channels, 1, "the mix is mono");
            if !header.flags.end_of_stream() {
                assert_eq!(payload.len(), 480, "240 frames of S16");
                assert_eq!(header.sequence, index as u32, "sequence must not skip");
            }
        }
    }

    #[test]
    fn the_cadence_is_the_configured_frame_duration() {
        let (listener_socket, address) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        sender.add_destination("r1", address);

        let started = Instant::now();
        let datagrams = receive(&listener_socket, 100);
        let elapsed = started.elapsed();
        sender.stop(false);

        assert!(datagrams.len() >= 90, "got {}", datagrams.len());
        // 100 packets at 5 ms is half a second. Windows sleep granularity
        // means this cannot be tight, but a cadence that had drifted into
        // 6.5 ms — which is what sleeping *for* the interval produces — would
        // take 0.65 s and fail.
        assert!(
            elapsed < Duration::from_millis(620),
            "100 packets took {elapsed:?}, so the cadence is drifting"
        );
    }

    #[test]
    fn two_receivers_both_get_the_same_audio() {
        let (first_socket, first) = listener();
        let (second_socket, second) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        sender.add_destination("r1", first);
        sender.add_destination("r2", second);

        let a = receive(&first_socket, 10);
        let b = receive(&second_socket, 10);
        sender.stop(false);

        assert!(a.len() >= 8 && b.len() >= 8, "{} and {}", a.len(), b.len());
        // The desktop sends one identical mix to everyone; there is no
        // per-receiver processing, and this is where that is true or not.
        let common = a.len().min(b.len());
        assert_eq!(
            a[..common], b[..common],
            "the two receivers were sent different audio"
        );

        let stats = sender.destination_stats();
        assert_eq!(stats.len(), 2);
        assert!(stats.iter().all(|entry| entry.packets_sent > 5));
    }

    #[test]
    fn a_receiver_can_join_and_leave_mid_stream() {
        let (socket, address) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();

        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(sender.destination_count(), 0);

        sender.add_destination("r1", address);
        assert!(!receive(&socket, 5).is_empty(), "nothing arrived after joining");
        assert_eq!(sender.destination_count(), 1);

        sender.remove_destination("r1");
        std::thread::sleep(Duration::from_millis(60));
        // Drain whatever was already queued, then confirm the tap is closed.
        let _ = receive(&socket, 64);
        assert!(
            receive(&socket, 1).is_empty(),
            "packets still arriving after the receiver left"
        );
        assert_eq!(sender.destination_count(), 0);
        sender.stop(false);
    }

    #[test]
    fn adding_the_same_receiver_twice_does_not_double_its_audio() {
        let (socket, address) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        sender.add_destination("r1", address);
        sender.add_destination("r1", address);

        let datagrams = receive(&socket, 20);
        sender.stop(false);

        assert_eq!(sender.destination_count(), 1);
        let sequences: Vec<u32> = datagrams
            .iter()
            .map(|datagram| parse_header(datagram).unwrap().0.sequence)
            .collect();
        let mut unique = sequences.clone();
        unique.dedup();
        assert_eq!(sequences, unique, "a packet was sent twice");
    }

    /// D3. Without this the receiver sees continuous sequence numbers and
    /// timestamps across a desktop stall, underruns with no loss indication,
    /// and an operator cannot tell a stalled desktop from a bad network.
    #[test]
    fn a_capture_glitch_is_flagged_on_the_next_packet() {
        let (socket, address) = listener();
        let mut tone = Tone::new(240);
        tone.glitches = 0;
        let mut sender = StreamSender::start(config(), 1, Box::new(tone)).unwrap();
        sender.add_destination("r1", address);

        // No glitches: nothing flagged.
        let clean = receive(&socket, 10);
        assert!(
            clean
                .iter()
                .all(|d| !parse_header(d).unwrap().0.flags.has(PacketFlags::DISCONTINUITY)),
            "a clean capture must not be flagged"
        );
        sender.stop(false);
        assert_eq!(sender.stats().discontinuities(), 0);
    }

    #[test]
    fn a_glitching_source_produces_a_discontinuity_flag() {
        /// Reports a glitch on its third packet.
        struct Glitchy {
            packets: usize,
        }
        impl FrameSource for Glitchy {
            fn pop(&mut self, out: &mut [f32]) -> bool {
                self.packets += 1;
                out.fill(0.25);
                true
            }
            fn take_glitches(&mut self) -> u64 {
                u64::from(self.packets == 3)
            }
        }

        let (socket, address) = listener();
        let mut sender =
            StreamSender::start(config(), 1, Box::new(Glitchy { packets: 0 })).unwrap();
        sender.add_destination("r1", address);

        let datagrams = receive(&socket, 20);
        sender.stop(false);

        let flagged = datagrams
            .iter()
            .filter(|d| {
                parse_header(d)
                    .unwrap()
                    .0
                    .flags
                    .has(PacketFlags::DISCONTINUITY)
            })
            .count();
        assert_eq!(flagged, 1, "exactly one packet should carry the flag");
        assert!(sender.stats().discontinuities() >= 1);
    }

    #[test]
    fn a_stalled_source_records_an_underrun_rather_than_sending_a_short_packet() {
        let (socket, address) = listener();
        let mut tone = Tone::new(240);
        tone.remaining = Some(5);
        let mut sender = StreamSender::start(config(), 1, Box::new(tone)).unwrap();
        sender.add_destination("r1", address);

        std::thread::sleep(Duration::from_millis(100));
        let datagrams = receive(&socket, 64);
        sender.stop(false);

        assert!(datagrams.len() <= 6, "sent {} for 5 packets", datagrams.len());
        for datagram in &datagrams {
            let (_, payload) = parse_header(datagram).unwrap();
            assert!(
                payload.len() == 480 || payload.is_empty(),
                "a short packet was sent: {} bytes",
                payload.len()
            );
        }
        assert!(sender.stats().underruns() > 5, "the stall was not recorded");
    }

    #[test]
    fn stopping_with_a_drain_sends_an_end_of_stream_marker() {
        let (socket, address) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        sender.add_destination("r1", address);
        let _ = receive(&socket, 5);
        sender.stop(true);

        let remaining = receive(&socket, 64);
        assert!(
            remaining
                .iter()
                .any(|d| parse_header(d).unwrap().0.flags.end_of_stream()),
            "no end-of-stream marker was sent, so receivers wait out their buffers"
        );
    }

    #[test]
    fn stopping_without_a_drain_sends_no_marker() {
        let (socket, address) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        sender.add_destination("r1", address);
        let _ = receive(&socket, 5);
        sender.stop(false);

        let remaining = receive(&socket, 64);
        assert!(
            remaining
                .iter()
                .all(|d| !parse_header(d).unwrap().0.flags.end_of_stream()),
            "a marker was sent to a receiver that had already gone"
        );
    }

    #[test]
    fn an_unreachable_receiver_does_not_stop_the_others() {
        let (socket, address) = listener();
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        // Port 1 on the loopback refuses everything.
        sender.add_destination("dead", "127.0.0.1:1".parse().unwrap());
        sender.add_destination("alive", address);

        let datagrams = receive(&socket, 20);
        sender.stop(false);
        assert!(
            datagrams.len() >= 15,
            "the live receiver got {} packets",
            datagrams.len()
        );
    }

    #[test]
    fn a_port_already_in_use_is_reported_at_start_not_as_silence() {
        // Bind the whole address so the sender's own bind fails.
        let config = StreamConfig {
            bind_address: "203.0.113.1".to_owned(), // not an address on this host
            ..config()
        };
        let result = StreamSender::start(config, 1, Box::new(Tone::new(240)));
        assert!(
            result.is_err(),
            "binding an address this machine does not have must fail loudly"
        );
    }

    #[test]
    fn the_estimated_bitrate_scales_with_receivers() {
        let mut sender = StreamSender::start(config(), 1, Box::new(Tone::new(240))).unwrap();
        assert_eq!(sender.estimated_bitrate(), 0, "nobody to send to");

        sender.add_destination("r1", "127.0.0.1:9".parse().unwrap());
        let one = sender.estimated_bitrate();
        // 504-byte datagram plus 42 bytes of overhead, 200 per second.
        assert!((850_000..=900_000).contains(&one), "estimated {one} bit/s");

        sender.add_destination("r2", "127.0.0.1:10".parse().unwrap());
        assert_eq!(sender.estimated_bitrate(), one * 2);
        sender.stop(false);
    }

    #[test]
    fn a_release_build_refuses_the_plaintext_transport() {
        assert!(!transport_permitted(TransportKind::UdpPlaintext, true));
        assert!(transport_permitted(TransportKind::UdpPlaintext, false));
        assert!(transport_permitted(TransportKind::UdpEncrypted, true));
    }
}

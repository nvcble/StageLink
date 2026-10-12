//! The audio socket, and the thread that feeds the jitter buffer.
//!
//! Two threads, matching the two that exist on a phone:
//!
//! * **network** — blocking `recv_from` into `receiver_core::NetworkInput`.
//!   Blocking rather than async on purpose: it does one thing, it should do it
//!   with the lowest possible wake-up latency, and an async runtime's
//!   scheduling adds jitter to the one measurement the jitter buffer is
//!   adapting on.
//! * **render** — pulls from `receiver_core::AudioOutput`. On a phone this is
//!   the Oboe callback. Here it is a paced loop writing to a file, which is
//!   the same contract: somebody asks for N frames on a deadline and silence
//!   is not an acceptable answer.

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use receiver_core::receiver::{Accepted, NetworkInput};
use tracing::{debug, info, warn};

/// Counters about the socket itself, as distinct from the protocol.
#[derive(Debug, Default)]
pub struct SocketStats {
    datagrams: AtomicU64,
    /// Datagrams from an address other than the desktop we joined.
    foreign: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
}

impl SocketStats {
    #[must_use]
    pub fn datagrams(&self) -> u64 {
        self.datagrams.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn foreign(&self) -> u64 {
        self.foreign.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
}

/// The network thread.
pub struct AudioSocket {
    stats: Arc<SocketStats>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    local_address: std::net::SocketAddr,
}

/// A bound socket that is not yet receiving.
///
/// The two steps are separate because of an ordering problem with no other
/// answer: the handshake has to declare the port this receiver bound, so the
/// socket must exist first — but the decryption key is derived from the
/// session id, which only arrives *in* the handshake, so the `NetworkInput`
/// that holds it has to be built afterwards. Binding without running lets
/// both be true.
pub struct BoundSocket {
    socket: UdpSocket,
    local_address: std::net::SocketAddr,
}

impl BoundSocket {
    /// Binds without starting a thread.
    pub fn bind(address: &str) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(address)?;
        let local_address = socket.local_addr()?;
        Ok(Self { socket, local_address })
    }

    /// The port this receiver is listening on, to declare in the handshake.
    #[must_use]
    pub const fn local_address(&self) -> std::net::SocketAddr {
        self.local_address
    }

    /// Starts the receive thread.
    pub fn run(
        self,
        expect_from: Option<std::net::IpAddr>,
        input: NetworkInput,
    ) -> std::io::Result<AudioSocket> {
        AudioSocket::run(self.socket, self.local_address, expect_from, input)
    }
}

impl AudioSocket {
    /// Binds and starts receiving, in one step.
    ///
    /// For callers that know the session id up front — a plaintext stream, or
    /// a test. Joining uses [`BoundSocket`] instead.
    pub fn start(
        bind: &str,
        expect_from: Option<std::net::IpAddr>,
        input: NetworkInput,
    ) -> std::io::Result<Self> {
        let bound = BoundSocket::bind(bind)?;
        bound.run(expect_from, input)
    }

    /// `expect_from` is the desktop's address. Datagrams from anywhere else
    /// are counted and dropped: on a venue network something else will
    /// eventually send to this port, and a receiver that fed stray traffic to
    /// its jitter buffer would be playing whatever arrived.
    fn run(
        socket: UdpSocket,
        local_address: std::net::SocketAddr,
        expect_from: Option<std::net::IpAddr>,
        mut input: NetworkInput,
    ) -> std::io::Result<Self> {
        // A timeout rather than a blocking read, so a stream that stops dead
        // still lets the thread notice it has been asked to stop.
        socket.set_read_timeout(Some(std::time::Duration::from_millis(250)))?;

        let stats = Arc::new(SocketStats::default());
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("stagelink-receive".to_owned())
                .spawn(move || {
                    // One MTU. A larger buffer would silently accept a
                    // datagram the protocol says cannot exist.
                    let mut buffer = vec![0_u8; 1_500];
                    while !stop.load(Ordering::Acquire) {
                        match socket.recv_from(&mut buffer) {
                            Ok((len, from)) => {
                                stats.datagrams.fetch_add(1, Ordering::Relaxed);
                                stats.bytes.fetch_add(len as u64, Ordering::Relaxed);

                                if expect_from.is_some_and(|expected| from.ip() != expected) {
                                    stats.foreign.fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }

                                let outcome = input.accept(&buffer[..len], now_us());
                                if matches!(outcome, Accepted::Encrypted) {
                                    // Loud, once: a receiver silently playing
                                    // nothing because every packet is
                                    // encrypted is the worst kind of failure.
                                    warn!(
                                        "the desktop is sending encrypted audio and this build \
                                         has no key"
                                    );
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::TimedOut
                                ) => {}
                            Err(error) => {
                                stats.errors.fetch_add(1, Ordering::Relaxed);
                                debug!(%error, "receive error");
                            }
                        }
                    }
                    debug!("receive thread stopped");
                })?
        };

        info!(%local_address, "listening for audio");
        Ok(Self {
            stats,
            stop,
            thread: Some(thread),
            local_address,
        })
    }

    #[must_use]
    pub fn stats(&self) -> Arc<SocketStats> {
        Arc::clone(&self.stats)
    }

    #[must_use]
    pub const fn local_address(&self) -> std::net::SocketAddr {
        self.local_address
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AudioSocket {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A monotonic microsecond clock for packet arrival times.
///
/// Monotonic, not wall clock: arrival times are only ever compared with each
/// other, and a wall clock can step backwards when NTP corrects it — which
/// would make the jitter measurement briefly nonsense.
pub fn now_us() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_protocol::{AudioPacketHeader, HEADER_BYTES, PacketFlags, encode_header};
    use receiver_core::receiver::ReceiverConfig;

    const FRAMES: usize = 240;

    fn packet(sequence: u32, value: f32) -> Vec<u8> {
        let mut datagram = vec![0_u8; HEADER_BYTES + FRAMES * 2];
        let header = AudioPacketHeader::new(
            0x5EED,
            sequence,
            u64::from(sequence) * 5_000,
            PacketFlags::empty(),
        );
        encode_header(&header, &mut datagram).unwrap();
        let samples = vec![value; FRAMES];
        audio_core::convert::f32_to_pcm_s16le(&samples, &mut datagram[HEADER_BYTES..]);
        datagram
    }

    #[test]
    fn the_monotonic_clock_only_moves_forward() {
        let first = now_us();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(now_us() > first);
    }

    #[test]
    fn real_packets_on_a_real_socket_reach_the_jitter_buffer() {
        let (net, mut audio) =
            receiver_core::receiver::new(ReceiverConfig::new(48_000, FRAMES));
        let mut socket = AudioSocket::start("127.0.0.1:0", None, net).unwrap();
        let address = socket.local_address();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        for sequence in 0..40 {
            sender.send_to(&packet(sequence, 0.5), address).unwrap();
        }

        // Give the receive thread a moment, then render.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut block = vec![0.0_f32; FRAMES];
        let mut peak = 0.0_f32;
        for _ in 0..30 {
            audio.render(&mut block);
            peak = peak.max(block.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs())));
        }

        assert!(socket.stats().datagrams() >= 35, "{}", socket.stats().datagrams());
        assert!(peak > 0.4, "the audio did not come through: peak {peak}");
        assert_eq!(audio.telemetry().packets_lost, 0);
        socket.stop();
    }

    /// On a venue network something else will eventually send to this port. A
    /// receiver that fed stray traffic to its jitter buffer would be playing
    /// whatever arrived.
    #[test]
    fn traffic_from_another_address_is_counted_and_dropped() {
        let (net, mut audio) =
            receiver_core::receiver::new(ReceiverConfig::new(48_000, FRAMES));
        // Expect audio only from an address nothing will send from.
        let mut socket =
            AudioSocket::start("127.0.0.1:0", Some("10.255.255.1".parse().unwrap()), net)
                .unwrap();
        let address = socket.local_address();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        for sequence in 0..20 {
            sender.send_to(&packet(sequence, 0.5), address).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut block = vec![0.0_f32; FRAMES];
        for _ in 0..20 {
            audio.render(&mut block);
        }

        assert!(socket.stats().foreign() >= 15, "foreign traffic was not rejected");
        assert_eq!(
            audio.telemetry().packets_received,
            0,
            "stray traffic reached the jitter buffer"
        );
        socket.stop();
    }

    #[test]
    fn stray_non_stagelink_traffic_is_rejected_without_a_panic() {
        let (net, _audio) = receiver_core::receiver::new(ReceiverConfig::new(48_000, FRAMES));
        let mut socket = AudioSocket::start("127.0.0.1:0", None, net).unwrap();
        let address = socket.local_address();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        for junk in [
            b"GET / HTTP/1.1\r\n\r\n".to_vec(),
            vec![0_u8; 3],
            vec![0xFF_u8; 1_400],
        ] {
            sender.send_to(&junk, address).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(socket.stats().datagrams() >= 3);
        socket.stop();
    }

    #[test]
    fn a_port_already_in_use_is_reported() {
        let (net, _audio) = receiver_core::receiver::new(ReceiverConfig::new(48_000, FRAMES));
        let held = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = held.local_addr().unwrap();

        let result = AudioSocket::start(&address.to_string(), None, net);
        assert!(
            result.is_err(),
            "binding a port another process holds must fail loudly"
        );
    }
}

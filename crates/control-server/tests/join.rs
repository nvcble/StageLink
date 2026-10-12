//! The control server, over a real socket.
//!
//! Driven as a phone would drive it: a WebSocket client that sends a `hello`
//! and reads the `welcome`. Nothing here stubs the transport, because the
//! handshake is exactly where a protocol mistake hides — a unit test of
//! `admit()` would have passed every one of the bugs these tests are for.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use control_server::server::{ControlHost, ControlServerConfig, HEARTBEAT_MISS_LIMIT};
use control_server::{Admission, ControlServer, ServerEvent};
use desktop_ipc::codec::{Envelope, EnvelopeKind};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use receiver_session::JoinCode;
use serde_json::{Value, json};

const OURS: &str = "1.0.0";

/// What the host was asked to do, so a test can check the effects rather than
/// only the replies.
#[derive(Default)]
struct Recorded {
    joined: Vec<String>,
    left: Vec<String>,
    heartbeats: Vec<(String, i64)>,
    telemetry: Vec<(String, Value)>,
}

struct Host {
    code: Mutex<Option<JoinCode>>,
    max_receivers: Mutex<u16>,
    revoked: Mutex<Vec<String>>,
    stream_params: Mutex<Option<Value>>,
    stale: Mutex<Vec<String>>,
    recorded: Mutex<Recorded>,
    now: AtomicI64,
    expiry_asked: AtomicBool,
}

impl Host {
    fn new() -> Arc<Self> {
        let now = 1_700_000_000_000;
        Arc::new(Self {
            code: Mutex::new(Some(JoinCode::mint(
                now,
                receiver_session::join_code::DEFAULT_TTL_MS,
            ))),
            max_receivers: Mutex::new(16),
            revoked: Mutex::new(Vec::new()),
            stream_params: Mutex::new(None),
            stale: Mutex::new(Vec::new()),
            recorded: Mutex::new(Recorded::default()),
            now: AtomicI64::new(now),
            expiry_asked: AtomicBool::new(false),
        })
    }

    fn token(&self) -> String {
        self.code.lock().as_ref().unwrap().token.clone()
    }
}

impl ControlHost for Host {
    fn join_code(&self) -> Option<JoinCode> {
        self.code.lock().clone()
    }
    fn control_protocol_version(&self) -> String {
        OURS.to_owned()
    }
    fn service_version(&self) -> String {
        "0.1.0-test".to_owned()
    }
    fn max_receivers(&self) -> u16 {
        *self.max_receivers.lock()
    }
    fn is_revoked(&self, receiver_id: &str) -> bool {
        self.revoked.lock().iter().any(|id| id == receiver_id)
    }
    fn stream_params(&self) -> Option<Value> {
        self.stream_params.lock().clone()
    }
    fn on_joined(&self, admission: &Admission) {
        self.recorded
            .lock()
            .joined
            .push(admission.receiver_id.clone());
    }
    fn on_left(&self, receiver_id: &str) {
        self.recorded.lock().left.push(receiver_id.to_owned());
    }
    fn on_heartbeat(&self, receiver_id: &str, sent_at: i64) {
        self.recorded
            .lock()
            .heartbeats
            .push((receiver_id.to_owned(), sent_at));
    }
    fn on_telemetry(&self, receiver_id: &str, report: &Value) {
        self.recorded
            .lock()
            .telemetry
            .push((receiver_id.to_owned(), report.clone()));
    }
    fn expire_stale(&self, _now_ms: i64, _timeout_ms: i64) -> Vec<String> {
        self.expiry_asked.store(true, Ordering::Relaxed);
        core::mem::take(&mut *self.stale.lock())
    }
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }
}

type Socket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn server(host: Arc<Host>) -> (ControlServer, SocketAddr) {
    let server = ControlServer::start(
        ControlServerConfig {
            bind_address: "127.0.0.1".to_owned(),
            port: 0,
        },
        host,
    )
    .await
    .expect("the server must bind");
    let address = server.local_address();
    (server, address)
}

async fn connect(address: SocketAddr) -> Socket {
    let url = format!("ws://{address}/");
    let (socket, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("the websocket handshake must succeed");
    socket
}

fn hello(token: &str, version: &str, with_receiver: bool) -> String {
    let receiver = with_receiver.then(|| {
        control_server::server::receiver_identity(
            "Pixel 8",
            "android",
            "0.1.0",
            &"ab".repeat(32),
        )
    });
    Envelope::new(EnvelopeKind::Hello {
        token: token.to_owned(),
        client: "stagelink-mobile/0.1.0".to_owned(),
        control_protocol_version: version.to_owned(),
        receiver,
    })
    .encode()
    .unwrap()
    .trim_end()
    .to_owned()
}

/// Reads the next envelope, or `None` if the socket closed first.
async fn next(socket: &mut Socket) -> Option<Envelope> {
    let message = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .ok()??
        .ok()?;
    let text = message.into_text().ok()?;
    Envelope::decode(&text).ok()
}

async fn send(socket: &mut Socket, line: String) {
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(line))
        .await
        .expect("send must succeed");
}

#[tokio::test]
async fn a_receiver_with_a_valid_code_is_welcomed_and_appears_in_the_roster() {
    let host = Host::new();
    let (mut server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    let welcome = next(&mut socket).await.expect("a welcome must arrive");

    match welcome.kind {
        EnvelopeKind::Welcome {
            service_version,
            control_protocol_version,
            audio_protocol_version,
            ..
        } => {
            assert_eq!(service_version, "0.1.0-test");
            assert_eq!(control_protocol_version, OURS);
            assert_eq!(audio_protocol_version, 1);
        }
        other => panic!("expected a welcome, got {other:?}"),
    }

    // The effect that matters: the host was told, so the roster has it and the
    // sender starts sending to it.
    assert_eq!(host.recorded.lock().joined.len(), 1);

    let mut joined = false;
    while let Some(event) = server.try_next_event() {
        if matches!(event, ServerEvent::Joined { .. }) {
            joined = true;
        }
    }
    assert!(joined, "no Joined event was reported");
}

#[tokio::test]
async fn a_wrong_token_is_disconnected_without_a_welcome() {
    let host = Host::new();
    let (mut server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&"0".repeat(64), OURS, true)).await;

    assert!(
        next(&mut socket).await.is_none(),
        "a rejected receiver must not be welcomed"
    );
    assert!(host.recorded.lock().joined.is_empty());

    // The operator still learns why, which is the point of refusing with a
    // reason even though the receiver is told nothing.
    let mut refusal = None;
    while let Some(event) = server.try_next_event() {
        if let ServerEvent::Refused { code, reason, .. } = event {
            refusal = Some((code, reason));
        }
    }
    let (code, reason) = refusal.expect("the refusal must be reported");
    assert_eq!(code, "PAIRING_REJECTED");
    assert!(reason.contains("token"), "got {reason}");
}

#[tokio::test]
async fn a_hello_without_a_receiver_block_is_refused() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    // The local Electron client's shape, arriving on the network port.
    send(&mut socket, hello(&host.token(), OURS, false)).await;
    assert!(next(&mut socket).await.is_none());
    assert!(host.recorded.lock().joined.is_empty());
}

#[tokio::test]
async fn an_incompatible_protocol_major_is_refused() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), "2.0.0", true)).await;
    assert!(next(&mut socket).await.is_none());
}

#[tokio::test]
async fn an_expired_code_is_refused() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    // Five minutes and a second after the code was minted.
    host.now.fetch_add(301_000, Ordering::Relaxed);

    let mut socket = connect(address).await;
    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(
        next(&mut socket).await.is_none(),
        "expiry is the point of a short-lived credential"
    );
}

#[tokio::test]
async fn with_no_code_minted_nobody_can_join() {
    let host = Host::new();
    *host.code.lock() = None;
    let (_server, address) = server(Arc::clone(&host)).await;

    let mut socket = connect(address).await;
    send(&mut socket, hello(&"a".repeat(64), OURS, true)).await;
    assert!(next(&mut socket).await.is_none());
}

#[tokio::test]
async fn a_revoked_device_cannot_rejoin_with_a_fresh_code() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    // Join once to learn the id the desktop assigns.
    let mut socket = connect(address).await;
    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());
    let receiver_id = host.recorded.lock().joined[0].clone();
    drop(socket);

    host.revoked.lock().push(receiver_id);

    let mut socket = connect(address).await;
    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(
        next(&mut socket).await.is_none(),
        "revocation must outrank a valid code"
    );
}

#[tokio::test]
async fn the_receiver_limit_is_enforced() {
    let host = Host::new();
    *host.max_receivers.lock() = 1;
    let (_server, address) = server(Arc::clone(&host)).await;

    let mut first = connect(address).await;
    send(&mut first, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut first).await.is_some());

    // A *different* device, so it is not treated as a reconnect.
    let mut second = connect(address).await;
    let second_hello = Envelope::new(EnvelopeKind::Hello {
        token: host.token(),
        client: "stagelink-mobile/0.1.0".to_owned(),
        control_protocol_version: OURS.to_owned(),
        receiver: Some(control_server::server::receiver_identity(
            "Pixel 7",
            "android",
            "0.1.0",
            &"cd".repeat(32),
        )),
    })
    .encode()
    .unwrap()
    .trim_end()
    .to_owned();
    send(&mut second, second_hello).await;

    assert!(
        next(&mut second).await.is_none(),
        "the session is full and the second device must be refused"
    );
    assert_eq!(host.recorded.lock().joined.len(), 1);
}

#[tokio::test]
async fn a_heartbeat_is_acknowledged_with_the_desktop_clock_and_the_echo() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());

    let request = Envelope::new(EnvelopeKind::Request {
        id: "h1".to_owned(),
        method: "receiver.heartbeat".to_owned(),
        params: json!({ "sentAt": 1_234_567 }),
    })
    .encode()
    .unwrap()
    .trim_end()
    .to_owned();
    send(&mut socket, request).await;

    let reply = next(&mut socket).await.expect("an ack must arrive");
    match reply.kind {
        EnvelopeKind::Response { id, ok, result: Some(result), .. } => {
            assert_eq!(id, "h1");
            assert!(ok);
            // Echoed so the receiver gets its round-trip time from one frame.
            assert_eq!(result["sentAt"], 1_234_567);
            assert_eq!(result["at"], host.now_ms());
            assert_eq!(result["missLimit"], HEARTBEAT_MISS_LIMIT);
        }
        other => panic!("expected a response, got {other:?}"),
    }

    assert_eq!(host.recorded.lock().heartbeats.len(), 1);
    assert_eq!(host.recorded.lock().heartbeats[0].1, 1_234_567);
}

#[tokio::test]
async fn a_telemetry_report_is_accepted_and_handed_to_the_host_verbatim() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());

    let report = json!({
        "reportedAt": 1_700_000_001_000_i64,
        "packetsReceived": 1_990,
        "packetsLost": 10,
        "lossPercent": 0.5,
        "jitterMs": 2.5,
        "bufferHealthMs": 40.0,
        "underruns": 0,
        "playbackState": "playing",
        "outputLatencyMs": 38.0,
        "resampleRatioPpm": -12.0,
    });
    let request = Envelope::new(EnvelopeKind::Request {
        id: "t1".to_owned(),
        method: "receiver.telemetry".to_owned(),
        params: report.clone(),
    })
    .encode()
    .unwrap()
    .trim_end()
    .to_owned();
    send(&mut socket, request).await;

    let reply = next(&mut socket).await.expect("an ack must arrive");
    match reply.kind {
        EnvelopeKind::Response { ok, result: Some(result), .. } => {
            assert!(ok);
            assert_eq!(result["accepted"], true);
        }
        other => panic!("expected a response, got {other:?}"),
    }

    let recorded = host.recorded.lock();
    assert_eq!(recorded.telemetry.len(), 1);
    // Verbatim: the desktop stamps provenance on arrival rather than trusting
    // the receiver to label its own claims.
    assert_eq!(recorded.telemetry[0].1, report);
}

/// A phone must not be able to revoke another device, stop the stream, or read
/// the operator's settings. The method registry is shared, so this is the only
/// thing standing between a receiver and the whole control surface.
#[tokio::test]
async fn a_receiver_cannot_call_the_operators_methods() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());

    for method in [
        "receivers.revoke",
        "stream.stop",
        "settings.get",
        "service.shutdown",
        "audio.listDevices",
    ] {
        let request = Envelope::new(EnvelopeKind::Request {
            id: method.to_owned(),
            method: method.to_owned(),
            params: json!({}),
        })
        .encode()
        .unwrap()
        .trim_end()
        .to_owned();
        send(&mut socket, request).await;

        let reply = next(&mut socket).await.expect("a refusal must arrive");
        match reply.kind {
            EnvelopeKind::Response { ok, error: Some(error), .. } => {
                assert!(!ok, "{method} was allowed");
                assert_eq!(error.code, "UNAUTHORIZED", "for {method}");
            }
            other => panic!("expected a refusal for {method}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_malformed_message_does_not_end_the_session() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());

    send(&mut socket, "{ not json".to_owned()).await;
    send(&mut socket, "null".to_owned()).await;

    // The connection still works afterwards.
    let request = Envelope::new(EnvelopeKind::Request {
        id: "h1".to_owned(),
        method: "receiver.heartbeat".to_owned(),
        params: json!({ "sentAt": 1 }),
    })
    .encode()
    .unwrap()
    .trim_end()
    .to_owned();
    send(&mut socket, request).await;
    assert!(
        next(&mut socket).await.is_some(),
        "one bad line should not cost a performer their monitor feed"
    );
}

#[tokio::test]
async fn the_welcome_carries_the_stream_parameters_when_a_stream_is_running() {
    let host = Host::new();
    *host.stream_params.lock() = Some(json!({
        "sessionId": 0x5EED,
        "sampleRate": 48_000,
        "frameDurationMs": 5.0,
        "payloadFormat": "pcm_s16le",
        "transport": "udp-plaintext",
        "streamPort": 45_872,
        "framesPerPacket": 240,
    }));
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    let welcome = next(&mut socket).await.expect("a welcome must arrive");

    match welcome.kind {
        EnvelopeKind::Welcome { stream: Some(stream), .. } => {
            // The only place a receiver learns the sample rate. Getting it
            // wrong plays the mix sharp in a performer's ears.
            assert_eq!(stream.sample_rate, 48_000);
            assert_eq!(stream.frames_per_packet, 240);
            assert_eq!(stream.session_id, 0x5EED);
        }
        other => panic!("expected stream parameters, got {other:?}"),
    }
}

#[tokio::test]
async fn no_stream_running_is_a_welcome_without_parameters_not_a_refusal() {
    // Joining before the operator starts the stream is normal: people connect
    // their phones while the band is still setting up.
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    match next(&mut socket).await.expect("a welcome must arrive").kind {
        EnvelopeKind::Welcome { stream, .. } => assert!(stream.is_none()),
        other => panic!("expected a welcome, got {other:?}"),
    }
    assert_eq!(host.recorded.lock().joined.len(), 1);
}

#[tokio::test]
async fn a_departing_receiver_is_removed_from_the_roster() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    let mut socket = connect(address).await;
    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());
    let receiver_id = host.recorded.lock().joined[0].clone();

    drop(socket);

    // The server notices the closed socket and tells the host.
    for _ in 0..50 {
        if !host.recorded.lock().left.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(host.recorded.lock().left, vec![receiver_id]);
}

/// A phone that goes into a tunnel does not close its socket, it simply stops.
/// Without the sweep it would hold a roster slot and keep being sent audio
/// into nothing.
#[tokio::test]
async fn a_silent_receiver_is_expired_by_the_sweep() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    let mut socket = connect(address).await;
    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());
    let receiver_id = host.recorded.lock().joined[0].clone();

    // The host decides who is stale, because it owns the heartbeat times.
    host.stale.lock().push(receiver_id.clone());

    for _ in 0..60 {
        if host.recorded.lock().left.contains(&receiver_id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        host.expiry_asked.load(Ordering::Relaxed),
        "the sweep never ran"
    );
    assert!(
        host.recorded.lock().left.contains(&receiver_id),
        "a silent receiver was never expired"
    );
    let _ = socket;
}

#[tokio::test]
async fn a_connection_that_says_nothing_is_dropped_rather_than_holding_a_slot() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    // A port scanner, or a broken client. It must not be welcomed and must
    // not occupy a receiver slot.
    let mut socket = connect(address).await;
    assert!(next(&mut socket).await.is_none());
    assert!(host.recorded.lock().joined.is_empty());
}

#[tokio::test]
async fn two_different_devices_can_both_join() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    for key in ["ab", "cd"] {
        let mut socket = connect(address).await;
        let line = Envelope::new(EnvelopeKind::Hello {
            token: host.token(),
            client: "stagelink-mobile/0.1.0".to_owned(),
            control_protocol_version: OURS.to_owned(),
            receiver: Some(control_server::server::receiver_identity(
                "Phone",
                "android",
                "0.1.0",
                &key.repeat(32),
            )),
        })
        .encode()
        .unwrap()
        .trim_end()
        .to_owned();
        send(&mut socket, line).await;
        assert!(next(&mut socket).await.is_some(), "{key} was not welcomed");
        core::mem::forget(socket);
    }

    let joined = host.recorded.lock().joined.clone();
    assert_eq!(joined.len(), 2);
    assert_ne!(joined[0], joined[1], "two devices must get two ids");
}

/// Phones lose Wi-Fi in a pocket and come back. The second connection wins
/// rather than being refused, or a performer would have to restart the app.
#[tokio::test]
async fn the_same_device_reconnecting_replaces_its_old_session() {
    let host = Host::new();
    let (_server, address) = server(Arc::clone(&host)).await;

    let mut first = connect(address).await;
    send(&mut first, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut first).await.is_some());

    let mut second = connect(address).await;
    send(&mut second, hello(&host.token(), OURS, true)).await;
    assert!(
        next(&mut second).await.is_some(),
        "a reconnect must be welcomed, not refused"
    );

    let joined = host.recorded.lock().joined.clone();
    assert_eq!(joined.len(), 2, "both connections were admitted");
    assert_eq!(joined[0], joined[1], "and they are the same device");
    core::mem::forget(first);
    core::mem::forget(second);
}

#[tokio::test]
async fn an_event_pushed_by_the_desktop_reaches_the_receiver() {
    let host = Host::new();
    let (server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());

    server
        .sessions()
        .broadcast("stream.status", &json!({ "state": "running" }), 42);

    let event = next(&mut socket).await.expect("the event must arrive");
    match event.kind {
        EnvelopeKind::Event { event, payload, at } => {
            assert_eq!(event, "stream.status");
            assert_eq!(payload["state"], "running");
            assert_eq!(at, 42);
        }
        other => panic!("expected an event, got {other:?}"),
    }
}

#[tokio::test]
async fn disconnecting_a_receiver_closes_its_socket() {
    let host = Host::new();
    let (server, address) = server(Arc::clone(&host)).await;
    let mut socket = connect(address).await;

    send(&mut socket, hello(&host.token(), OURS, true)).await;
    assert!(next(&mut socket).await.is_some());
    let receiver_id = host.recorded.lock().joined[0].clone();

    server.disconnect(&receiver_id);

    assert!(
        next(&mut socket).await.is_none(),
        "the receiver's socket should have closed"
    );
}

#[tokio::test]
async fn a_port_already_in_use_is_reported_rather_than_silently_not_listening() {
    let host = Host::new();
    let (server, address) = server(Arc::clone(&host)).await;

    let second = ControlServer::start(
        ControlServerConfig {
            bind_address: "127.0.0.1".to_owned(),
            port: address.port(),
        },
        Host::new(),
    )
    .await;
    assert!(
        second.is_err(),
        "binding a port already in use must fail loudly"
    );
    let _ = server;
}

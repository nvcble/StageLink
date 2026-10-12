//! The listener, the handshake and the per-connection loop.

use std::net::SocketAddr;
use std::sync::Arc;

use desktop_ipc::codec::{Envelope, EnvelopeKind, ReceiverIdentity, ServiceErrorPayload};
use parking_lot::Mutex;
use receiver_session::JoinCode;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::join::{self, Admission, JoinError, JoinRequest};
use crate::session::ReceiverSessions;

/// Heartbeat interval a receiver is told to use.
pub const HEARTBEAT_INTERVAL_MS: i64 = 1_000;
/// Missed heartbeats before a receiver is considered gone.
pub const HEARTBEAT_MISS_LIMIT: u32 = 5;
/// How often the server looks for receivers that have stopped heartbeating.
pub const EXPIRY_SWEEP_MS: u64 = 1_000;

/// Longest control message accepted, matching the pipe's limit.
const MAX_MESSAGE_BYTES: usize = 1_048_576;

/// What the server needs from the rest of the service.
///
/// A trait rather than a direct dependency on `stagelink-service`, which would
/// be circular. It also means the whole server is testable against a stub —
/// and it is, including the join handshake, the roster effects and expiry.
pub trait ControlHost: Send + Sync + 'static {
    /// The current join code, or `None` if none has been minted.
    fn join_code(&self) -> Option<JoinCode>;

    /// Control protocol version this desktop speaks.
    fn control_protocol_version(&self) -> String;

    fn service_version(&self) -> String;

    /// How many receivers are permitted at once.
    fn max_receivers(&self) -> u16;

    /// Whether this device's authorisation has been withdrawn.
    fn is_revoked(&self, receiver_id: &str) -> bool;

    /// Audio parameters for the `welcome`, or `None` if no stream is running.
    ///
    /// `None` is normal: a receiver may join before the operator starts the
    /// stream. It is told the parameters when the stream starts.
    fn stream_params(&self) -> Option<Value>;

    /// A receiver was admitted. The host adds it to the roster and starts
    /// sending audio to it.
    fn on_joined(&self, admission: &Admission);

    /// A receiver's connection ended.
    fn on_left(&self, receiver_id: &str);

    /// A heartbeat arrived.
    fn on_heartbeat(&self, receiver_id: &str, sent_at: i64);

    /// A telemetry report arrived. Everything in it is the receiver's claim.
    fn on_telemetry(&self, receiver_id: &str, report: &Value);

    /// Receivers that have not been heard from within `timeout_ms`.
    ///
    /// Asked of the host rather than tracked here because the roster already
    /// holds the heartbeat times and is what the operator sees; two copies of
    /// "when did we last hear from this phone" would disagree.
    fn expire_stale(&self, now_ms: i64, timeout_ms: i64) -> Vec<String>;

    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// Listener configuration.
#[derive(Debug, Clone)]
pub struct ControlServerConfig {
    pub bind_address: String,
    pub port: u16,
}

/// Something that happened, for the service to log or push to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Listening { address: SocketAddr },
    Joined { receiver_id: String, address: SocketAddr },
    Refused { address: SocketAddr, reason: String, code: &'static str },
    Left { receiver_id: String },
    Expired { receiver_id: String },
}

/// The control server.
pub struct ControlServer {
    sessions: Arc<ReceiverSessions>,
    local_address: SocketAddr,
    shutdown: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    events: mpsc::UnboundedReceiver<ServerEvent>,
}

impl ControlServer {
    /// Binds the listener and starts serving.
    ///
    /// Binding happens before this returns, so a port already in use is an
    /// error the operator sees immediately rather than a server that silently
    /// never accepts anybody.
    pub async fn start<H: ControlHost>(
        config: ControlServerConfig,
        host: Arc<H>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind((config.bind_address.as_str(), config.port)).await?;
        let local_address = listener.local_addr()?;

        let sessions = Arc::new(ReceiverSessions::new());
        let (event_tx, events) = mpsc::unbounded_channel();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        let _ = event_tx.send(ServerEvent::Listening { address: local_address });
        info!(%local_address, "receiver control server listening");

        {
            let sessions = Arc::clone(&sessions);
            let host = Arc::clone(&host);
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accepted = listener.accept() => match accepted {
                            Ok((stream, address)) => {
                                let sessions = Arc::clone(&sessions);
                                let host = Arc::clone(&host);
                                let event_tx = event_tx.clone();
                                // One task per connection. A receiver with a
                                // bad network must not hold up anyone else's
                                // handshake.
                                tokio::spawn(async move {
                                    serve(stream, address, sessions, host, event_tx).await;
                                });
                            }
                            Err(error) => {
                                // Accept failures are per-connection, not
                                // fatal: a listener that gave up here would
                                // take the whole session's receivers with it.
                                warn!(%error, "accept failed");
                            }
                        },
                    }
                }
                debug!("control server stopped accepting");
            });
        }

        // Expiry sweep. Separate from the per-connection tasks because the
        // case it exists for is a receiver whose task is blocked waiting for
        // bytes that will never arrive — a phone that went into a tunnel does
        // not close its socket, it simply stops. Without this it would hold a
        // roster slot and keep receiving audio into nothing.
        {
            let sessions = Arc::downgrade(&sessions);
            let host = Arc::clone(&host);
            let event_tx = event_tx.clone();
            let timeout = HEARTBEAT_INTERVAL_MS * i64::from(HEARTBEAT_MISS_LIMIT);
            tokio::spawn(async move {
                let mut ticker =
                    tokio::time::interval(std::time::Duration::from_millis(EXPIRY_SWEEP_MS));
                loop {
                    ticker.tick().await;
                    // A weak reference, so the sweep ends when the server is
                    // dropped rather than keeping it alive for ever.
                    let Some(sessions) = sessions.upgrade() else { break };
                    for receiver_id in host.expire_stale(host.now_ms(), timeout) {
                        if sessions.remove(&receiver_id).is_some() {
                            info!(receiver = %receiver_id, "receiver timed out");
                        }
                        host.on_left(&receiver_id);
                        let _ = event_tx.send(ServerEvent::Expired { receiver_id });
                    }
                }
                debug!("expiry sweep stopped");
            });
        }

        Ok(Self {
            sessions,
            local_address,
            shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
            events,
        })
    }

    #[must_use]
    pub const fn local_address(&self) -> SocketAddr {
        self.local_address
    }

    #[must_use]
    pub fn sessions(&self) -> Arc<ReceiverSessions> {
        Arc::clone(&self.sessions)
    }

    /// The next thing that happened, if anything has.
    pub fn try_next_event(&mut self) -> Option<ServerEvent> {
        self.events.try_recv().ok()
    }

    /// Waits for the next event.
    pub async fn next_event(&mut self) -> Option<ServerEvent> {
        self.events.recv().await
    }

    /// Disconnects one receiver.
    pub fn disconnect(&self, receiver_id: &str) {
        if let Some(handle) = self.sessions.remove(receiver_id) {
            // Nothing is sent first: the receiver finds out because its socket
            // closes, and a "you are being disconnected" message it might not
            // read is not worth the round trip.
            drop(handle);
        }
    }

    /// Stops accepting and closes every connection.
    pub fn shutdown(&self) {
        if let Some(signal) = self.shutdown.lock().take() {
            let _ = signal.send(());
        }
        for id in self.sessions.ids() {
            self.sessions.remove(&id);
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Serves one connection: handshake, then the message loop.
async fn serve<H: ControlHost>(
    stream: TcpStream,
    address: SocketAddr,
    sessions: Arc<ReceiverSessions>,
    host: Arc<H>,
    events: mpsc::UnboundedSender<ServerEvent>,
) {
    // Nagle off: control messages are small and latency matters more than
    // packing them. A heartbeat delayed by 40 ms of Nagle is a heartbeat
    // closer to being missed.
    let _ = stream.set_nodelay(true);

    let websocket = match tokio_tungstenite::accept_async(stream).await {
        Ok(websocket) => websocket,
        Err(error) => {
            debug!(%address, %error, "websocket handshake failed");
            return;
        }
    };

    use futures_util::StreamExt;
    let (mut writer, mut reader) = websocket.split();

    // -- handshake ---------------------------------------------------------
    let first = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reader.next(),
    )
    .await
    {
        Ok(Some(Ok(message))) => message,
        // A connection that opens and says nothing is either a port scanner
        // or a broken client. Either way it does not get to hold a slot.
        _ => {
            debug!(%address, "no hello within the handshake timeout");
            return;
        }
    };

    let Some(line) = text_of(&first) else {
        debug!(%address, "the first message was not text");
        return;
    };
    if line.len() > MAX_MESSAGE_BYTES {
        return;
    }

    let envelope = match Envelope::decode(&line) {
        Ok(envelope) => envelope,
        Err(error) => {
            debug!(%address, %error, "the hello did not parse");
            return;
        }
    };

    let EnvelopeKind::Hello {
        token,
        client,
        control_protocol_version,
        receiver,
    } = envelope.kind
    else {
        let _ = events.send(ServerEvent::Refused {
            address,
            reason: JoinError::NotAHello.to_string(),
            code: JoinError::NotAHello.code(),
        });
        return;
    };

    let ours = host.control_protocol_version();
    let admission = join::admit(
        &JoinRequest {
            token: &token,
            control_protocol_version: &control_protocol_version,
            receiver: receiver.as_ref(),
            address,
            now_ms: host.now_ms(),
        },
        host.join_code().as_ref(),
        &ours,
        sessions.count(),
        host.max_receivers(),
        |id| host.is_revoked(id),
    );

    let admission = match admission {
        Ok(admission) => admission,
        Err(error) => {
            // Logged with the reason for the operator; the receiver is told
            // nothing beyond its socket closing. See the open item in
            // docs/protocol/pairing.md.
            info!(%address, %client, reason = %error, "receiver refused");
            let _ = events.send(ServerEvent::Refused {
                address,
                reason: error.to_string(),
                code: error.code(),
            });
            return;
        }
    };

    let receiver_id = admission.receiver_id.clone();
    let (handle, mut outbound, displaced) = sessions.insert(&receiver_id, address);
    if displaced.is_some() {
        debug!(receiver = %receiver_id, "replaced an earlier connection from the same device");
    }

    let welcome = Envelope::new(EnvelopeKind::Welcome {
        service_version: host.service_version(),
        control_protocol_version: ours,
        audio_protocol_version: audio_protocol::AUDIO_PROTOCOL_VERSION,
        available_audio_apis: vec!["wasapi".to_owned()],
        pid: std::process::id(),
        stream: None,
        key_fingerprint: None,
    });
    let welcome = match with_stream_params(welcome, host.stream_params()) {
        Ok(envelope) => envelope,
        Err(()) => return,
    };

    if send(&mut writer, &welcome).await.is_err() {
        sessions.remove_if_current(&handle);
        return;
    }

    host.on_joined(&admission);
    info!(receiver = %receiver_id, %address, %client, "receiver joined");
    let _ = events.send(ServerEvent::Joined {
        receiver_id: receiver_id.clone(),
        address,
    });

    // -- message loop ------------------------------------------------------
    loop {
        tokio::select! {
            outgoing = outbound.recv() => match outgoing {
                Some(envelope) => {
                    if send(&mut writer, &envelope).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = reader.next() => match incoming {
                Some(Ok(message)) => {
                    if message.is_close() {
                        break;
                    }
                    let Some(line) = text_of(&message) else { continue };
                    if line.len() > MAX_MESSAGE_BYTES {
                        continue;
                    }
                    if let Some(reply) = handle_message(&line, &receiver_id, &host) {
                        if send(&mut writer, &reply).await.is_err() {
                            break;
                        }
                    }
                }
                // A read error or a closed socket. Either way this receiver
                // is gone; its roster entry is removed below.
                _ => break,
            },
        }
    }

    sessions.remove_if_current(&handle);
    host.on_left(&receiver_id);
    info!(receiver = %receiver_id, "receiver left");
    let _ = events.send(ServerEvent::Left { receiver_id });
}

/// Adds the audio parameters to a `welcome`, if a stream is running.
fn with_stream_params(envelope: Envelope, params: Option<Value>) -> Result<Envelope, ()> {
    let Some(params) = params else {
        return Ok(envelope);
    };
    let EnvelopeKind::Welcome {
        service_version,
        control_protocol_version,
        audio_protocol_version,
        available_audio_apis,
        pid,
        key_fingerprint,
        ..
    } = envelope.kind
    else {
        return Err(());
    };
    let stream = serde_json::from_value(params).map_err(|error| {
        warn!(%error, "the stream parameters did not match the schema");
    })?;
    Ok(Envelope::new(EnvelopeKind::Welcome {
        service_version,
        control_protocol_version,
        audio_protocol_version,
        available_audio_apis,
        pid,
        stream: Some(stream),
        key_fingerprint,
    }))
}

/// Handles one message from a receiver. Returns a reply, if one is owed.
fn handle_message<H: ControlHost>(
    line: &str,
    receiver_id: &str,
    host: &Arc<H>,
) -> Option<Envelope> {
    let envelope = match Envelope::decode(line) {
        Ok(envelope) => envelope,
        Err(error) => {
            // One malformed line does not end the session: a receiver on a
            // shared network sees stray traffic, and a performer's monitor
            // must not depend on nobody else using the port.
            debug!(receiver = %receiver_id, %error, "discarding a malformed message");
            return None;
        }
    };

    match envelope.kind {
        EnvelopeKind::Request { id, method, params } => match method.as_str() {
            "receiver.heartbeat" => {
                let sent_at = params.get("sentAt").and_then(Value::as_i64).unwrap_or(0);
                host.on_heartbeat(receiver_id, sent_at);
                Some(Envelope::ok_response(
                    id,
                    json!({
                        "at": host.now_ms(),
                        // Echoed so the receiver gets its round-trip time
                        // without any bookkeeping of its own.
                        "sentAt": sent_at,
                        "missLimit": HEARTBEAT_MISS_LIMIT,
                    }),
                ))
            }
            "receiver.telemetry" => {
                host.on_telemetry(receiver_id, &params);
                Some(Envelope::ok_response(id, json!({ "accepted": true })))
            }
            // A receiver may only call the two methods above. Everything else
            // in the registry is the *operator's*: a phone must not be able to
            // revoke another device, stop the stream or read the settings.
            other => Some(Envelope::error_response(
                id,
                ServiceErrorPayload::new(
                    "UNAUTHORIZED",
                    format!("a receiver may not call {other}"),
                )
                .with_detail(
                    "Receivers may call receiver.heartbeat and receiver.telemetry only.",
                ),
            )),
        },
        // A second hello on an authenticated connection, or a response or
        // event travelling the wrong way. Ignored rather than answered.
        _ => None,
    }
}

async fn send<S>(writer: &mut S, envelope: &Envelope) -> Result<(), ()>
where
    S: futures_util::SinkExt<Message> + Unpin,
{
    let line = envelope.encode().map_err(|_| ())?;
    // The envelope's own terminator is not needed: WebSocket frames the
    // message, so a trailing newline would just be a byte every receiver has
    // to trim.
    writer
        .send(Message::text(line.trim_end().to_owned()))
        .await
        .map_err(|_| ())
}

fn text_of(message: &Message) -> Option<String> {
    match message {
        Message::Text(text) => Some(text.to_string()),
        // Binary is accepted as UTF-8 so a client using a binary frame for
        // JSON is not mysteriously ignored.
        Message::Binary(bytes) => String::from_utf8(bytes.to_vec()).ok(),
        _ => None,
    }
}

/// Builds the `receiver` block for a hello. Used by tests and by the mobile
/// client; kept here so both sides agree on the shape.
#[must_use]
pub fn receiver_identity(
    device_name: &str,
    platform: &str,
    app_version: &str,
    public_key: &str,
) -> ReceiverIdentity {
    ReceiverIdentity {
        device_name: device_name.to_owned(),
        platform: platform.to_owned(),
        platform_version: None,
        app_version: Some(app_version.to_owned()),
        public_key: public_key.to_owned(),
        audio_port: None,
    }
}

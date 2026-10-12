//! The named-pipe control server.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::codec::{Envelope, EnvelopeKind, MAX_ENVELOPE_BYTES, ServiceErrorPayload};
use crate::error::{IpcError, Result};
use crate::handler::{ControlHandler, EventSink};
use crate::security::PipeSecurity;
use crate::{constant_time_eq, pipe_path};

/// Depth of the per-client event queue.
///
/// Sized for about two seconds of metering at 25 Hz plus headroom. Beyond that
/// the client is clearly not reading, and dropping is the right answer.
const EVENT_QUEUE_DEPTH: usize = 128;

/// How long a connected client has to send its `hello` before being dropped.
const HANDSHAKE_TIMEOUT_SECS: u64 = 10;

/// Upper bound on simultaneously existing pipe instances. See `create_pipe`.
const MAX_PIPE_INSTANCES: usize = 4;

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Random instance id; becomes part of the pipe name.
    pub instance_id: String,
    /// Shared secret the client must present. Passed to Electron at spawn.
    pub token: String,
}

impl ServerConfig {
    /// Generates a fresh instance id and token.
    #[must_use]
    pub fn generate() -> Self {
        Self { instance_id: crate::random_hex(), token: crate::random_hex() }
    }

    #[must_use]
    pub fn pipe_path(&self) -> String {
        pipe_path(&self.instance_id)
    }
}

/// Accepts control clients, one at a time, for the service's lifetime.
pub struct IpcServer<H: ControlHandler> {
    config: ServerConfig,
    handler: Arc<H>,
    security: PipeSecurity,
}

impl<H: ControlHandler> IpcServer<H> {
    pub fn new(config: ServerConfig, handler: Arc<H>) -> Result<Self> {
        let security = PipeSecurity::current_user_only()?;
        Ok(Self { config, handler, security })
    }

    #[must_use]
    pub fn pipe_path(&self) -> String {
        self.config.pipe_path()
    }

    /// Creates a pipe instance restricted to the current user.
    fn create_pipe(&self, first: bool) -> Result<NamedPipeServer> {
        let path = self.config.pipe_path();
        // SAFETY: `security` outlives this call and points at a valid
        // SECURITY_ATTRIBUTES whose descriptor it owns.
        let server = unsafe {
            ServerOptions::new()
                // Refuses to create a second instance if something already
                // holds this name — a squatting attempt fails loudly rather
                // than silently sharing the name with us.
                .first_pipe_instance(first)
                // Nothing off-machine has any business here.
                .reject_remote_clients(true)
                // This bounds pipe *instances*, not concurrency: clients are
                // still served strictly one at a time. The slack covers
                // lifecycle overlap — the instance being served, the one
                // already listening, and instances the kernel has not finished
                // tearing down after a client went away. Too tight a bound
                // surfaces as ERROR_PIPE_BUSY during an Electron restart.
                .max_instances(MAX_PIPE_INSTANCES)
                .create_with_security_attributes_raw(&path, self.security.as_raw())
        }?;
        Ok(server)
    }

    /// Runs until `shutdown` resolves.
    ///
    /// Client-scoped failures (a disconnect, a bad token, junk on the wire) are
    /// logged and the server goes back to listening. The audio service keeps
    /// running throughout — losing the UI must never stop the monitor feed.
    pub async fn run(self, mut shutdown: tokio::sync::oneshot::Receiver<()>) -> Result<()> {
        info!(pipe = %self.config.pipe_path(), "control channel listening");

        let mut listener = self.create_pipe(true)?;

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    debug!("control channel shutting down");
                    return Ok(());
                }
                accepted = listener.connect() => {
                    accepted?;
                }
            }

            let connected = listener;
            // Stand up the next instance *before* serving this client. Creating
            // it afterwards leaves a window in which the pipe name does not
            // exist, and the kernel also reports ERROR_PIPE_BUSY if the
            // previous instance has not finished tearing down. An Electron
            // restart would then fail to reconnect to a perfectly healthy
            // audio service.
            listener = self.create_pipe(false)?;

            debug!("control client connected");
            match self.serve_client(connected).await {
                Ok(()) => debug!("control client disconnected cleanly"),
                Err(error) if error.is_client_scoped() => {
                    warn!(%error, "control client dropped");
                }
                Err(error) => return Err(error),
            }
            self.handler.on_client_disconnected();
        }
    }

    /// Handshake, then concurrent request handling and event delivery.
    async fn serve_client(&self, pipe: NamedPipeServer) -> Result<()> {
        let (reader, mut writer) = tokio::io::split(pipe);
        let mut lines = BufReader::new(reader).lines();

        // -- handshake -----------------------------------------------------
        let hello = tokio::time::timeout(
            core::time::Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
            lines.next_line(),
        )
        .await
        .map_err(|_| IpcError::Handshake("client did not send a hello in time"))?
        .map_err(IpcError::Io)?
        .ok_or(IpcError::Disconnected)?;

        if hello.len() > MAX_ENVELOPE_BYTES {
            return Err(IpcError::EnvelopeTooLarge { limit: MAX_ENVELOPE_BYTES });
        }

        let welcome = self.authenticate(&hello)?;
        writer.write_all(welcome.encode()?.as_bytes()).await?;
        writer.flush().await?;
        info!("control client authenticated");

        // -- serve ---------------------------------------------------------
        let (event_tx, mut event_rx) = mpsc::channel::<Envelope>(EVENT_QUEUE_DEPTH);
        let sink = EventSink::new(event_tx);
        self.handler.on_client_connected(sink.clone());

        loop {
            tokio::select! {
                // Events are drained first so a busy request stream cannot
                // starve metering updates.
                biased;

                outgoing = event_rx.recv() => {
                    match outgoing {
                        Some(envelope) => {
                            writer.write_all(envelope.encode()?.as_bytes()).await?;
                            writer.flush().await?;
                        }
                        // Every sink was dropped; the service no longer wants
                        // to push to this client.
                        None => return Ok(()),
                    }
                }

                incoming = lines.next_line() => {
                    let Some(line) = incoming? else {
                        return Ok(());
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Some(response) = self.dispatch(&line).await {
                        writer.write_all(response.encode()?.as_bytes()).await?;
                        writer.flush().await?;
                    }
                }
            }
        }
    }

    /// Validates the `hello` and builds the `welcome`.
    fn authenticate(&self, line: &str) -> Result<Envelope> {
        let envelope = Envelope::decode(line)?;
        let EnvelopeKind::Hello { token, client, control_protocol_version, receiver } =
            envelope.kind
        else {
            return Err(IpcError::Handshake("first message was not a hello"));
        };

        // `receiver` belongs to the join handshake on the control server. This
        // pipe sets `reject_remote_clients(true)`, so a receiver can never
        // reach it; a hello claiming to be one is confused or hostile either
        // way, and admitting it would leave a phantom in the roster.
        if receiver.is_some() {
            return Err(IpcError::Handshake(
                "the local control pipe does not admit receivers",
            ));
        }

        // Constant-time, so a wrong token leaks nothing about the right one.
        if !constant_time_eq(&token, &self.config.token) {
            return Err(IpcError::Handshake("handshake token rejected"));
        }

        let ours = self.handler.control_protocol_version();
        if !major_versions_match(&control_protocol_version, &ours) {
            return Err(IpcError::ProtocolMismatch { peer: control_protocol_version, ours });
        }

        debug!(%client, "handshake accepted");
        Ok(Envelope::new(EnvelopeKind::Welcome {
            service_version: self.handler.service_version(),
            control_protocol_version: ours,
            audio_protocol_version: self.handler.audio_protocol_version(),
            available_audio_apis: self.handler.available_audio_apis(),
            pid: std::process::id(),
            // Both are for admitted receivers on the control server. The local
            // client drives the stream, so it has no use for either.
            stream: None,
            key_fingerprint: None,
        }))
    }

    /// Routes one request. Returns `None` for messages that need no reply.
    async fn dispatch(&self, line: &str) -> Option<Envelope> {
        let envelope = match Envelope::decode(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                // No correlation id is available, so this cannot be answered as
                // a response. Log and carry on rather than dropping the client:
                // one malformed line should not end a live session's control
                // channel.
                warn!(%error, "discarding a malformed control message");
                return None;
            }
        };

        match envelope.kind {
            EnvelopeKind::Request { id, method, params } => {
                match self.handler.handle(&method, params).await {
                    Ok(result) => Some(Envelope::ok_response(id, result)),
                    Err(error) => Some(Envelope::error_response(id, error)),
                }
            }
            EnvelopeKind::Hello { .. } => {
                warn!("ignoring a second hello on an authenticated connection");
                None
            }
            // Responses and events only travel service -> client.
            EnvelopeKind::Response { id, .. } => Some(Envelope::error_response(
                id,
                ServiceErrorPayload::new(
                    "INVALID_REQUEST",
                    "the service does not accept response envelopes",
                ),
            )),
            EnvelopeKind::Event { event, .. } => {
                warn!(%event, "ignoring an event sent by the client");
                None
            }
            EnvelopeKind::Welcome { .. } => None,
        }
    }
}

/// Whether two semantic versions share a major, tolerating a newer peer minor.
#[must_use]
pub fn major_versions_match(peer: &str, ours: &str) -> bool {
    fn major(value: &str) -> Option<u32> {
        value.split('.').next()?.trim().parse().ok()
    }
    match (major(peer), major(ours)) {
        (Some(peer_major), Some(our_major)) => peer_major == our_major,
        // An unparseable version is refused rather than assumed compatible.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ReceiverIdentity;
    use serde_json::{Value, json};
    use tokio::io::AsyncBufReadExt;
    use tokio::net::windows::named_pipe::ClientOptions;

    struct StubHandler;

    impl ControlHandler for StubHandler {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> core::result::Result<Value, ServiceErrorPayload> {
            match method {
                "service.ping" => Ok(json!({ "at": 1, "uptimeMs": 2 })),
                "echo" => Ok(params),
                "boom" => Err(ServiceErrorPayload::new("INTERNAL", "deliberate failure")),
                other => Err(ServiceErrorPayload::new(
                    "UNKNOWN_METHOD",
                    format!("no such method: {other}"),
                )),
            }
        }
        fn service_version(&self) -> String {
            "0.1.0-test".into()
        }
        fn control_protocol_version(&self) -> String {
            "1.0.0".into()
        }
        fn audio_protocol_version(&self) -> u8 {
            1
        }
        fn available_audio_apis(&self) -> Vec<String> {
            vec!["wasapi".into()]
        }
    }

    /// Spawns a server and returns its config plus a shutdown handle.
    fn spawn_server() -> (ServerConfig, tokio::sync::oneshot::Sender<()>) {
        let config = ServerConfig::generate();
        let server = IpcServer::new(config.clone(), Arc::new(StubHandler)).unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if let Err(error) = server.run(shutdown_rx).await {
                eprintln!("SERVER RUN FAILED: {error:?}");
            }
        });
        (config, shutdown_tx)
    }

    /// Connects, retrying while the server creates its first pipe instance.
    async fn connect(config: &ServerConfig) -> tokio::net::windows::named_pipe::NamedPipeClient {
        let path = config.pipe_path();
        for _ in 0..100 {
            match ClientOptions::new().open(&path) {
                Ok(client) => return client,
                Err(_) => tokio::time::sleep(core::time::Duration::from_millis(20)).await,
            }
        }
        panic!("could not connect to {path}: {:?}", ClientOptions::new().open(&path).err());
    }

    fn hello(token: &str, version: &str) -> String {
        hello_from(token, version, None)
    }

    fn hello_from(token: &str, version: &str, receiver: Option<ReceiverIdentity>) -> String {
        Envelope::new(EnvelopeKind::Hello {
            token: token.into(),
            client: "test/0.1.0".into(),
            control_protocol_version: version.into(),
            receiver,
        })
        .encode()
        .unwrap()
    }

    #[tokio::test]
    async fn a_valid_handshake_is_welcomed_and_requests_are_answered() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(hello(&config.token, "1.0.0").as_bytes())
            .await
            .unwrap();

        let welcome = Envelope::decode(&lines.next_line().await.unwrap().unwrap()).unwrap();
        match welcome.kind {
            EnvelopeKind::Welcome { service_version, audio_protocol_version, .. } => {
                assert_eq!(service_version, "0.1.0-test");
                assert_eq!(audio_protocol_version, 1);
            }
            other => panic!("expected a welcome, got {other:?}"),
        }

        writer
            .write_all(
                Envelope::new(EnvelopeKind::Request {
                    id: "r1".into(),
                    method: "service.ping".into(),
                    params: json!({}),
                })
                .encode()
                .unwrap()
                .as_bytes(),
            )
            .await
            .unwrap();

        let response = Envelope::decode(&lines.next_line().await.unwrap().unwrap()).unwrap();
        match response.kind {
            EnvelopeKind::Response { id, ok, result, .. } => {
                assert_eq!(id, "r1");
                assert!(ok);
                assert_eq!(result.unwrap()["uptimeMs"], 2);
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_wrong_token_is_refused_and_the_connection_is_dropped() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(hello("0000000000000000000000000000beef", "1.0.0").as_bytes())
            .await
            .unwrap();

        // The server must close without welcoming us.
        assert!(
            lines.next_line().await.unwrap_or(None).is_none(),
            "a rejected client must not receive a welcome"
        );
    }

    /// Even holding the right token, a hello that claims to be a receiver is
    /// refused here: receivers join on the control server, and letting one in
    /// through the local pipe would put a device in the roster that no
    /// network-facing code ever admitted.
    #[tokio::test]
    async fn a_receiver_hello_is_refused_on_the_local_pipe() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        let receiver = ReceiverIdentity {
            device_name: "Pixel 8".into(),
            platform: "android".into(),
            platform_version: Some("14".into()),
            app_version: Some("0.1.0".into()),
            public_key: "aa".repeat(32),
        };
        writer
            .write_all(hello_from(&config.token, "1.0.0", Some(receiver)).as_bytes())
            .await
            .unwrap();

        assert!(
            lines.next_line().await.unwrap_or(None).is_none(),
            "a receiver must not be welcomed onto the local pipe"
        );
    }

    #[tokio::test]
    async fn an_incompatible_protocol_major_is_refused() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(hello(&config.token, "2.0.0").as_bytes())
            .await
            .unwrap();

        assert!(lines.next_line().await.unwrap_or(None).is_none());
    }

    #[tokio::test]
    async fn a_request_before_the_handshake_is_refused() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(
                Envelope::new(EnvelopeKind::Request {
                    id: "r1".into(),
                    method: "service.ping".into(),
                    params: json!({}),
                })
                .encode()
                .unwrap()
                .as_bytes(),
            )
            .await
            .unwrap();

        assert!(
            lines.next_line().await.unwrap_or(None).is_none(),
            "an unauthenticated request must not be served"
        );
    }

    #[tokio::test]
    async fn a_handler_error_becomes_a_structured_error_response() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(hello(&config.token, "1.0.0").as_bytes())
            .await
            .unwrap();
        lines.next_line().await.unwrap().unwrap(); // welcome

        writer
            .write_all(
                Envelope::new(EnvelopeKind::Request {
                    id: "r2".into(),
                    method: "boom".into(),
                    params: json!({}),
                })
                .encode()
                .unwrap()
                .as_bytes(),
            )
            .await
            .unwrap();

        let response = Envelope::decode(&lines.next_line().await.unwrap().unwrap()).unwrap();
        match response.kind {
            EnvelopeKind::Response { ok, error: Some(error), .. } => {
                assert!(!ok);
                assert_eq!(error.code, "INTERNAL");
            }
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_malformed_line_does_not_end_the_session() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(hello(&config.token, "1.0.0").as_bytes())
            .await
            .unwrap();
        lines.next_line().await.unwrap().unwrap(); // welcome

        // Junk, then a valid request. The junk must be discarded and the
        // request still answered: a live service must not lose its control
        // channel over one bad line.
        writer.write_all(b"{ not json at all\n").await.unwrap();
        writer
            .write_all(
                Envelope::new(EnvelopeKind::Request {
                    id: "r3".into(),
                    method: "echo".into(),
                    params: json!({ "ok": true }),
                })
                .encode()
                .unwrap()
                .as_bytes(),
            )
            .await
            .unwrap();

        let response = Envelope::decode(&lines.next_line().await.unwrap().unwrap()).unwrap();
        match response.kind {
            EnvelopeKind::Response { id, result, .. } => {
                assert_eq!(id, "r3");
                assert_eq!(result.unwrap()["ok"], true);
            }
            other => panic!("expected the echo response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_server_accepts_a_second_client_after_the_first_leaves() {
        let (config, _shutdown) = spawn_server();

        // First client connects, authenticates, then goes away — as Electron
        // does when it restarts.
        {
            let client = connect(&config).await;
            let (reader, mut writer) = tokio::io::split(client);
            let mut lines = BufReader::new(reader).lines();
            writer
                .write_all(hello(&config.token, "1.0.0").as_bytes())
                .await
                .unwrap();
            lines.next_line().await.unwrap().unwrap();
        }

        // The service is still running, so a fresh client must be served.
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();
        writer
            .write_all(hello(&config.token, "1.0.0").as_bytes())
            .await
            .unwrap();
        let welcome = lines.next_line().await.unwrap().unwrap();
        assert!(welcome.contains("welcome"), "got {welcome}");
    }

    #[tokio::test]
    async fn unknown_methods_are_reported_rather_than_executed() {
        let (config, _shutdown) = spawn_server();
        let client = connect(&config).await;
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(hello(&config.token, "1.0.0").as_bytes())
            .await
            .unwrap();
        lines.next_line().await.unwrap().unwrap();

        writer
            .write_all(
                Envelope::new(EnvelopeKind::Request {
                    id: "r4".into(),
                    method: "system.exec".into(),
                    params: json!({ "cmd": "calc.exe" }),
                })
                .encode()
                .unwrap()
                .as_bytes(),
            )
            .await
            .unwrap();

        let response = Envelope::decode(&lines.next_line().await.unwrap().unwrap()).unwrap();
        match response.kind {
            EnvelopeKind::Response { ok, error: Some(error), .. } => {
                assert!(!ok);
                assert_eq!(error.code, "UNKNOWN_METHOD");
            }
            other => panic!("expected UNKNOWN_METHOD, got {other:?}"),
        }
    }

    #[test]
    fn version_matching_requires_the_same_major() {
        assert!(major_versions_match("1.0.0", "1.0.0"));
        assert!(major_versions_match("1.9.3", "1.0.0"), "a newer minor is fine");
        assert!(!major_versions_match("2.0.0", "1.0.0"));
        assert!(!major_versions_match("", "1.0.0"));
        assert!(!major_versions_match("banana", "1.0.0"));
    }

    #[test]
    fn generated_configs_are_unique() {
        let first = ServerConfig::generate();
        let second = ServerConfig::generate();
        assert_ne!(first.token, second.token);
        assert_ne!(first.instance_id, second.instance_id);
        assert_ne!(first.pipe_path(), second.pipe_path());
    }
}

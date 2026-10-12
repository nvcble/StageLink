//! The seam between the transport and the service.
//!
//! [`ControlHandler`] is implemented by the service; the IPC server knows
//! nothing about audio, receivers or settings. That keeps the transport
//! testable in isolation (see the server's tests, which drive it with a stub
//! handler) and keeps protocol concerns out of the audio engine.

use serde_json::Value;

use crate::codec::{Envelope, ServiceErrorPayload};

/// Handles one control request.
///
/// Implementations must not block the async runtime for long. Anything that
/// touches hardware or the disk should be moved to a blocking task — the audio
/// path is already on its own OS thread and is unaffected either way, but a
/// blocked runtime would stall telemetry and make the UI appear frozen.
pub trait ControlHandler: Send + Sync + 'static {
    /// Dispatches `method` with `params`, returning the JSON result.
    fn handle(
        &self,
        method: &str,
        params: Value,
    ) -> impl core::future::Future<Output = core::result::Result<Value, ServiceErrorPayload>> + Send;

    /// Service version reported in the handshake.
    fn service_version(&self) -> String;

    /// Control protocol version this service speaks.
    fn control_protocol_version(&self) -> String;

    /// Audio packet protocol version.
    fn audio_protocol_version(&self) -> u8;

    /// Audio APIs actually usable at runtime.
    fn available_audio_apis(&self) -> Vec<String>;

    /// Called when a client connects and completes the handshake.
    ///
    /// The service uses this to push an immediate snapshot of every piece of
    /// state, so a reconnecting Electron does not have to poll.
    fn on_client_connected(&self, sink: EventSink) {
        let _ = sink;
    }

    /// Called when the client goes away. The service keeps running: the audio
    /// must not stop because a window reloaded.
    fn on_client_disconnected(&self) {}
}

/// Pushes events to the connected client.
///
/// Cloneable and cheap. Sending to a disconnected client is not an error — it
/// is the normal state between an Electron restart and the next handshake, so
/// `send` simply returns `false`.
#[derive(Clone)]
pub struct EventSink {
    sender: tokio::sync::mpsc::Sender<Envelope>,
}

impl EventSink {
    #[must_use]
    pub const fn new(sender: tokio::sync::mpsc::Sender<Envelope>) -> Self {
        Self { sender }
    }

    /// Queues an event. Returns false if the client is gone or its queue is
    /// full.
    ///
    /// A full queue drops the event rather than applying backpressure: the
    /// alternative is letting a slow UI stall the telemetry thread, and a
    /// dropped meter frame is invisible while a stalled service is not. Events
    /// that carry state the UI must not miss are also available as a request,
    /// so a dropped push is always recoverable.
    #[must_use]
    pub fn send(&self, envelope: Envelope) -> bool {
        self.sender.try_send(envelope).is_ok()
    }

    /// Whether a client is currently attached.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.sender.is_closed()
    }

    /// Free slots in the event queue, for diagnostics.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.sender.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn events_are_delivered_in_order() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        let sink = EventSink::new(sender);

        assert!(sink.is_connected());
        for index in 0..3 {
            assert!(sink.send(Envelope::event("audio.levels", json!({ "n": index }), index)));
        }

        for index in 0..3 {
            let envelope = receiver.recv().await.unwrap();
            match envelope.kind {
                crate::codec::EnvelopeKind::Event { at, .. } => assert_eq!(at, index),
                other => panic!("expected an event, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_full_queue_drops_rather_than_blocking() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(2);
        let sink = EventSink::new(sender);

        assert!(sink.send(Envelope::event("audio.levels", json!({}), 1)));
        assert!(sink.send(Envelope::event("audio.levels", json!({}), 2)));
        // Third send has nowhere to go; it must return false immediately rather
        // than await a slot.
        assert!(
            !sink.send(Envelope::event("audio.levels", json!({}), 3)),
            "a full event queue must drop, not block"
        );
    }

    #[tokio::test]
    async fn sending_to_a_departed_client_is_not_an_error() {
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let sink = EventSink::new(sender);
        drop(receiver);

        assert!(!sink.is_connected());
        assert!(!sink.send(Envelope::event("audio.levels", json!({}), 1)));
    }
}

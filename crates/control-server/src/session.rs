//! Connected receivers, and how the desktop talks back to them.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use desktop_ipc::codec::Envelope;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::debug;

/// How many envelopes may be queued to one receiver before the slow ones are
/// dropped.
///
/// Small on purpose. The control channel carries nothing a receiver needs
/// *historically* — every event is a snapshot of current state, so a dropped
/// one is superseded by the next. A deep queue would only let a stalled
/// receiver accumulate a backlog of stale truths and then act on them.
const OUTBOUND_QUEUE: usize = 32;

/// One connected receiver, from the desktop's side.
pub struct SessionHandle {
    pub receiver_id: String,
    /// Where the control connection came from. Its port is the OS-chosen
    /// source port of a TCP connection, never where audio should be sent.
    pub address: SocketAddr,
    /// The UDP port this receiver declared it is listening on, if it did.
    pub audio_port: Option<u16>,
    outbound: mpsc::Sender<Envelope>,
}

impl SessionHandle {
    /// Sends an envelope. Never blocks and never fails the caller.
    ///
    /// A full queue drops the message rather than waiting: the alternative is
    /// a stalled phone holding up the thread that serves everyone else, and
    /// the thing being sent is a snapshot that will be sent again.
    pub fn send(&self, envelope: Envelope) -> bool {
        match self.outbound.try_send(envelope) {
            Ok(()) => true,
            Err(_) => {
                debug!(receiver = %self.receiver_id, "dropped a control message; queue full");
                false
            }
        }
    }

    /// Sends an event.
    pub fn send_event(&self, event: &str, payload: Value, at: i64) -> bool {
        self.send(Envelope::event(event, payload, at))
    }
}

/// The set of connected receivers.
///
/// Separate from the roster in `receiver-session`: that one is the operator's
/// view and is persisted and reported, this one is the live socket bookkeeping
/// and disappears when the process does.
#[derive(Default)]
pub struct ReceiverSessions {
    inner: Mutex<HashMap<String, Arc<SessionHandle>>>,
}

impl ReceiverSessions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a connection, returning the handle and any it displaced.
    ///
    /// A device reconnecting replaces its previous session rather than being
    /// refused. Phones change networks, lose Wi-Fi in a pocket and come back;
    /// refusing the new connection because a dead one is still registered
    /// would mean a performer has to restart the app to get their monitor
    /// back.
    pub fn insert(
        &self,
        receiver_id: &str,
        address: SocketAddr,
        audio_port: Option<u16>,
    ) -> (Arc<SessionHandle>, mpsc::Receiver<Envelope>, Option<Arc<SessionHandle>>) {
        let (sender, receiver) = mpsc::channel(OUTBOUND_QUEUE);
        let handle = Arc::new(SessionHandle {
            receiver_id: receiver_id.to_owned(),
            address,
            audio_port,
            outbound: sender,
        });
        let displaced = self
            .inner
            .lock()
            .insert(receiver_id.to_owned(), Arc::clone(&handle));
        (handle, receiver, displaced)
    }

    /// Removes a connection, but only if it is still the registered one.
    ///
    /// The guard matters: a reconnect registers the new session before the old
    /// one notices it has been displaced, and an unguarded removal would then
    /// delete the *live* session when the dead one finally cleaned up.
    pub fn remove_if_current(&self, handle: &SessionHandle) {
        let mut sessions = self.inner.lock();
        if let Some(current) = sessions.get(&handle.receiver_id) {
            if Arc::as_ptr(current) == std::ptr::from_ref(handle) {
                sessions.remove(&handle.receiver_id);
            }
        }
    }

    pub fn remove(&self, receiver_id: &str) -> Option<Arc<SessionHandle>> {
        self.inner.lock().remove(receiver_id)
    }

    #[must_use]
    pub fn get(&self, receiver_id: &str) -> Option<Arc<SessionHandle>> {
        self.inner.lock().get(receiver_id).cloned()
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.inner.lock().len()
    }

    /// Every connected receiver, and where its audio has to be sent.
    ///
    /// This is what the sender needs when the operator presses Start with
    /// phones already joined — which is the normal order, since people connect
    /// while the band is still setting up. Taken from the live connections
    /// rather than the roster: the roster also holds devices that have gone,
    /// and it does not record the port each one declared.
    #[must_use]
    pub fn audio_destinations(&self, configured_stream_port: u16) -> Vec<(String, SocketAddr)> {
        self.inner
            .lock()
            .values()
            .map(|handle| {
                (
                    handle.receiver_id.clone(),
                    crate::join::audio_destination(
                        handle.address.ip(),
                        handle.audio_port,
                        configured_stream_port,
                    ),
                )
            })
            .collect()
    }

    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.inner.lock().keys().cloned().collect();
        ids.sort_unstable();
        ids
    }

    /// Sends an event to every connected receiver.
    pub fn broadcast(&self, event: &str, payload: &Value, at: i64) {
        let handles: Vec<Arc<SessionHandle>> = self.inner.lock().values().cloned().collect();
        for handle in handles {
            handle.send_event(event, payload.clone(), at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> SocketAddr {
        "10.0.0.5:51000".parse().unwrap()
    }

    #[test]
    fn a_registered_session_can_be_found_and_removed() {
        let sessions = ReceiverSessions::new();
        let (handle, _rx, displaced) = sessions.insert("r1", address(), None);
        assert!(displaced.is_none());
        assert_eq!(sessions.count(), 1);
        assert!(sessions.get("r1").is_some());

        sessions.remove_if_current(&handle);
        assert_eq!(sessions.count(), 0);
    }

    /// Phones lose Wi-Fi in a pocket and come back. Refusing the new
    /// connection because a dead one is still registered would mean restarting
    /// the app to get a monitor feed back.
    #[test]
    fn a_reconnect_displaces_the_previous_session_rather_than_being_refused() {
        let sessions = ReceiverSessions::new();
        let (first, _rx1, _) = sessions.insert("r1", address(), None);
        let (second, _rx2, displaced) = sessions.insert("r1", "10.0.0.6:52000".parse().unwrap(), None);

        assert!(displaced.is_some(), "the old session should be handed back");
        assert_eq!(sessions.count(), 1, "not two sessions for one device");
        assert_eq!(sessions.get("r1").unwrap().address, second.address);
        let _ = first;
    }

    /// The ordering hazard: the old connection's cleanup runs *after* the new
    /// one has registered, and an unguarded removal would delete the live
    /// session.
    #[test]
    fn a_displaced_session_cleaning_up_does_not_remove_the_live_one() {
        let sessions = ReceiverSessions::new();
        let (first, _rx1, _) = sessions.insert("r1", address(), None);
        let (_second, _rx2, _) = sessions.insert("r1", "10.0.0.6:52000".parse().unwrap(), None);

        // The old connection finally notices and cleans up.
        sessions.remove_if_current(&first);

        assert_eq!(
            sessions.count(),
            1,
            "the live session was removed by the dead one's cleanup"
        );
    }

    #[tokio::test]
    async fn an_event_reaches_the_receiver() {
        let sessions = ReceiverSessions::new();
        let (_handle, mut rx, _) = sessions.insert("r1", address(), None);

        sessions.broadcast("receivers.changed", &serde_json::json!({ "x": 1 }), 7);
        let envelope = rx.recv().await.expect("the event must arrive");
        match envelope.kind {
            desktop_ipc::codec::EnvelopeKind::Event { event, at, .. } => {
                assert_eq!(event, "receivers.changed");
                assert_eq!(at, 7);
            }
            other => panic!("expected an event, got {other:?}"),
        }
    }

    /// A stalled phone must not hold up the thread that serves everyone else,
    /// and the thing being dropped is a snapshot that will be sent again.
    #[tokio::test]
    async fn a_full_queue_drops_rather_than_blocking() {
        let sessions = ReceiverSessions::new();
        let (handle, _rx, _) = sessions.insert("r1", address(), None);

        let mut sent = 0;
        for index in 0..OUTBOUND_QUEUE * 4 {
            if handle.send_event("stream.status", serde_json::json!({ "i": index }), 0) {
                sent += 1;
            }
        }
        assert_eq!(sent, OUTBOUND_QUEUE, "the queue should bound at its capacity");
    }

    #[tokio::test]
    async fn sending_to_a_departed_receiver_is_not_an_error() {
        let sessions = ReceiverSessions::new();
        let (handle, rx, _) = sessions.insert("r1", address(), None);
        drop(rx);
        assert!(!handle.send_event("stream.status", Value::Null, 0));
    }

    #[test]
    fn ids_are_sorted_so_the_roster_does_not_reshuffle() {
        let sessions = ReceiverSessions::new();
        for id in ["r3", "r1", "r2"] {
            let (_h, _rx, _) = sessions.insert(id, address(), None);
            // Leak the handles deliberately: dropping them would not
            // unregister, but keeping them is what a live server does.
            core::mem::forget(_h);
            core::mem::forget(_rx);
        }
        assert_eq!(sessions.ids(), vec!["r1", "r2", "r3"]);
    }
}

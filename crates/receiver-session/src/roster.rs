//! The live receiver roster.
//!
//! Tracks which receivers are currently attached and what they have told us
//! about themselves. Two rules this module enforces:
//!
//! * **Desktop-measured and receiver-reported figures are kept apart.** The
//!   packet counts we maintain here are ours; anything inside `telemetry` came
//!   off the wire from a phone and is tagged as such.
//! * **An empty roster is empty.** There is no placeholder entry, no demo
//!   device and no simulated receiver. If nothing is connected, the UI shows
//!   its empty state.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Live connectivity of one receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Offline,
    Connecting,
    /// Control channel up, no audio.
    Connected,
    /// Control channel up and audio flowing.
    Streaming,
    /// Missed the heartbeat deadline.
    TimedOut,
}

/// Quality figures a receiver reported about itself.
///
/// Every field is the receiver's claim. The desktop cannot verify any of it,
/// which is why the type name and the protocol's `source: "receiver-reported"`
/// discriminator both say so.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverReportedTelemetry {
    /// Desktop clock time when we accepted the report.
    pub received_at: i64,
    /// Receiver's own clock time when it built the report.
    pub reported_at: i64,
    pub packets_received: u64,
    pub packets_lost: u64,
    pub loss_percent: f64,
    pub jitter_ms: f64,
    pub buffer_health_ms: f64,
    pub underruns: u64,
}

/// One roster entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterEntry {
    pub receiver_id: String,
    pub connection_state: ConnectionState,
    pub ip_address: Option<String>,
    pub joined_at: i64,
    pub last_heartbeat_at: Option<i64>,
    /// Desktop-measured: datagrams we handed to the socket for this receiver.
    pub packets_sent: u64,
    /// Desktop-measured: datagrams dropped because this receiver's queue was
    /// full. A slow receiver loses its own audio and nobody else's.
    pub packets_dropped_locally: u64,
    /// Receiver's own claims, if it has reported any.
    pub telemetry: Option<ReceiverReportedTelemetry>,
}

impl RosterEntry {
    #[must_use]
    pub fn new(receiver_id: impl Into<String>, ip_address: Option<String>, now_ms: i64) -> Self {
        Self {
            receiver_id: receiver_id.into(),
            connection_state: ConnectionState::Connecting,
            ip_address,
            joined_at: now_ms,
            last_heartbeat_at: None,
            packets_sent: 0,
            packets_dropped_locally: 0,
            telemetry: None,
        }
    }

    /// Whether this receiver should currently be sent audio.
    #[must_use]
    pub const fn should_receive_audio(&self) -> bool {
        matches!(
            self.connection_state,
            ConnectionState::Connected | ConnectionState::Streaming
        )
    }
}

/// The set of attached receivers.
#[derive(Debug, Default)]
pub struct ReceiverRoster {
    entries: HashMap<String, RosterEntry>,
    /// Receivers seen at any point this session, for the session total.
    seen_this_session: HashMap<String, ()>,
}

impl ReceiverRoster {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or refreshes a receiver. Returns true if this is a new arrival.
    pub fn join(&mut self, receiver_id: &str, ip_address: Option<String>, now_ms: i64) -> bool {
        let first_time = !self.seen_this_session.contains_key(receiver_id);
        self.seen_this_session.insert(receiver_id.to_owned(), ());

        self.entries
            .entry(receiver_id.to_owned())
            .and_modify(|entry| {
                entry.connection_state = ConnectionState::Connected;
                entry.ip_address = ip_address.clone();
                entry.last_heartbeat_at = Some(now_ms);
            })
            .or_insert_with(|| {
                let mut entry = RosterEntry::new(receiver_id, ip_address, now_ms);
                entry.connection_state = ConnectionState::Connected;
                entry.last_heartbeat_at = Some(now_ms);
                entry
            });

        first_time
    }

    /// Removes a receiver.
    pub fn leave(&mut self, receiver_id: &str) -> Option<RosterEntry> {
        self.entries.remove(receiver_id)
    }

    pub fn set_state(&mut self, receiver_id: &str, state: ConnectionState) {
        if let Some(entry) = self.entries.get_mut(receiver_id) {
            entry.connection_state = state;
        }
    }

    pub fn record_heartbeat(&mut self, receiver_id: &str, now_ms: i64) {
        if let Some(entry) = self.entries.get_mut(receiver_id) {
            entry.last_heartbeat_at = Some(now_ms);
            if entry.connection_state == ConnectionState::TimedOut {
                entry.connection_state = ConnectionState::Connected;
            }
        }
    }

    /// Stores a receiver's self-report.
    pub fn record_telemetry(&mut self, receiver_id: &str, telemetry: ReceiverReportedTelemetry) {
        if let Some(entry) = self.entries.get_mut(receiver_id) {
            entry.telemetry = Some(telemetry);
        }
    }

    /// Desktop-measured send accounting.
    pub fn record_send(&mut self, receiver_id: &str, sent: u64, dropped: u64) {
        if let Some(entry) = self.entries.get_mut(receiver_id) {
            entry.packets_sent = entry.packets_sent.saturating_add(sent);
            entry.packets_dropped_locally = entry.packets_dropped_locally.saturating_add(dropped);
        }
    }

    /// Marks receivers that have not been heard from as timed out, returning
    /// their ids.
    pub fn expire_stale(&mut self, now_ms: i64, timeout_ms: i64) -> Vec<String> {
        let mut expired = Vec::new();
        for entry in self.entries.values_mut() {
            if entry.connection_state == ConnectionState::TimedOut {
                continue;
            }
            let last = entry.last_heartbeat_at.unwrap_or(entry.joined_at);
            if now_ms - last > timeout_ms {
                entry.connection_state = ConnectionState::TimedOut;
                expired.push(entry.receiver_id.clone());
            }
        }
        expired
    }

    #[must_use]
    pub fn get(&self, receiver_id: &str) -> Option<&RosterEntry> {
        self.entries.get(receiver_id)
    }

    /// Every entry, ordered by id so the UI does not reshuffle on each refresh.
    #[must_use]
    pub fn entries(&self) -> Vec<RosterEntry> {
        let mut entries: Vec<_> = self.entries.values().cloned().collect();
        entries.sort_by(|a, b| a.receiver_id.cmp(&b.receiver_id));
        entries
    }

    /// Receivers that should be sent audio right now.
    #[must_use]
    pub fn audio_targets(&self) -> Vec<&RosterEntry> {
        self.entries
            .values()
            .filter(|entry| entry.should_receive_audio())
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Count of receivers currently connected or streaming.
    #[must_use]
    pub fn active_count(&self) -> u32 {
        self.entries
            .values()
            .filter(|entry| entry.should_receive_audio())
            .count() as u32
    }

    /// Distinct receivers seen at any point this session.
    #[must_use]
    pub fn total_seen(&self) -> u32 {
        self.seen_this_session.len() as u32
    }

    /// Clears everything. Called when a new session starts.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.seen_this_session.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telemetry(loss: f64) -> ReceiverReportedTelemetry {
        ReceiverReportedTelemetry {
            received_at: 100,
            reported_at: 99,
            packets_received: 1_000,
            packets_lost: 10,
            loss_percent: loss,
            jitter_ms: 2.5,
            buffer_health_ms: 40.0,
            underruns: 0,
        }
    }

    #[test]
    fn a_new_roster_is_genuinely_empty() {
        let roster = ReceiverRoster::new();
        assert!(roster.is_empty());
        assert_eq!(roster.len(), 0);
        assert_eq!(roster.active_count(), 0);
        assert!(
            roster.entries().is_empty(),
            "there must be no placeholder or demo receiver"
        );
    }

    #[test]
    fn joining_adds_a_connected_receiver() {
        let mut roster = ReceiverRoster::new();
        assert!(roster.join("r1", Some("10.0.0.5".into()), 100));
        let entry = roster.get("r1").unwrap();
        assert_eq!(entry.connection_state, ConnectionState::Connected);
        assert_eq!(entry.ip_address.as_deref(), Some("10.0.0.5"));
        assert_eq!(entry.joined_at, 100);
        assert!(entry.should_receive_audio());
    }

    #[test]
    fn rejoining_is_not_counted_as_a_new_receiver() {
        let mut roster = ReceiverRoster::new();
        assert!(roster.join("r1", None, 100), "first join is new");
        roster.leave("r1");
        assert!(
            !roster.join("r1", None, 200),
            "a reconnect in the same session is not a new receiver"
        );
        assert_eq!(roster.total_seen(), 1);
    }

    #[test]
    fn leaving_removes_the_entry_and_returns_it() {
        let mut roster = ReceiverRoster::new();
        roster.join("r1", None, 100);
        let removed = roster.leave("r1").unwrap();
        assert_eq!(removed.receiver_id, "r1");
        assert!(roster.is_empty());
        assert!(roster.leave("r1").is_none(), "leaving twice is harmless");
    }

    #[test]
    fn desktop_counters_and_receiver_claims_are_stored_separately() {
        let mut roster = ReceiverRoster::new();
        roster.join("r1", None, 100);
        roster.record_send("r1", 500, 3);
        roster.record_telemetry("r1", telemetry(1.0));

        let entry = roster.get("r1").unwrap();
        // Ours.
        assert_eq!(entry.packets_sent, 500);
        assert_eq!(entry.packets_dropped_locally, 3);
        // Theirs.
        assert_eq!(entry.telemetry.unwrap().packets_received, 1_000);
        assert!((entry.telemetry.unwrap().loss_percent - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn send_accounting_accumulates() {
        let mut roster = ReceiverRoster::new();
        roster.join("r1", None, 0);
        roster.record_send("r1", 10, 1);
        roster.record_send("r1", 5, 2);
        let entry = roster.get("r1").unwrap();
        assert_eq!(entry.packets_sent, 15);
        assert_eq!(entry.packets_dropped_locally, 3);
    }

    #[test]
    fn telemetry_for_an_unknown_receiver_is_ignored() {
        let mut roster = ReceiverRoster::new();
        roster.record_telemetry("ghost", telemetry(50.0));
        roster.record_send("ghost", 100, 0);
        assert!(
            roster.is_empty(),
            "an unsolicited report must not create a receiver"
        );
    }

    #[test]
    fn stale_receivers_time_out_once_and_recover_on_a_heartbeat() {
        let mut roster = ReceiverRoster::new();
        roster.join("r1", None, 1_000);
        roster.join("r2", None, 1_000);
        roster.record_heartbeat("r2", 5_000);

        let expired = roster.expire_stale(6_000, 2_000);
        assert_eq!(expired, vec!["r1".to_owned()]);
        assert_eq!(
            roster.get("r1").unwrap().connection_state,
            ConnectionState::TimedOut
        );
        assert!(!roster.get("r1").unwrap().should_receive_audio());

        // Already timed out, so it is not reported a second time.
        assert!(roster.expire_stale(7_000, 2_000).is_empty());

        // A heartbeat brings it back.
        roster.record_heartbeat("r1", 7_100);
        assert_eq!(
            roster.get("r1").unwrap().connection_state,
            ConnectionState::Connected
        );
    }

    #[test]
    fn audio_targets_exclude_timed_out_and_connecting_receivers() {
        let mut roster = ReceiverRoster::new();
        roster.join("connected", None, 0);
        roster.join("streaming", None, 0);
        roster.set_state("streaming", ConnectionState::Streaming);
        roster.join("stale", None, 0);
        roster.set_state("stale", ConnectionState::TimedOut);
        roster.join("joining", None, 0);
        roster.set_state("joining", ConnectionState::Connecting);

        let mut ids: Vec<_> = roster
            .audio_targets()
            .iter()
            .map(|entry| entry.receiver_id.clone())
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["connected".to_owned(), "streaming".to_owned()]);
        assert_eq!(roster.active_count(), 2);
    }

    #[test]
    fn entries_are_returned_in_a_stable_order() {
        let mut roster = ReceiverRoster::new();
        for id in ["zebra", "alpha", "mike"] {
            roster.join(id, None, 0);
        }
        let ids: Vec<_> = roster
            .entries()
            .iter()
            .map(|entry| entry.receiver_id.clone())
            .collect();
        assert_eq!(ids, vec!["alpha", "mike", "zebra"]);
    }

    #[test]
    fn clearing_resets_the_session_totals() {
        let mut roster = ReceiverRoster::new();
        roster.join("r1", None, 0);
        roster.join("r2", None, 0);
        assert_eq!(roster.total_seen(), 2);

        roster.clear();
        assert!(roster.is_empty());
        assert_eq!(roster.total_seen(), 0);
        assert!(roster.join("r1", None, 1), "after a clear, r1 is new again");
    }
}

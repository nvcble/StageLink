//! Transport abstraction and sequence tracking.
//!
//! The transport is an explicit, named choice rather than an implementation
//! detail, because "which transport am I on?" is a question with safety
//! consequences: a production build must refuse to send a monitor mix in the
//! clear over a venue network.

use serde::{Deserialize, Serialize};

use crate::packet::sequence_delta;

/// Wire transport for audio payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportKind {
    /// Unauthenticated, unencrypted UDP. Development and the headless test
    /// receiver only — see [`TransportKind::allowed_in_production`].
    UdpPlaintext,
    /// ChaCha20-Poly1305 over the payload with the header as associated data,
    /// using per-session keys derived during the paired control handshake.
    UdpEncrypted,
}

impl TransportKind {
    /// Whether a release build may use this transport.
    ///
    /// A production StageLink must never fall back to plaintext: the audio is a
    /// live feed of a church service or performance on a shared venue network.
    #[must_use]
    pub const fn allowed_in_production(self) -> bool {
        matches!(self, Self::UdpEncrypted)
    }

    #[must_use]
    pub const fn is_encrypted(self) -> bool {
        matches!(self, Self::UdpEncrypted)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UdpPlaintext => "udp-plaintext",
            Self::UdpEncrypted => "udp-encrypted",
        }
    }
}

/// What a tracker made of an arriving packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceVerdict {
    /// First packet of the stream.
    First,
    /// The expected next packet.
    InOrder,
    /// Arrived in order but `missing` packets were skipped.
    Gap { missing: u32 },
    /// Arrived after a later packet; `behind` packets late.
    Reordered { behind: u32 },
    /// Exact repeat of the highest sequence seen.
    Duplicate,
    /// Session identifier changed: the desktop restarted its stream.
    SessionChanged,
}

/// Tracks sequence continuity for one stream.
///
/// Used by the headless test receiver and by StageLink Mobile. Deliberately
/// allocation-free so a receiver can run it on its audio thread.
#[derive(Debug, Clone)]
pub struct SequenceTracker {
    session_id: Option<u32>,
    highest: u32,
    received: u64,
    /// Count of sequence numbers never seen, derived from forward gaps.
    lost: u64,
    reordered: u64,
    duplicates: u64,
}

impl Default for SequenceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            session_id: None,
            highest: 0,
            received: 0,
            lost: 0,
            reordered: 0,
            duplicates: 0,
        }
    }

    /// Records a packet and classifies it.
    pub fn observe(&mut self, session_id: u32, sequence: u32) -> SequenceVerdict {
        match self.session_id {
            None => {
                self.session_id = Some(session_id);
                self.highest = sequence;
                self.received = 1;
                SequenceVerdict::First
            }
            Some(current) if current != session_id => {
                // A new session restarts every statistic: sequence numbers from
                // the previous session are meaningless now.
                self.session_id = Some(session_id);
                self.highest = sequence;
                self.received = 1;
                self.lost = 0;
                self.reordered = 0;
                self.duplicates = 0;
                SequenceVerdict::SessionChanged
            }
            Some(_) => {
                self.received += 1;
                let delta = sequence_delta(self.highest, sequence);
                match delta {
                    0 => {
                        self.duplicates += 1;
                        SequenceVerdict::Duplicate
                    }
                    1 => {
                        self.highest = sequence;
                        SequenceVerdict::InOrder
                    }
                    d if d > 1 => {
                        let missing = (d - 1) as u32;
                        self.lost += u64::from(missing);
                        self.highest = sequence;
                        SequenceVerdict::Gap { missing }
                    }
                    d => {
                        // Negative: a late packet. If we had counted it as lost
                        // on an earlier gap, take it back off the lost tally.
                        self.reordered += 1;
                        self.lost = self.lost.saturating_sub(1);
                        SequenceVerdict::Reordered { behind: (-d) as u32 }
                    }
                }
            }
        }
    }

    #[must_use]
    pub const fn received(&self) -> u64 {
        self.received
    }

    #[must_use]
    pub const fn lost(&self) -> u64 {
        self.lost
    }

    #[must_use]
    pub const fn reordered(&self) -> u64 {
        self.reordered
    }

    #[must_use]
    pub const fn duplicates(&self) -> u64 {
        self.duplicates
    }

    #[must_use]
    pub const fn session_id(&self) -> Option<u32> {
        self.session_id
    }

    /// Loss as a percentage of packets the sender is believed to have sent.
    #[must_use]
    pub fn loss_percent(&self) -> f64 {
        let expected = self.received + self.lost;
        if expected == 0 {
            0.0
        } else {
            (self.lost as f64 / expected as f64) * 100.0
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_is_refused_in_production() {
        assert!(!TransportKind::UdpPlaintext.allowed_in_production());
        assert!(TransportKind::UdpEncrypted.allowed_in_production());
    }

    #[test]
    fn first_packet_is_classified_as_first() {
        let mut tracker = SequenceTracker::new();
        assert_eq!(tracker.observe(1, 100), SequenceVerdict::First);
        assert_eq!(tracker.received(), 1);
        assert_eq!(tracker.lost(), 0);
    }

    #[test]
    fn a_clean_run_reports_no_loss() {
        let mut tracker = SequenceTracker::new();
        tracker.observe(1, 0);
        for sequence in 1..1_000 {
            assert_eq!(tracker.observe(1, sequence), SequenceVerdict::InOrder);
        }
        assert_eq!(tracker.received(), 1_000);
        assert_eq!(tracker.lost(), 0);
        assert!(tracker.loss_percent().abs() < f64::EPSILON);
    }

    #[test]
    fn a_gap_counts_the_missing_packets() {
        let mut tracker = SequenceTracker::new();
        tracker.observe(1, 0);
        tracker.observe(1, 1);
        assert_eq!(tracker.observe(1, 5), SequenceVerdict::Gap { missing: 3 });
        assert_eq!(tracker.lost(), 3);
        // 3 missing out of 6 expected (0..=5) is 50%.
        assert!((tracker.loss_percent() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn a_late_packet_is_reordered_not_lost() {
        let mut tracker = SequenceTracker::new();
        tracker.observe(1, 0);
        tracker.observe(1, 3); // gap of 2 (1 and 2 missing)
        assert_eq!(tracker.lost(), 2);

        // Packet 2 shows up late; one of those was not actually lost.
        assert_eq!(tracker.observe(1, 2), SequenceVerdict::Reordered { behind: 1 });
        assert_eq!(tracker.reordered(), 1);
        assert_eq!(tracker.lost(), 1);
    }

    #[test]
    fn duplicates_are_counted_separately() {
        let mut tracker = SequenceTracker::new();
        tracker.observe(1, 0);
        tracker.observe(1, 1);
        assert_eq!(tracker.observe(1, 1), SequenceVerdict::Duplicate);
        assert_eq!(tracker.duplicates(), 1);
        assert_eq!(tracker.lost(), 0);
    }

    #[test]
    fn tracks_across_the_32_bit_wrap() {
        let mut tracker = SequenceTracker::new();
        tracker.observe(1, u32::MAX - 1);
        assert_eq!(tracker.observe(1, u32::MAX), SequenceVerdict::InOrder);
        assert_eq!(tracker.observe(1, 0), SequenceVerdict::InOrder);
        assert_eq!(tracker.observe(1, 1), SequenceVerdict::InOrder);
        assert_eq!(tracker.lost(), 0, "the wrap must not look like loss");
    }

    #[test]
    fn a_new_session_resets_statistics() {
        let mut tracker = SequenceTracker::new();
        tracker.observe(1, 0);
        tracker.observe(1, 10); // 9 lost
        assert_eq!(tracker.lost(), 9);

        assert_eq!(tracker.observe(2, 0), SequenceVerdict::SessionChanged);
        assert_eq!(tracker.lost(), 0);
        assert_eq!(tracker.received(), 1);
        assert_eq!(tracker.session_id(), Some(2));
    }

    #[test]
    fn loss_percent_is_zero_before_any_packet() {
        assert!(SequenceTracker::new().loss_percent().abs() < f64::EPSILON);
    }
}

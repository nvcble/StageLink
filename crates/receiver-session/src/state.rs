use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Where a session is in its lifecycle.
///
/// Mirrors `SessionState` in `@stagelink/shared-protocol`; the transition table
/// below is the authoritative copy and the TypeScript one exists so the UI can
/// grey out impossible actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionState {
    /// Nothing running.
    Idle,
    /// Opening the capture device and binding sockets.
    Preparing,
    /// Capture and sockets up, audio not yet being sent.
    Ready,
    /// Audio flowing to receivers.
    Streaming,
    /// Draining queues and closing down.
    Stopping,
    /// A fault occurred; attempting automatic recovery.
    Recovering,
    /// Unrecoverable without operator action.
    Error,
}

impl SessionState {
    /// States reachable from here.
    #[must_use]
    pub const fn allowed_next(self) -> &'static [Self] {
        match self {
            Self::Idle => &[Self::Preparing],
            Self::Preparing => &[Self::Ready, Self::Error, Self::Stopping],
            Self::Ready => &[Self::Streaming, Self::Stopping, Self::Error],
            Self::Streaming => &[Self::Stopping, Self::Recovering, Self::Error],
            Self::Stopping => &[Self::Idle, Self::Error],
            Self::Recovering => &[Self::Streaming, Self::Ready, Self::Error, Self::Stopping],
            Self::Error => &[Self::Idle, Self::Preparing],
        }
    }

    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        self.allowed_next().contains(&next)
    }

    /// Whether the session is doing work, so a second start must be refused.
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::Idle | Self::Error)
    }

    /// Whether audio is being transmitted right now.
    #[must_use]
    pub const fn is_streaming(self) -> bool {
        matches!(self, Self::Streaming)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Streaming => "streaming",
            Self::Stopping => "stopping",
            Self::Recovering => "recovering",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    #[error("cannot move from {from} to {to}")]
    IllegalTransition { from: &'static str, to: &'static str },

    #[error("a session is already {0}")]
    AlreadyActive(&'static str),

    #[error("no session is running")]
    NotRunning,
}

/// What a transition request resulted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// The state changed.
    Changed { from: SessionState, to: SessionState },
    /// Already in the requested state; nothing to do. Reported rather than
    /// treated as an error so a duplicate stop is idempotent instead of noisy.
    NoOp,
}

/// One session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub label: Option<String>,
    pub state: SessionState,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub duration_ms: i64,
    pub peak_receivers: u32,
    pub total_receivers: u32,
    pub last_error: Option<String>,
}

impl Session {
    /// Creates an idle session.
    #[must_use]
    pub fn new(label: Option<String>, now_ms: i64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            state: SessionState::Idle,
            created_at: now_ms,
            started_at: None,
            ended_at: None,
            duration_ms: 0,
            peak_receivers: 0,
            total_receivers: 0,
            last_error: None,
        }
    }

    /// Attempts a transition, maintaining the derived timestamps.
    pub fn transition(
        &mut self,
        next: SessionState,
        now_ms: i64,
    ) -> Result<TransitionOutcome, SessionError> {
        if self.state == next {
            return Ok(TransitionOutcome::NoOp);
        }
        if !self.state.can_transition_to(next) {
            return Err(SessionError::IllegalTransition {
                from: self.state.as_str(),
                to: next.as_str(),
            });
        }

        let from = self.state;
        self.state = next;

        match next {
            SessionState::Streaming if self.started_at.is_none() => {
                self.started_at = Some(now_ms);
            }
            SessionState::Idle => {
                self.ended_at = Some(now_ms);
                if let Some(started) = self.started_at {
                    self.duration_ms = (now_ms - started).max(0);
                }
            }
            // Leaving the error state clears the stale message so the UI does
            // not keep showing a fault that has been dealt with.
            SessionState::Preparing if from == SessionState::Error => {
                self.last_error = None;
            }
            _ => {}
        }

        Ok(TransitionOutcome::Changed { from, to: next })
    }

    /// Moves to `Error` and records why. Always legal.
    pub fn fail(&mut self, message: impl Into<String>, now_ms: i64) {
        self.last_error = Some(message.into());
        if self.state != SessionState::Error {
            let _ = self.state.can_transition_to(SessionState::Error);
            self.state = SessionState::Error;
            self.ended_at = Some(now_ms);
            if let Some(started) = self.started_at {
                self.duration_ms = (now_ms - started).max(0);
            }
        }
    }

    /// Live duration, or the final duration once the session has ended.
    #[must_use]
    pub fn duration_at(&self, now_ms: i64) -> i64 {
        match (self.started_at, self.ended_at) {
            (Some(started), None) => (now_ms - started).max(0),
            _ => self.duration_ms,
        }
    }

    /// Records the current receiver count, tracking the peak.
    pub fn observe_receiver_count(&mut self, count: u32) {
        self.peak_receivers = self.peak_receivers.max(count);
    }

    /// Counts a receiver joining for the first time this session.
    pub fn count_new_receiver(&mut self) {
        self.total_receivers = self.total_receivers.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [SessionState; 7] = [
        SessionState::Idle,
        SessionState::Preparing,
        SessionState::Ready,
        SessionState::Streaming,
        SessionState::Stopping,
        SessionState::Recovering,
        SessionState::Error,
    ];

    #[test]
    fn a_new_session_is_idle_with_an_id() {
        let session = Session::new(Some("Sunday 1st".into()), 1_000);
        assert_eq!(session.state, SessionState::Idle);
        assert!(!session.id.is_empty());
        assert_eq!(session.started_at, None);
        assert_eq!(session.duration_ms, 0);
    }

    #[test]
    fn session_ids_are_unique() {
        let a = Session::new(None, 0);
        let b = Session::new(None, 0);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn the_normal_lifecycle_is_permitted() {
        let mut session = Session::new(None, 0);
        for (state, at) in [
            (SessionState::Preparing, 10),
            (SessionState::Ready, 20),
            (SessionState::Streaming, 30),
            (SessionState::Stopping, 90),
            (SessionState::Idle, 100),
        ] {
            assert!(
                matches!(
                    session.transition(state, at),
                    Ok(TransitionOutcome::Changed { .. })
                ),
                "{state:?} should be reachable"
            );
        }
        assert_eq!(session.started_at, Some(30));
        assert_eq!(session.ended_at, Some(100));
        assert_eq!(session.duration_ms, 70);
    }

    #[test]
    fn starting_a_stream_without_preparing_is_refused() {
        let mut session = Session::new(None, 0);
        assert!(matches!(
            session.transition(SessionState::Streaming, 1),
            Err(SessionError::IllegalTransition { from: "idle", to: "streaming" })
        ));
        assert_eq!(session.state, SessionState::Idle, "a refusal must not mutate");
    }

    #[test]
    fn a_duplicate_request_is_a_no_op_rather_than_an_error() {
        let mut session = Session::new(None, 0);
        session.transition(SessionState::Preparing, 1).unwrap();
        session.transition(SessionState::Ready, 2).unwrap();
        session.transition(SessionState::Streaming, 3).unwrap();
        // Pressing Start twice must not be an error, and must not restart the
        // clock.
        assert_eq!(
            session.transition(SessionState::Streaming, 50),
            Ok(TransitionOutcome::NoOp)
        );
        assert_eq!(session.started_at, Some(3));
    }

    #[test]
    fn stopping_twice_is_idempotent() {
        let mut session = Session::new(None, 0);
        session.transition(SessionState::Preparing, 1).unwrap();
        session.transition(SessionState::Ready, 2).unwrap();
        session.transition(SessionState::Stopping, 3).unwrap();
        session.transition(SessionState::Idle, 4).unwrap();
        assert_eq!(
            session.transition(SessionState::Idle, 5),
            Ok(TransitionOutcome::NoOp)
        );
        assert_eq!(session.ended_at, Some(4), "the end time must not move");
    }

    #[test]
    fn recovery_can_return_to_streaming() {
        let mut session = Session::new(None, 0);
        session.transition(SessionState::Preparing, 1).unwrap();
        session.transition(SessionState::Ready, 2).unwrap();
        session.transition(SessionState::Streaming, 3).unwrap();
        session.transition(SessionState::Recovering, 4).unwrap();
        assert!(session.transition(SessionState::Streaming, 5).is_ok());
        assert_eq!(session.started_at, Some(3), "recovery is not a new start");
    }

    #[test]
    fn failing_records_the_reason_and_is_always_legal() {
        let mut session = Session::new(None, 0);
        session.transition(SessionState::Preparing, 1).unwrap();
        session.transition(SessionState::Ready, 2).unwrap();
        session.transition(SessionState::Streaming, 3).unwrap();

        session.fail("capture device disconnected", 40);
        assert_eq!(session.state, SessionState::Error);
        assert_eq!(
            session.last_error.as_deref(),
            Some("capture device disconnected")
        );
        assert_eq!(session.duration_ms, 37);
    }

    #[test]
    fn restarting_after_an_error_clears_the_stale_message() {
        let mut session = Session::new(None, 0);
        session.fail("something broke", 10);
        session.transition(SessionState::Preparing, 20).unwrap();
        assert_eq!(
            session.last_error, None,
            "a resolved fault must not keep showing in the UI"
        );
    }

    #[test]
    fn the_error_state_cannot_jump_straight_back_to_streaming() {
        let mut session = Session::new(None, 0);
        session.fail("broken", 1);
        assert!(session.transition(SessionState::Streaming, 2).is_err());
        assert!(session.transition(SessionState::Ready, 2).is_err());
        // The device must be reopened first.
        assert!(session.transition(SessionState::Preparing, 2).is_ok());
    }

    #[test]
    fn every_state_has_a_transition_table_and_none_lists_itself() {
        for state in ALL {
            assert!(
                !state.allowed_next().is_empty(),
                "{state:?} is a dead end"
            );
            assert!(
                !state.allowed_next().contains(&state),
                "{state:?} must not list itself; use the NoOp path"
            );
        }
    }

    #[test]
    fn active_and_streaming_classification_is_consistent() {
        assert!(!SessionState::Idle.is_active());
        assert!(!SessionState::Error.is_active());
        for state in [
            SessionState::Preparing,
            SessionState::Ready,
            SessionState::Streaming,
            SessionState::Stopping,
            SessionState::Recovering,
        ] {
            assert!(state.is_active(), "{state:?} should count as active");
        }
        assert!(SessionState::Streaming.is_streaming());
        assert!(!SessionState::Ready.is_streaming());
    }

    #[test]
    fn live_duration_grows_until_the_session_ends() {
        let mut session = Session::new(None, 0);
        session.transition(SessionState::Preparing, 1).unwrap();
        session.transition(SessionState::Ready, 2).unwrap();
        session.transition(SessionState::Streaming, 100).unwrap();
        assert_eq!(session.duration_at(1_100), 1_000);

        session.transition(SessionState::Stopping, 1_100).unwrap();
        session.transition(SessionState::Idle, 1_200).unwrap();
        assert_eq!(session.duration_at(9_999), 1_100, "a finished session is fixed");
    }

    #[test]
    fn receiver_counts_track_the_peak() {
        let mut session = Session::new(None, 0);
        session.observe_receiver_count(3);
        session.observe_receiver_count(7);
        session.observe_receiver_count(2);
        assert_eq!(session.peak_receivers, 7);

        session.count_new_receiver();
        session.count_new_receiver();
        assert_eq!(session.total_receivers, 2);
    }

    #[test]
    fn states_serialise_in_the_protocol_form() {
        assert_eq!(
            serde_json::to_string(&SessionState::Recovering).unwrap(),
            "\"recovering\""
        );
    }
}

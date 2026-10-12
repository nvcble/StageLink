//! Session lifecycle and the receiver roster.
//!
//! The session state machine is the authority on what the system is doing. Both
//! the UI and the service consult it before acting, which is how duplicate
//! start/stop requests and impossible transitions are refused in one place
//! rather than guarded ad hoc at every call site.

pub mod roster;
pub mod state;

pub use roster::{ReceiverRoster, RosterEntry};
pub use state::{Session, SessionError, SessionState, TransitionOutcome};

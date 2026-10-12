//! The receiver control channel.
//!
//! The server a phone joins. Carries the **same NDJSON envelope and the same
//! method vocabulary** as the local Electron pipe — one envelope codec, one
//! method registry, two transports — because a second protocol for the same
//! conversation is a second protocol to keep in step, and they would not stay
//! in step.
//!
//! ```text
//!   phone ──WebSocket/TCP 45871──► control-server ──► roster
//!              NDJSON envelopes          │             │
//!                                        ▼             ▼
//!                                   join code     stream sender
//!                                   (constant     (add/remove
//!                                    time)         destination)
//! ```
//!
//! ## Why WebSocket rather than a raw TCP stream
//!
//! The local pipe gets away with raw NDJSON because both ends are ours and the
//! pipe cannot be reached from the network. This server is on a venue network
//! and will eventually be spoken to by receiver implementations that are not
//! ours — the protocol documentation exists for exactly that. WebSocket gives
//! those implementations message framing for free, in every language, instead
//! of each one having to get partial-line buffering right. It also means a
//! phone's browser could join without an app at all.
//!
//! ## What authorises a receiver
//!
//! Possession of the join token from the QR code, compared in constant time,
//! and nothing else. There is no approval queue — scanning *is* the
//! authorisation. The token is a bearer credential, which is why it expires
//! after five minutes and the operator can rotate it at will. See
//! `docs/protocol/pairing.md`.

pub mod join;
pub mod server;
pub mod session;

pub use join::{Admission, JoinError};
pub use server::{ControlServer, ControlServerConfig, ServerEvent};
pub use session::{ReceiverSessions, SessionHandle};

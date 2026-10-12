//! StageLink local persistence.
//!
//! One SQLite database, opened by the Rust service and by nothing else. A
//! single writer avoids the lock contention that would otherwise show up at the
//! worst possible moment, and it keeps the schema in one place.
//!
//! Two rules this crate exists to enforce:
//!
//! 1. **Never from the audio path.** Every function here can block on disk
//!    I/O. Capture and packetisation must not call them, directly or
//!    indirectly. The service only touches the database from its control task.
//! 2. **No secrets in ordinary rows.** Receiver public keys and fingerprints
//!    live here because they are not secret. Long-term private keys and session
//!    keys go to the OS credential store instead — see [`secrets`].

pub mod db;
pub mod error;
pub mod migrations;
pub mod repositories;
pub mod secrets;

pub use db::{Database, SCHEMA_VERSION};
pub use error::{PersistenceError, Result};
pub use repositories::{
    ReceiverRecord, ReceiverRepository, SessionRecord, SessionRepository, SettingsRepository,
};
pub use secrets::SecretStore;

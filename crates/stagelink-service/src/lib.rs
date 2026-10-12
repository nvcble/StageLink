//! StageLink native audio service, as a library.
//!
//! The binary in `main.rs` is a thin wrapper around this. Exposing the modules
//! as a library is what lets the protocol cross-check tests in `tests/protocol`
//! drive the *real* Rust implementation — the same code the service runs —
//! rather than a reimplementation that could drift from it.

pub mod audio_engine;
pub mod handler;
pub mod serialize;
pub mod settings;
pub mod state;
pub mod telemetry_task;

pub use handler::{CONTROL_PROTOCOL_VERSION, DISPATCHED_METHODS, SERVICE_VERSION};
pub use settings::AppSettings;

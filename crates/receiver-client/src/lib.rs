//! The receiver side of StageLink, as a library.
//!
//! Everything a receiver needs that is not platform-specific: the QR payload,
//! the control channel, the audio socket, and the glue that holds them
//! together. The audio *path* is `receiver-core`; this crate is what connects
//! it to a real desktop.
//!
//! ```text
//!   QR ──► JoinPayload ──► ControlClient ──► welcome + stream params
//!                                │                      │
//!                                │ heartbeat 1 s        ▼
//!                                │ telemetry 2 s   AudioSocket ──► jitter buffer
//!                                ▼                                      │
//!                           desktop roster                         AudioOutput
//!                                                                       │
//!                                                               platform sink
//! ```
//!
//! The mobile app sits on top of this through a C ABI, and the headless
//! `stagelink-receiver` binary sits on top of the same thing with a WAV writer
//! where the sink would be. That is the point: the binary is not a simulation
//! of the app, it is the app's own lower half with a different output.

pub mod audio;
pub mod control;
pub mod join_payload;
pub mod session;

pub use audio::{AudioSocket, SocketStats};
pub use control::{
    ConnectionState, ControlClient, ControlEvent, ControlStatus, DeviceIdentity,
    TelemetryReport, Welcome,
};
pub use join_payload::{JoinPayload, PayloadError};
pub use session::{ReceiverSession, SessionConfig};

/// Control protocol version this receiver speaks.
///
/// Must match the desktop's major. It is checked at the scan rather than at
/// the handshake, so a person standing in front of the laptop is told to
/// update something instead of watching a connection fail.
pub const CONTROL_PROTOCOL_VERSION: &str = "1.0.0";

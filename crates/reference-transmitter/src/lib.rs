//! StageLink reference transmitter, as a library.
//!
//! The binary of the same name sends over a socket. This library exposes the
//! same pieces so other test tools can drive them directly — in particular
//! `receiver-harness`, which needs packet-at-a-time control and its own clock
//! so that a 90-minute drift scenario can run in a few seconds instead of 90
//! minutes.
//!
//! Everything here is a **test fixture**. The audio is generated or replayed
//! from a file, never captured, and no part of the desktop application depends
//! on this crate.

pub mod impairment;
pub mod source;
pub mod transmit;

pub use impairment::{Impairment, ImpairmentConfig, ImpairmentStats, Verdict};
pub use source::Source;
pub use transmit::{CollectingSink, Sink, TransmitConfig, TransmitReport, Transmitter};

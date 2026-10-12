//! Audio packetisation and transmission.
//!
//! ## Phase status
//!
//! The **packetiser** ([`packetiser`]) is complete and tested: it turns the
//! mono `f32` capture ring into StageLink datagrams with correct sequencing,
//! timestamps and silence/discontinuity flags.
//!
//! The **UDP sender** is Phase 3 work. [`config`] and [`queue`] define the
//! shapes it will use — in particular the per-receiver bounded queue, which is
//! the mechanism that stops one slow phone on a congested Wi-Fi link from
//! stalling the capture thread or starving the other receivers. The queue is
//! implemented and tested here because its drop policy is the part worth
//! getting right before any sockets exist.

pub mod config;
pub mod packetiser;
pub mod queue;

pub use config::{StreamConfig, StreamConfigError};
pub use packetiser::{PacketiserStats, PacketBuilder};
pub use queue::{QueueStats, ReceiverQueue};

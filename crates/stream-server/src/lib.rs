//! Audio packetisation and transmission.
//!
//! ## Phase status
//!
//! The **packetiser** ([`packetiser`]) is complete and tested: it turns the
//! mono `f32` capture ring into StageLink datagrams with correct sequencing,
//! timestamps and silence/discontinuity flags.
//!
//! The **UDP sender** ([`sender`]) is complete: one socket, one thread, one
//! packet per frame interval, fanned out to every receiver in the roster, with
//! a bounded queue each so one slow phone on a congested link cannot stall the
//! capture thread or starve its peers.
//!
//! Encryption is the remaining gap: the sender sets the `ENCRYPTED` flag and
//! reserves the AEAD tag region, but nothing seals the payload yet, so a
//! release build still refuses to stream.

pub mod config;
pub mod packetiser;
pub mod queue;
pub mod sender;

pub use config::{StreamConfig, StreamConfigError};
pub use packetiser::{PacketiserStats, PacketBuilder};
pub use queue::{QueueStats, ReceiverQueue};
pub use sender::{DestinationStats, FrameSource, SenderStats, StreamSender};

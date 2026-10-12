//! StageLink audio packet protocol, version 1.
//!
//! This crate is the authoritative Rust implementation of the wire format
//! described in `docs/protocol/audio-packets.md` and mirrored in
//! `packages/shared-protocol/src/audio-packet.ts`. The test suite in
//! `tests/protocol` asserts the two stay in agreement.
//!
//! Design notes that matter for a receiver implementor:
//!
//! * The 24-byte header is **always in the clear**, even on the encrypted
//!   transport, so a receiver can sequence and reorder packets before spending
//!   CPU on decryption. On the encrypted transport the header is bound in as
//!   AEAD associated data, so it still cannot be altered undetected.
//! * Sequence numbers are 32-bit and wrap. Use [`sequence_delta`] rather than
//!   subtracting, or a wrap looks like four billion lost packets.
//! * Timestamps are capture-clock microseconds from a monotonic source. They
//!   are useful for jitter measurement and for ordering, but they are *not*
//!   wall-clock time and must not be compared across a service restart.

pub mod packet;
pub mod transport;

pub use packet::{
    AEAD_TAG_BYTES, AudioPacketHeader, HEADER_BYTES, MAX_PAYLOAD_BYTES, PACKET_MAGIC,
    PacketError, PacketFlags, PayloadFormat, datagram_bytes, encode_header, fits_in_mtu,
    parse_header, sequence_delta,
};
pub use transport::{SequenceTracker, SequenceVerdict, TransportKind};

/// Audio protocol version written into every datagram.
pub const AUDIO_PROTOCOL_VERSION: u8 = 1;

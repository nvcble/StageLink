//! Audio packet header encode/decode.

use thiserror::Error;

use crate::AUDIO_PROTOCOL_VERSION;

/// ASCII `"SLNK"`. First four bytes of every datagram.
pub const PACKET_MAGIC: [u8; 4] = *b"SLNK";

/// Fixed header size. Pinned by the protocol tests so receivers may hard-code it.
pub const HEADER_BYTES: usize = 24;

/// Poly1305 authentication tag appended to encrypted payloads.
pub const AEAD_TAG_BYTES: usize = 16;

/// Largest payload we will emit, chosen to stay inside a 1500-byte MTU.
pub const MAX_PAYLOAD_BYTES: usize = 1_400;

/// Field offsets within the header.
mod offset {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const FLAGS: usize = 5;
    pub const PAYLOAD_FORMAT: usize = 6;
    pub const CHANNELS: usize = 7;
    pub const SESSION_ID: usize = 8;
    pub const SEQUENCE: usize = 12;
    pub const TIMESTAMP_US: usize = 16;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PacketError {
    #[error("datagram is {len} bytes, shorter than the {HEADER_BYTES}-byte header")]
    TooShort { len: usize },

    #[error("bad magic: expected \"SLNK\"")]
    BadMagic,

    #[error("unsupported audio protocol version {found} (this build speaks {AUDIO_PROTOCOL_VERSION})")]
    VersionMismatch { found: u8 },

    #[error("unknown payload format code {0}")]
    UnknownPayloadFormat(u8),

    #[error("expected 1 channel (mono monitor mix), header declares {0}")]
    NotMono(u8),

    #[error("payload is {len} bytes, over the {MAX_PAYLOAD_BYTES}-byte limit")]
    PayloadTooLarge { len: usize },

    #[error("encrypted packet payload is {len} bytes, too short to contain a {AEAD_TAG_BYTES}-byte tag")]
    MissingAeadTag { len: usize },

    #[error("output buffer is {available} bytes, needs {required}")]
    BufferTooSmall { required: usize, available: usize },

    #[error("payload is {len} bytes, not a whole number of 16-bit samples")]
    RaggedPayload { len: usize },
}

/// Payload encoding. Only PCM S16LE is defined for protocol v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PayloadFormat {
    PcmS16Le = 0,
}

impl PayloadFormat {
    fn from_code(code: u8) -> Result<Self, PacketError> {
        match code {
            0 => Ok(Self::PcmS16Le),
            other => Err(PacketError::UnknownPayloadFormat(other)),
        }
    }

    /// Bytes per mono sample in this encoding.
    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::PcmS16Le => 2,
        }
    }
}

/// Header bitflags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PacketFlags(pub u8);

impl PacketFlags {
    pub const ENCRYPTED: u8 = 1 << 0;
    pub const END_OF_STREAM: u8 = 1 << 1;
    pub const SILENCE: u8 = 1 << 2;
    pub const DISCONTINUITY: u8 = 1 << 3;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The raw bitfield, as it appears in the header.
    ///
    /// A receiver needs this: it stores a packet's flags alongside its audio
    /// and reads them back when the packet is played, which may be a hundred
    /// milliseconds later and on a different thread.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Rebuilds flags from a raw bitfield.
    ///
    /// Unknown bits are kept rather than rejected, because
    /// `docs/protocol/audio-packets.md` requires them to be ignored — that is
    /// what lets a minor protocol revision stay compatible.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn with(self, flag: u8) -> Self {
        Self(self.0 | flag)
    }

    #[must_use]
    pub const fn has(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    #[must_use]
    pub const fn encrypted(self) -> bool {
        self.has(Self::ENCRYPTED)
    }

    #[must_use]
    pub const fn end_of_stream(self) -> bool {
        self.has(Self::END_OF_STREAM)
    }
}

/// Parsed packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacketHeader {
    pub version: u8,
    pub flags: PacketFlags,
    pub payload_format: PayloadFormat,
    pub channels: u8,
    /// Truncated session identifier; lets a receiver notice a session change.
    pub session_id: u32,
    pub sequence: u32,
    /// Capture-clock microseconds, monotonic within a session.
    pub timestamp_us: u64,
}

impl AudioPacketHeader {
    /// Builds a v1 mono PCM header.
    #[must_use]
    pub const fn new(session_id: u32, sequence: u32, timestamp_us: u64, flags: PacketFlags) -> Self {
        Self {
            version: AUDIO_PROTOCOL_VERSION,
            flags,
            payload_format: PayloadFormat::PcmS16Le,
            channels: 1,
            session_id,
            sequence,
            timestamp_us,
        }
    }
}

/// Writes the header into the first [`HEADER_BYTES`] of `out`.
pub fn encode_header(header: &AudioPacketHeader, out: &mut [u8]) -> Result<usize, PacketError> {
    if out.len() < HEADER_BYTES {
        return Err(PacketError::BufferTooSmall {
            required: HEADER_BYTES,
            available: out.len(),
        });
    }
    out[offset::MAGIC..offset::MAGIC + 4].copy_from_slice(&PACKET_MAGIC);
    out[offset::VERSION] = header.version;
    out[offset::FLAGS] = header.flags.0;
    out[offset::PAYLOAD_FORMAT] = header.payload_format as u8;
    out[offset::CHANNELS] = header.channels;
    out[offset::SESSION_ID..offset::SESSION_ID + 4]
        .copy_from_slice(&header.session_id.to_le_bytes());
    out[offset::SEQUENCE..offset::SEQUENCE + 4].copy_from_slice(&header.sequence.to_le_bytes());
    out[offset::TIMESTAMP_US..offset::TIMESTAMP_US + 8]
        .copy_from_slice(&header.timestamp_us.to_le_bytes());
    Ok(HEADER_BYTES)
}

/// Parses a datagram into its header and payload slice.
///
/// Validates everything a receiver must check before trusting the payload:
/// magic, version, payload format, channel count, payload length, and — for
/// encrypted packets — that there is room for the AEAD tag.
pub fn parse_header(datagram: &[u8]) -> Result<(AudioPacketHeader, &[u8]), PacketError> {
    if datagram.len() < HEADER_BYTES {
        return Err(PacketError::TooShort { len: datagram.len() });
    }
    if datagram[offset::MAGIC..offset::MAGIC + 4] != PACKET_MAGIC {
        return Err(PacketError::BadMagic);
    }

    let version = datagram[offset::VERSION];
    if version != AUDIO_PROTOCOL_VERSION {
        return Err(PacketError::VersionMismatch { found: version });
    }

    let flags = PacketFlags(datagram[offset::FLAGS]);
    let payload_format = PayloadFormat::from_code(datagram[offset::PAYLOAD_FORMAT])?;
    let channels = datagram[offset::CHANNELS];
    if channels != 1 {
        return Err(PacketError::NotMono(channels));
    }

    let session_id = u32::from_le_bytes(
        datagram[offset::SESSION_ID..offset::SESSION_ID + 4]
            .try_into()
            .expect("4-byte slice"),
    );
    let sequence = u32::from_le_bytes(
        datagram[offset::SEQUENCE..offset::SEQUENCE + 4]
            .try_into()
            .expect("4-byte slice"),
    );
    let timestamp_us = u64::from_le_bytes(
        datagram[offset::TIMESTAMP_US..offset::TIMESTAMP_US + 8]
            .try_into()
            .expect("8-byte slice"),
    );

    let payload = &datagram[HEADER_BYTES..];
    if payload.len() > MAX_PAYLOAD_BYTES + AEAD_TAG_BYTES {
        return Err(PacketError::PayloadTooLarge { len: payload.len() });
    }
    if flags.encrypted() && payload.len() < AEAD_TAG_BYTES {
        return Err(PacketError::MissingAeadTag { len: payload.len() });
    }
    // A plaintext PCM payload must be a whole number of samples; a ragged one
    // means a truncated datagram and should be dropped, not half-decoded.
    if !flags.encrypted() && !payload.len().is_multiple_of(payload_format.bytes_per_sample()) {
        return Err(PacketError::RaggedPayload { len: payload.len() });
    }

    Ok((
        AudioPacketHeader {
            version,
            flags,
            payload_format,
            channels,
            session_id,
            sequence,
            timestamp_us,
        },
        payload,
    ))
}

/// Total datagram size for `frames` mono S16 samples on a given transport.
#[must_use]
pub const fn datagram_bytes(frames: usize, encrypted: bool) -> usize {
    HEADER_BYTES + frames * 2 + if encrypted { AEAD_TAG_BYTES } else { 0 }
}

/// Whether a configuration fits one unfragmented datagram.
///
/// Accounts for the 20-byte IPv4 and 8-byte UDP headers.
#[must_use]
pub const fn fits_in_mtu(frames: usize, encrypted: bool, mtu: usize) -> bool {
    datagram_bytes(frames, encrypted) + 28 <= mtu
}

/// Forward distance from `previous` to `current` across the 32-bit wrap.
///
/// Returns a negative value for a reordered (late) packet. Both the desktop and
/// every receiver must use this so their loss statistics agree.
#[must_use]
pub fn sequence_delta(previous: u32, current: u32) -> i64 {
    const SPAN: i64 = 1 << 32;
    let delta = i64::from(current) - i64::from(previous);
    let wrapped = delta.rem_euclid(SPAN);
    if wrapped > SPAN / 2 { wrapped - SPAN } else { wrapped }
}

/// Nonce length for ChaCha20-Poly1305 (the IETF 96-bit variant).
pub const AEAD_NONCE_BYTES: usize = 12;

/// The AEAD nonce for one packet, byte-exactly.
///
/// ```text
///  offset size contents
///  ------ ---- --------------------------------
///       0    4 zero
///       4    4 sessionId, little-endian
///       8    4 sequence,  little-endian
/// ```
///
/// Four leading zero bytes rather than four trailing ones, so the varying part
/// sits in the low-order half as it does in TLS and in every other
/// counter-style construction a reviewer will have seen. The zeros are reserved
/// for a future epoch field; a receiver must not require them to be zero beyond
/// what this function produces.
///
/// # Why this must be specified down to the byte
///
/// Nonce reuse under ChaCha20-Poly1305 is catastrophic, not merely weak: two
/// packets sealed with the same key and nonce leak the XOR of their plaintexts
/// and, far worse, allow the Poly1305 key to be recovered and arbitrary packets
/// forged. A forged packet here means injected audio in a performer's ears. So
/// the uniqueness argument has to be explicit:
///
/// * `sequence` is unique within a session, and increments by exactly one.
/// * It wraps at 2^32. At the default 200 packets/second that is **248 days**
///   of continuous streaming, so it cannot happen inside a service — but the
///   sender must not rely on that. A session that reaches the wrap must take a
///   fresh key.
/// * `sessionId` separates sessions, but it is a *truncated* identifier and
///   therefore may repeat. It is in the nonce to keep an accidental sequence
///   restart from colliding, **not** as the uniqueness guarantee.
///
/// The actual guarantee is the key: each session derives its own via
/// `HKDF-SHA256(ikm = control-channel shared secret, salt = sessionId_le,
/// info = "stagelink audio v1")`. One key per session means a nonce need only
/// be unique within a session, which `sequence` delivers.
#[must_use]
pub fn aead_nonce(session_id: u32, sequence: u32) -> [u8; AEAD_NONCE_BYTES] {
    let mut nonce = [0u8; AEAD_NONCE_BYTES];
    nonce[4..8].copy_from_slice(&session_id.to_le_bytes());
    nonce[8..12].copy_from_slice(&sequence.to_le_bytes());
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> AudioPacketHeader {
        AudioPacketHeader::new(0xDEAD_BEEF, 42, 1_234_567, PacketFlags::empty())
    }

    #[test]
    fn header_size_is_pinned() {
        assert_eq!(HEADER_BYTES, 24);
        assert_eq!(AUDIO_PROTOCOL_VERSION, 1);
    }

    #[test]
    fn header_round_trips() {
        let header = sample_header();
        let mut datagram = vec![0_u8; HEADER_BYTES + 4];
        encode_header(&header, &mut datagram).unwrap();
        datagram[HEADER_BYTES..].copy_from_slice(&[1, 2, 3, 4]);

        let (parsed, payload) = parse_header(&datagram).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(payload, &[1, 2, 3, 4]);
    }

    #[test]
    fn magic_is_written_first() {
        let mut buffer = [0_u8; HEADER_BYTES];
        encode_header(&sample_header(), &mut buffer).unwrap();
        assert_eq!(&buffer[0..4], b"SLNK");
    }

    #[test]
    fn encode_refuses_a_short_buffer() {
        let mut buffer = [0_u8; HEADER_BYTES - 1];
        assert!(matches!(
            encode_header(&sample_header(), &mut buffer),
            Err(PacketError::BufferTooSmall { .. })
        ));
    }

    #[test]
    fn rejects_a_truncated_datagram() {
        assert!(matches!(
            parse_header(&[0_u8; 10]),
            Err(PacketError::TooShort { len: 10 })
        ));
    }

    #[test]
    fn rejects_foreign_traffic() {
        // Something else entirely arriving on our port must not be decoded.
        let mut datagram = [0_u8; HEADER_BYTES];
        datagram[0..4].copy_from_slice(b"HTTP");
        assert_eq!(parse_header(&datagram), Err(PacketError::BadMagic));
    }

    #[test]
    fn rejects_a_future_protocol_version() {
        let mut datagram = vec![0_u8; HEADER_BYTES];
        encode_header(&sample_header(), &mut datagram).unwrap();
        datagram[4] = 2;
        assert_eq!(
            parse_header(&datagram),
            Err(PacketError::VersionMismatch { found: 2 })
        );
    }

    #[test]
    fn rejects_a_non_mono_header() {
        let mut datagram = vec![0_u8; HEADER_BYTES];
        encode_header(&sample_header(), &mut datagram).unwrap();
        datagram[7] = 2;
        assert_eq!(parse_header(&datagram), Err(PacketError::NotMono(2)));
    }

    #[test]
    fn rejects_an_unknown_payload_format() {
        let mut datagram = vec![0_u8; HEADER_BYTES];
        encode_header(&sample_header(), &mut datagram).unwrap();
        datagram[6] = 9;
        assert_eq!(
            parse_header(&datagram),
            Err(PacketError::UnknownPayloadFormat(9))
        );
    }

    #[test]
    fn rejects_a_ragged_pcm_payload() {
        let mut datagram = vec![0_u8; HEADER_BYTES + 3];
        encode_header(&sample_header(), &mut datagram).unwrap();
        assert!(matches!(
            parse_header(&datagram),
            Err(PacketError::RaggedPayload { len: 3 })
        ));
    }

    #[test]
    fn rejects_an_oversized_payload() {
        let mut datagram = vec![0_u8; HEADER_BYTES + MAX_PAYLOAD_BYTES + AEAD_TAG_BYTES + 2];
        encode_header(&sample_header(), &mut datagram).unwrap();
        assert!(matches!(
            parse_header(&datagram),
            Err(PacketError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_an_encrypted_packet_with_no_room_for_the_tag() {
        let header = AudioPacketHeader::new(
            1,
            1,
            0,
            PacketFlags::empty().with(PacketFlags::ENCRYPTED),
        );
        let mut datagram = vec![0_u8; HEADER_BYTES + 8];
        encode_header(&header, &mut datagram).unwrap();
        assert!(matches!(
            parse_header(&datagram),
            Err(PacketError::MissingAeadTag { len: 8 })
        ));
    }

    #[test]
    fn encrypted_payloads_may_have_any_byte_length_above_the_tag() {
        // Ciphertext is not sample-aligned, so the ragged-payload check must
        // not apply to it.
        let header = AudioPacketHeader::new(
            1,
            1,
            0,
            PacketFlags::empty().with(PacketFlags::ENCRYPTED),
        );
        let mut datagram = vec![0_u8; HEADER_BYTES + AEAD_TAG_BYTES + 7];
        encode_header(&header, &mut datagram).unwrap();
        assert!(parse_header(&datagram).is_ok());
    }

    /// The two vectors in `docs/protocol/audio-packets.md`. A receiver that
    /// disagrees with these cannot decrypt a single packet, so they are
    /// written out byte by byte rather than recomputed from the function
    /// under test.
    #[test]
    fn the_aead_nonce_matches_the_published_test_vectors() {
        assert_eq!(
            aead_nonce(0x0000_5EED, 0),
            [0x00, 0x00, 0x00, 0x00, 0xED, 0x5E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            aead_nonce(0x1234_5678, 0xFFFF_FFFF),
            [0x00, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0xFF, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn consecutive_packets_in_a_session_never_share_a_nonce() {
        let session = 7;
        let mut seen = std::collections::HashSet::new();
        for sequence in 0..1_000 {
            assert!(
                seen.insert(aead_nonce(session, sequence)),
                "sequence {sequence} reused a nonce"
            );
        }
        // And the sequence is what varies, not the session: the same sequence
        // in a different session must also differ, so an accidental sequence
        // restart cannot collide with the previous session's packets.
        assert_ne!(aead_nonce(7, 0), aead_nonce(8, 0));
    }

    #[test]
    fn flags_compose_and_read_back() {
        let flags = PacketFlags::empty()
            .with(PacketFlags::ENCRYPTED)
            .with(PacketFlags::DISCONTINUITY);
        assert!(flags.encrypted());
        assert!(flags.has(PacketFlags::DISCONTINUITY));
        assert!(!flags.end_of_stream());
        assert!(!flags.has(PacketFlags::SILENCE));
    }

    #[test]
    fn flags_survive_the_round_trip() {
        let header = AudioPacketHeader::new(
            7,
            9,
            11,
            PacketFlags::empty().with(PacketFlags::END_OF_STREAM),
        );
        let mut datagram = vec![0_u8; HEADER_BYTES];
        encode_header(&header, &mut datagram).unwrap();
        let (parsed, _) = parse_header(&datagram).unwrap();
        assert!(parsed.flags.end_of_stream());
    }

    #[test]
    fn mtu_arithmetic_matches_the_typescript_helpers() {
        // 5 ms at 48 kHz, encrypted.
        assert_eq!(datagram_bytes(240, false), 24 + 480);
        assert_eq!(datagram_bytes(240, true), 24 + 480 + 16);
        assert!(fits_in_mtu(240, true, 1_500));
        assert!(fits_in_mtu(480, true, 1_500));
        // 20 ms would fragment.
        assert!(!fits_in_mtu(960, true, 1_500));
    }

    #[test]
    fn sequence_delta_handles_the_wrap() {
        assert_eq!(sequence_delta(10, 11), 1);
        assert_eq!(sequence_delta(10, 15), 5);
        assert_eq!(sequence_delta(10, 9), -1);
        assert_eq!(sequence_delta(u32::MAX, 0), 1);
        assert_eq!(sequence_delta(u32::MAX - 1, 1), 3);
        assert_eq!(sequence_delta(0, u32::MAX), -1);
        assert_eq!(sequence_delta(5, 5), 0);
    }
}

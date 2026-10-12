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

//! Sealing and opening audio payloads.
//!
//! One implementation, shared by the desktop sender and every receiver,
//! because two implementations of an AEAD is two chances to disagree about a
//! nonce — and disagreeing about a nonce is either silence or a key
//! disclosure, never something small.
//!
//! The scheme is specified byte-for-byte in `docs/protocol/audio-packets.md`:
//!
//! ```text
//! key   = HKDF-SHA256(ikm = join token, salt = sessionId_le, info = "stagelink audio v1")
//! nonce = 0x00·4 ‖ sessionId_le(4) ‖ sequence_le(4)
//! aad   = the 24-byte header, exactly as transmitted
//! ```
//!
//! ## Where the key comes from, and what that does and does not buy
//!
//! The join token. It is 256 bits from the OS generator, it is displayed only
//! as a QR code on the operator's screen, and it is already what authorises a
//! receiver — so the set of people who can decrypt the audio is exactly the
//! set who were shown the code. That closes the two threats that matter on a
//! venue network:
//!
//! * **Eavesdropping.** Nobody on the Wi-Fi can hear the mix without having
//!   been shown the code.
//! * **Forgery.** Nobody can inject audio, or forge an `END_OF_STREAM` marker
//!   and silence every performer at once, which a plaintext stream allows
//!   anyone on the network to do.
//!
//! What it does **not** buy, stated plainly because it is a real limitation
//! and not a theoretical one:
//!
//! * **Revocation does not re-key.** Every receiver admitted under one join
//!   code derives the same key, so a device the operator revokes can still
//!   decrypt the stream it was already receiving. Rotating the code does not
//!   help: the key was derived at join time. Closing this needs a per-receiver
//!   key delivered over a control channel that is itself encrypted and
//!   authenticated, which is a larger piece of work — see the open item in
//!   `docs/protocol/pairing.md`.
//! * **No forward secrecy.** A token captured later decrypts a recording of
//!   that session. The token is already a bearer credential with a five-minute
//!   life, so this adds no exposure that joining did not already have.
//!
//! A shared group key is also what SRTP does for exactly this shape of
//! problem — one identical stream to many listeners — so the structure is not
//! unusual. The gap is the revocation story, and it is the gap, not the
//! cipher.

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::packet::{AEAD_NONCE_BYTES, AEAD_TAG_BYTES, HEADER_BYTES, aead_nonce};

/// HKDF `info` string. Part of the wire contract: changing it changes every
/// derived key, so it is pinned by a test.
pub const KEY_INFO: &[u8] = b"stagelink audio v1";

/// A derived per-session audio key.
///
/// Deliberately not `Clone` or `Debug`: a key that can be printed ends up in
/// a log, and a key that can be cloned ends up in two places with two
/// lifetimes.
pub struct SessionKey {
    cipher: ChaCha20Poly1305,
    session_id: u32,
}

impl SessionKey {
    /// Derives the key for one session from the join token.
    ///
    /// The session id is the HKDF salt, so two sessions under the same token
    /// have different keys — which is what makes the nonce's uniqueness
    /// argument hold when a sequence number restarts at zero.
    #[must_use]
    pub fn derive(token: &str, session_id: u32) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(&session_id.to_le_bytes()), token.as_bytes());
        let mut key = [0_u8; 32];
        hkdf.expand(KEY_INFO, &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");

        let cipher = ChaCha20Poly1305::new(&Key::from(key));
        // The key material is copied into the cipher's own state; zero this
        // copy so it does not sit on the stack for the rest of the frame.
        key.fill(0);

        Self { cipher, session_id }
    }

    /// The session this key belongs to.
    #[must_use]
    pub const fn session_id(&self) -> u32 {
        self.session_id
    }

    /// Seals a datagram in place.
    ///
    /// `datagram` must be the header followed by `plaintext_len` bytes of
    /// payload and then [`AEAD_TAG_BYTES`] of room for the tag — which is what
    /// the packetiser already produces, so there is nothing to reallocate on
    /// the send path.
    ///
    /// The header is **not** encrypted and is bound in as associated data, so
    /// a receiver can sequence and discard duplicates before spending any CPU
    /// on decryption while still being unable to have the header altered
    /// underneath it.
    pub fn seal(
        &self,
        datagram: &mut [u8],
        plaintext_len: usize,
        sequence: u32,
    ) -> Result<usize, CryptoError> {
        let total = HEADER_BYTES + plaintext_len + AEAD_TAG_BYTES;
        if datagram.len() < total {
            return Err(CryptoError::NoRoomForTag);
        }

        let (header, rest) = datagram.split_at_mut(HEADER_BYTES);
        let aad: [u8; HEADER_BYTES] = header.try_into().map_err(|_| CryptoError::BadHeader)?;

        let nonce = Nonce::from(aead_nonce(self.session_id, sequence));
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce, &aad, (&mut rest[..plaintext_len]).into())
            .map_err(|_| CryptoError::SealFailed)?;

        rest[plaintext_len..plaintext_len + AEAD_TAG_BYTES].copy_from_slice(&tag);
        Ok(total)
    }

    /// Opens a payload in place, returning the plaintext length.
    ///
    /// `payload` is the ciphertext followed by the tag, and `header` is the
    /// 24 bytes exactly as they arrived. A failure here means the packet was
    /// altered, replayed under a different header, or sealed with a different
    /// key — and in every case the only safe thing to do is discard it.
    pub fn open(
        &self,
        header: &[u8],
        payload: &mut [u8],
        sequence: u32,
    ) -> Result<usize, CryptoError> {
        if payload.len() < AEAD_TAG_BYTES {
            return Err(CryptoError::NoTag);
        }
        let aad: [u8; HEADER_BYTES] = header
            .get(..HEADER_BYTES)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(CryptoError::BadHeader)?;

        let plaintext_len = payload.len() - AEAD_TAG_BYTES;
        let (ciphertext, tag) = payload.split_at_mut(plaintext_len);
        let nonce = Nonce::from(aead_nonce(self.session_id, sequence));
        let tag = chacha20poly1305::Tag::try_from(&*tag).map_err(|_| CryptoError::NoTag)?;

        self.cipher
            .decrypt_inout_detached(&nonce, &aad, ciphertext.into(), &tag)
            .map_err(|_| CryptoError::Unauthentic)?;

        Ok(plaintext_len)
    }
}

/// Why a packet could not be sealed or opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("the datagram has no room for the authentication tag")]
    NoRoomForTag,

    #[error("the payload is too short to contain a tag")]
    NoTag,

    #[error("the header is not {HEADER_BYTES} bytes")]
    BadHeader,

    #[error("the payload could not be sealed")]
    SealFailed,

    #[error(
        "the packet failed authentication: it was altered, replayed under a different header, \
         or sealed with a different key"
    )]
    Unauthentic,
}

/// Nonce length, re-exported so a caller does not have to reach into `packet`.
pub const NONCE_BYTES: usize = AEAD_NONCE_BYTES;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{AudioPacketHeader, PacketFlags, encode_header, parse_header};

    const TOKEN: &str = "9f2c3d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f";
    const SESSION: u32 = 0x5EED;
    const FRAMES: usize = 240;

    /// A datagram with a real header, a payload, and room for the tag.
    fn datagram(sequence: u32, value: u8) -> (Vec<u8>, usize) {
        let payload_len = FRAMES * 2;
        let mut datagram = vec![0_u8; HEADER_BYTES + payload_len + AEAD_TAG_BYTES];
        let header = AudioPacketHeader::new(
            SESSION,
            sequence,
            u64::from(sequence) * 5_000,
            PacketFlags::empty().with(PacketFlags::ENCRYPTED),
        );
        encode_header(&header, &mut datagram).unwrap();
        datagram[HEADER_BYTES..HEADER_BYTES + payload_len].fill(value);
        (datagram, payload_len)
    }

    #[test]
    fn a_sealed_packet_opens_back_to_the_same_audio() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let (mut sealed, payload_len) = datagram(7, 0xAB);
        let expected = sealed[HEADER_BYTES..HEADER_BYTES + payload_len].to_vec();

        let total = key.seal(&mut sealed, payload_len, 7).unwrap();
        assert_eq!(total, HEADER_BYTES + payload_len + AEAD_TAG_BYTES);
        assert_ne!(
            sealed[HEADER_BYTES..HEADER_BYTES + payload_len],
            expected[..],
            "the payload should not still be in the clear"
        );

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        let opened = key.open(header, payload, 7).unwrap();
        assert_eq!(opened, payload_len);
        assert_eq!(&payload[..opened], &expected[..]);
    }

    /// The header stays readable so a receiver can sequence and drop
    /// duplicates before spending CPU on decryption.
    #[test]
    fn the_header_is_left_in_the_clear_and_still_parses() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let (mut sealed, payload_len) = datagram(42, 0x11);
        key.seal(&mut sealed, payload_len, 42).unwrap();

        let (header, _) = parse_header(&sealed).expect("the header must still parse");
        assert_eq!(header.session_id, SESSION);
        assert_eq!(header.sequence, 42);
        assert!(header.flags.encrypted());
    }

    /// The forgery this closes. On a plaintext stream anyone on the venue
    /// network can send an `END_OF_STREAM` marker and silence every performer
    /// at once.
    #[test]
    fn a_packet_from_someone_without_the_token_is_refused() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let attacker = SessionKey::derive(&"0".repeat(64), SESSION);

        let (mut forged, payload_len) = datagram(7, 0xFF);
        attacker.seal(&mut forged, payload_len, 7).unwrap();

        let (header, payload) = forged.split_at_mut(HEADER_BYTES);
        assert_eq!(key.open(header, payload, 7), Err(CryptoError::Unauthentic));
    }

    #[test]
    fn a_tampered_payload_is_refused() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let (mut sealed, payload_len) = datagram(7, 0x22);
        key.seal(&mut sealed, payload_len, 7).unwrap();

        // Flip one bit of ciphertext.
        sealed[HEADER_BYTES + 10] ^= 0x01;

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        assert_eq!(key.open(header, payload, 7), Err(CryptoError::Unauthentic));
    }

    /// The header is associated data, so altering it is detected even though
    /// it travels in the clear. Without this, somebody could renumber packets
    /// and make a receiver play them in the wrong order.
    #[test]
    fn a_tampered_header_is_refused_even_though_it_is_not_encrypted() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let (mut sealed, payload_len) = datagram(7, 0x33);
        key.seal(&mut sealed, payload_len, 7).unwrap();

        // Set the END_OF_STREAM flag, which would stop every receiver.
        sealed[5] |= PacketFlags::END_OF_STREAM;

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        assert_eq!(key.open(header, payload, 7), Err(CryptoError::Unauthentic));
    }

    /// The nonce binds the sequence number, so a packet replayed under a
    /// different one fails rather than being played in the wrong place.
    #[test]
    fn a_packet_replayed_at_a_different_sequence_is_refused() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let (mut sealed, payload_len) = datagram(7, 0x44);
        key.seal(&mut sealed, payload_len, 7).unwrap();

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        assert_eq!(key.open(header, payload, 8), Err(CryptoError::Unauthentic));
    }

    /// Why the session id is the HKDF salt: when a sequence restarts at zero,
    /// a different key means the nonce pair is not reused.
    #[test]
    fn two_sessions_under_one_token_have_different_keys() {
        let first = SessionKey::derive(TOKEN, 1);
        let second = SessionKey::derive(TOKEN, 2);

        let (mut sealed, payload_len) = datagram(0, 0x55);
        first.seal(&mut sealed, payload_len, 0).unwrap();

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        assert_eq!(
            second.open(header, payload, 0),
            Err(CryptoError::Unauthentic),
            "a sequence restart under a new session must not reuse a key and nonce"
        );
    }

    #[test]
    fn the_same_token_and_session_derive_the_same_key_on_both_ends() {
        // The desktop and the receiver each derive independently; if this were
        // not deterministic, nothing would ever decrypt.
        let sender = SessionKey::derive(TOKEN, SESSION);
        let receiver = SessionKey::derive(TOKEN, SESSION);

        let (mut sealed, payload_len) = datagram(99, 0x66);
        sender.seal(&mut sealed, payload_len, 99).unwrap();

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        assert!(receiver.open(header, payload, 99).is_ok());
    }

    /// An end-of-stream marker carries no audio, so it seals a zero-length
    /// payload into a tag-only one. This is the case defect D1 was about.
    #[test]
    fn a_zero_length_payload_seals_and_opens() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let mut marker = vec![0_u8; HEADER_BYTES + AEAD_TAG_BYTES];
        let header = AudioPacketHeader::new(
            SESSION,
            100,
            500_000,
            PacketFlags::empty()
                .with(PacketFlags::ENCRYPTED)
                .with(PacketFlags::END_OF_STREAM),
        );
        encode_header(&header, &mut marker).unwrap();

        let total = key.seal(&mut marker, 0, 100).unwrap();
        assert_eq!(total, HEADER_BYTES + AEAD_TAG_BYTES);

        // And it parses, which is what D1 broke.
        let (parsed, _) = parse_header(&marker).expect("the marker must parse");
        assert!(parsed.flags.end_of_stream());

        let (header, payload) = marker.split_at_mut(HEADER_BYTES);
        assert_eq!(key.open(header, payload, 100).unwrap(), 0);
    }

    #[test]
    fn a_datagram_with_no_room_for_the_tag_is_refused_rather_than_truncated() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let mut tight = vec![0_u8; HEADER_BYTES + 480];
        assert_eq!(
            key.seal(&mut tight, 480, 0),
            Err(CryptoError::NoRoomForTag)
        );
    }

    #[test]
    fn a_payload_too_short_to_hold_a_tag_is_refused() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let header = vec![0_u8; HEADER_BYTES];
        let mut tiny = vec![0_u8; 8];
        assert_eq!(key.open(&header, &mut tiny, 0), Err(CryptoError::NoTag));
    }

    #[test]
    fn a_short_header_is_refused() {
        let key = SessionKey::derive(TOKEN, SESSION);
        let header = vec![0_u8; 8];
        let mut payload = vec![0_u8; 64];
        assert_eq!(
            key.open(&header, &mut payload, 0),
            Err(CryptoError::BadHeader)
        );
    }

    /// The `info` string is part of the wire contract: changing it changes
    /// every derived key, so a receiver built against a different one
    /// decrypts nothing.
    #[test]
    fn the_key_derivation_info_string_is_frozen() {
        assert_eq!(KEY_INFO, b"stagelink audio v1");
    }

    #[test]
    fn the_nonce_is_twelve_bytes_as_the_cipher_requires() {
        assert_eq!(NONCE_BYTES, 12);
        assert_eq!(aead_nonce(1, 1).len(), 12);
    }

    /// A different token is a different key, which is what makes the join
    /// code the thing that grants access to the audio and not just to the
    /// roster.
    #[test]
    fn rotating_the_join_code_changes_the_key() {
        let old = SessionKey::derive(TOKEN, SESSION);
        let new = SessionKey::derive(&"a".repeat(64), SESSION);

        let (mut sealed, payload_len) = datagram(1, 0x77);
        old.seal(&mut sealed, payload_len, 1).unwrap();

        let (header, payload) = sealed.split_at_mut(HEADER_BYTES);
        assert_eq!(new.open(header, payload, 1), Err(CryptoError::Unauthentic));
    }
}

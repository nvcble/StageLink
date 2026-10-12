/**
 * Binary audio packet format, v1.
 *
 * This is the only part of StageLink that is not JSON. It is defined here as
 * well as in `crates/audio-protocol` because StageLink Mobile needs an
 * authoritative, language-neutral description. `docs/protocol/audio-packets.md`
 * is the prose version of these constants — hand-written, not generated, so a
 * change here needs a change there. `tests/protocol` catches drift between the
 * two *implementations*; nothing catches drift from the document.
 *
 * Layout — 24-byte header, little-endian, followed by the payload:
 *
 * ```
 *  offset size field              notes
 *  ------ ---- -----------------  --------------------------------------------
 *       0    4 magic              ASCII "SLNK"
 *       4    1 version            AUDIO_PROTOCOL_VERSION
 *       5    1 flags              bitfield, see PacketFlags
 *       6    1 payloadFormat      0 = PCM_S16LE
 *       7    1 channels           always 1 (mono monitor mix)
 *       8    4 sessionId          truncated session identifier
 *      12    4 sequence           wraps at 2^32; gaps mean loss
 *      16    8 timestampUs        capture-clock microseconds, monotonic
 *      24    N payload            frames * 2 bytes, or AEAD ciphertext
 * ```
 *
 * The header is always transmitted in the clear, even on the encrypted
 * transport: a receiver must be able to detect loss and reorder packets before
 * it spends CPU on decryption, and the header is authenticated as AEAD
 * associated data so it still cannot be tampered with undetected.
 *
 * On the encrypted transport the payload is
 * `ChaCha20-Poly1305(key, nonce, plaintext, aad = header)` with a 16-byte
 * authentication tag appended, and the nonce built by {@link aeadNonce}. Keys
 * are per-session and derived during the paired control-channel handshake; they
 * never appear in a datagram.
 */

export const PACKET_MAGIC = 'SLNK' as const
export const PACKET_MAGIC_BYTES = Object.freeze([0x53, 0x4c, 0x4e, 0x4b]) // "SLNK"

export const PACKET_HEADER_BYTES = 24 as const

/** Poly1305 tag length appended to encrypted payloads. */
export const AEAD_TAG_BYTES = 16 as const

/** Header field offsets, for implementations that parse by hand. */
export const PACKET_OFFSETS = Object.freeze({
  magic: 0,
  version: 4,
  flags: 5,
  payloadFormat: 6,
  channels: 7,
  sessionId: 8,
  sequence: 12,
  timestampUs: 16,
  payload: 24,
})

export const PacketFlags = Object.freeze({
  /** Payload is AEAD-encrypted and carries a trailing tag. */
  Encrypted: 1 << 0,
  /** Final packet of a session; receivers should stop and drain. */
  EndOfStream: 1 << 1,
  /** Payload is digital silence; receivers may skip decode. */
  Silence: 1 << 2,
  /** Marks a discontinuity (capture glitch) before this packet. */
  Discontinuity: 1 << 3,
})

export const PayloadFormatCode = Object.freeze({
  PCM_S16LE: 0,
})

/**
 * Maximum payload we will emit. Keeps the datagram inside a 1500-byte MTU with
 * room for IPv4/UDP headers and the AEAD tag, so no packet is ever fragmented.
 */
export const MAX_PAYLOAD_BYTES = 1_400 as const

/** Total datagram size for a given frame count on a given transport. */
export function datagramBytes(frames: number, encrypted: boolean): number {
  return PACKET_HEADER_BYTES + frames * 2 + (encrypted ? AEAD_TAG_BYTES : 0)
}

/** Whether a configuration fits in one unfragmented datagram. */
export function fitsInMtu(frames: number, encrypted: boolean, mtu = 1_500): boolean {
  // 20-byte IPv4 header + 8-byte UDP header.
  return datagramBytes(frames, encrypted) + 28 <= mtu
}

export interface AudioPacketHeader {
  version: number
  flags: number
  payloadFormat: number
  channels: number
  sessionId: number
  sequence: number
  timestampUs: bigint
}

/**
 * Forward distance from `previous` to `current`, accounting for 32-bit wrap.
 * Returns a negative number for a reordered (late) packet.
 *
 * Used identically by the desktop and by receivers so loss statistics agree.
 */
export function sequenceDelta(previous: number, current: number): number {
  const span = 0x1_0000_0000
  let delta = (current - previous) % span
  if (delta < 0) delta += span
  // Treat the upper half of the space as "behind" rather than a huge jump.
  return delta > span / 2 ? delta - span : delta
}

/** Nonce length for ChaCha20-Poly1305 (the IETF 96-bit variant). */
export const AEAD_NONCE_BYTES = 12 as const

/**
 * The AEAD nonce for one packet, byte-exactly.
 *
 * ```
 *  offset size contents
 *  ------ ---- --------------------------------
 *       0    4 zero (reserved for a future epoch field)
 *       4    4 sessionId, little-endian
 *       8    4 sequence,  little-endian
 * ```
 *
 * Nonce reuse under ChaCha20-Poly1305 is catastrophic rather than merely weak:
 * it leaks the XOR of two plaintexts and allows the Poly1305 key to be
 * recovered, which means forged packets — injected audio in a performer's ears.
 * So uniqueness is argued explicitly, and the argument rests on the key, not on
 * this function:
 *
 * - Each session derives its own key via `HKDF-SHA256(ikm = control-channel
 *   shared secret, salt = sessionId_le, info = "stagelink audio v1")`, so a
 *   nonce only has to be unique *within* a session.
 * - `sequence` provides that, incrementing by one per packet. It wraps at 2^32,
 *   which at 200 packets/second is 248 days of continuous streaming; a sender
 *   reaching the wrap must take a fresh key rather than continue.
 * - `sessionId` is in the nonce so an accidental sequence restart cannot
 *   collide with the previous session's packets. It is a *truncated*
 *   identifier and may repeat, so it is not itself a guarantee.
 *
 * Mirrors `audio_protocol::aead_nonce`; `tests/protocol` pins them together.
 */
export function aeadNonce(sessionId: number, sequence: number): Uint8Array {
  const nonce = new Uint8Array(AEAD_NONCE_BYTES)
  const view = new DataView(nonce.buffer)
  // `>>> 0` so a value past 2^31 that arrived as a negative number from a
  // bitwise-derived source is written as the unsigned sessionId the sender
  // used, rather than relying on the reader to know ToUint32 coercion.
  view.setUint32(4, sessionId >>> 0, true)
  view.setUint32(8, sequence >>> 0, true)
  return nonce
}

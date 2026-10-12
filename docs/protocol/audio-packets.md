# StageLink audio packet format, version 1

The only non-JSON part of StageLink. This document is the contract for
StageLink Mobile and for any other receiver implementation.

Authoritative implementations:

- Rust: `crates/audio-protocol/src/packet.rs`
- TypeScript: `packages/shared-protocol/src/audio-packet.ts`

`tests/protocol` drives real packets through both so they cannot drift.

## Datagram layout

One audio frame per UDP datagram. 24-byte header, little-endian throughout,
followed by the payload.

```
 offset  size  field           notes
 ------  ----  --------------  --------------------------------------------
      0     4  magic           ASCII "SLNK" (0x53 0x4C 0x4E 0x4B)
      4     1  version         1
      5     1  flags           bitfield, see below
      6     1  payloadFormat   0 = PCM_S16LE
      7     1  channels        always 1 — the mono monitor mix
      8     4  sessionId       truncated session identifier
     12     4  sequence        wraps at 2^32
     16     8  timestampUs     capture-clock microseconds, monotonic
     24     N  payload         frames × 2 bytes, or AEAD ciphertext + tag
```

### Flags

| Bit | Value | Name | Meaning |
| --- | --- | --- | --- |
| 0 | `0x01` | `ENCRYPTED` | Payload is AEAD-encrypted and carries a trailing 16-byte tag |
| 1 | `0x02` | `END_OF_STREAM` | Final packet of a session; stop and drain |
| 2 | `0x04` | `SILENCE` | Payload is digital silence; decoding may be skipped |
| 3 | `0x08` | `DISCONTINUITY` | A capture glitch occurred before this packet; resynchronise rather than treating the gap as drift |

Unknown flag bits must be ignored, not rejected — that is how a minor protocol
revision stays compatible.

## The header is always in the clear

Even on the encrypted transport. Two reasons, both deliberate:

1. A receiver must be able to detect loss and reorder packets **before**
   spending CPU on decryption. Decrypting out of order, or decrypting a packet
   that turns out to be a duplicate, wastes the budget that matters most.
2. The header is bound into the AEAD as associated data, so it still cannot be
   altered undetected.

Nothing secret is ever in a header. `sessionId` is a truncated identifier, not a
capability; possessing it grants nothing.

## Encryption (production transport)

> **Status: implemented.** `audio_protocol::crypto` is the single
> implementation, used by the desktop sender and every receiver. Verified end
> to end against a release build: real audio from a mixer, sealed, delivered to
> two concurrent receivers, and written to a listenable WAV.
>
> Read the limitation below on revocation before relying on this: it is real,
> it is not a cipher problem, and it is the reason this section says what the
> scheme does *not* buy as well as what it does.

```
ciphertext || tag = ChaCha20-Poly1305(
    key   = HKDF-SHA256(
                ikm  = the join token, as ASCII hex,
                salt = sessionId as 4 bytes little-endian,
                info = "stagelink audio v1",
                len  = 32),
    nonce = aead_nonce(sessionId, sequence),   // 12 bytes, below
    aad   = the 24-byte header, exactly as transmitted,
    plaintext = PCM S16LE samples
)
```

### Where the key comes from

**The join token.** It is 256 bits from the OS generator, it is shown only as
a QR code on the operator's screen, and it is already what authorises a
receiver — so the set of people who can decrypt the audio is exactly the set
who were shown the code. Nothing is transmitted: both ends derive the key
independently from what the QR already established.

The session id is the HKDF salt, so two sessions under one token have
different keys. That is what makes the nonce's uniqueness argument hold when a
sequence number restarts at zero.

This closes the two threats that matter on a venue network:

- **Eavesdropping.** Nobody on the Wi-Fi can hear the mix without having been
  shown the code.
- **Forgery.** Nobody can inject audio, or forge an `END_OF_STREAM` marker and
  silence every performer at once — which a plaintext stream lets anyone on
  the network do.

### What it does not buy

Stated plainly, because these are real limitations and not theoretical ones.

**Revocation does not re-key.** Every receiver admitted under one join code
derives the same key, so a device the operator revokes with
`receivers.revoke` is disconnected and removed from the roster but can still
decrypt the stream it was already receiving. Rotating the join code does not
help: the key was derived at join time from the code that was current then.

Closing it needs a per-receiver key delivered over a control channel that is
itself encrypted and authenticated — an ephemeral X25519 exchange
authenticated by the join token, with the desktop's long-term key pinned via
the `fp` fingerprint that discovery already advertises. That is its own piece
of work and is tracked as an open item in [`pairing.md`](pairing.md).

In the meantime the operational answer is the honest one: **revoking a device
mid-service ejects it from the roster and stops the desktop sending to it, but
does not stop it listening to packets it can still see on the network.** To cut
it off completely, stop and restart the stream, which takes a new session id
and therefore a new key.

**No forward secrecy.** A token captured later decrypts a recording of that
session. The token is already a bearer credential with a five-minute life, so
this adds no exposure that joining did not already have.

A shared group key is what SRTP does for this shape of problem — one identical
stream to many listeners — so the structure is not unusual. The gap is the
revocation story, and it is the gap rather than the cipher.

`tag` is the 16-byte Poly1305 tag, appended after the ciphertext. The ciphertext
is the same length as the plaintext (ChaCha20 is a stream cipher), so an
encrypted datagram is exactly 16 bytes longer than its plaintext equivalent.

### Nonce layout

```
 offset size contents
 ------ ---- ----------------------------------------------
      0    4 zero — reserved for a future epoch field
      4    4 sessionId, little-endian
      8    4 sequence,  little-endian
```

Four *leading* zero bytes, not trailing: the varying part belongs in the
low-order half, as in TLS and in counter-style constructions generally. A
receiver must not reject a nonce on the basis of the reserved bytes beyond
building it as specified here.

**Test vectors** (hex, as transmitted):

| sessionId | sequence | nonce |
| --- | --- | --- |
| `0x00005EED` | `0` | `00000000 ED5E0000 00000000` |
| `0x12345678` | `0xFFFFFFFF` | `00000000 78563412 FFFFFFFF` |

These are asserted in `crates/audio-protocol/src/packet.rs`,
`packages/shared-protocol/test/protocol.test.ts`, and compared across the two
implementations in `tests/protocol/cross-implementation.test.ts`.

### Why the nonce is specified to the byte

Nonce reuse under ChaCha20-Poly1305 is catastrophic rather than merely weak. Two
packets sealed with the same key and nonce leak the XOR of their plaintexts,
and — far worse — allow the Poly1305 authentication key to be recovered, after
which an attacker can forge packets that a receiver will accept. A forged packet
in StageLink is injected audio in a performer's ears, live. So the uniqueness
argument is stated rather than assumed:

- **The key carries the guarantee.** One key per session means a nonce only has
  to be unique *within* a session.
- **`sequence` delivers that**, incrementing by exactly one per packet.
- **`sessionId` is defence in depth, not the guarantee.** It is a truncated
  identifier and may repeat; it is in the nonce so that an accidental sequence
  restart cannot collide with the previous session's packets.

**Senders must obey two rules:**

1. `sequence` wraps at 2^32. At the default 200 packets/second that is 248 days
   of continuous streaming, so it cannot occur inside a service — but a sender
   reaching the wrap **must** rekey rather than continue.
2. A new session **must** take a fresh `sessionId`, and therefore a fresh
   derived key, before `sequence` restarts at zero.

Keys are per-session, derived during the authenticated control-channel
handshake, and never appear in a datagram. A release build refuses the plaintext
transport outright (`StreamConfig::validate`); plaintext exists so the headless
test receiver can validate framing without key exchange.

## Frame geometry

Mono PCM S16LE at 48 kHz:

| Frame duration | Frames | Payload | Datagram (plaintext) | Datagram (encrypted) | Packets/sec |
| --- | --- | --- | --- | --- | --- |
| 2.5 ms | 120 | 240 B | 264 B | 280 B | 400 |
| **5 ms** (default) | **240** | **480 B** | **504 B** | **520 B** | **200** |
| 10 ms | 480 | 960 B | 984 B | 1000 B | 100 |

Frame durations are capped at 10 ms because one packet must fit a single
1500-byte datagram: 20 ms would be 1920 payload bytes and would be
IP-fragmented, and a fragmented audio packet loses all of its audio if any
fragment is dropped. `fits_in_mtu()` enforces this and the test suite asserts it
for every offered duration.

Egress for *n* receivers is roughly `n × 520 bytes × 200/sec` plus 42 bytes of
UDP/IP/Ethernet overhead per packet — about 0.9 Mbit/s per receiver, so eight
receivers sit comfortably inside a 100 Mbit link.

## Sequence numbers and loss

`sequence` increments by one per packet and **wraps at 2^32**. Subtracting
naively makes a wrap look like four billion lost packets. Use the shared
helper:

```rust
// Rust
audio_protocol::sequence_delta(previous, current) // -> i64, negative if late
```

```ts
// TypeScript
sequenceDelta(previous, current) // -> number, negative if late
```

A receiver should classify each arrival as first, in-order, a gap, reordered, a
duplicate, or a session change. `audio_protocol::SequenceTracker` implements
exactly this and is allocation-free, so a receiver can run it on its audio
thread. Using it means desktop and receiver loss statistics agree.

A **session change** (`sessionId` differs) resets every statistic: sequence
numbers from a previous session are meaningless, and the audio is not
continuous, so buffered audio from the old session should be discarded.

## Timestamps

`timestampUs` is capture-clock microseconds, monotonic within a session, derived
from the frame count. It is useful for jitter measurement and ordering. It is
**not** wall-clock time and must not be compared across a service restart.

## What a receiver must validate

Before trusting a payload:

1. `magic == "SLNK"` — anything else is stray traffic on the port and must be
   dropped silently, not decoded.
2. `version == 1` — refuse other versions rather than guessing.
3. `channels == 1` — StageLink is mono end to end.
4. `payloadFormat` is known.
5. Payload length is within bounds, and for plaintext, a whole number of
   16-bit samples. A ragged payload means a truncated datagram.
6. For encrypted packets, there is room for the 16-byte tag.

`parse_header()` performs all six. A receiver on a shared venue network will see
unrelated traffic and must survive it — the headless test receiver counts
rejected datagrams and carries on rather than aborting.

## Verifying an implementation

```bash
# Drive the full analysis path against synthetic packets, including a
# deliberately dropped one, and write a listenable WAV.
cargo run -p test-receiver --bin stagelink-test-receiver -- \
  --self-test --output ./received.wav
```

This asserts sequencing, loss accounting, PCM round-trip accuracy and WAV
output. It is the check that makes "streaming works" falsifiable.

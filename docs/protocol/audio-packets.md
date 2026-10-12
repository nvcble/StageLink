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

```
ciphertext || tag = ChaCha20-Poly1305(
    key   = per-session key from the paired control handshake,
    nonce = derived from sessionId || sequence,
    aad   = the 24-byte header,
    plaintext = PCM S16LE samples
)
```

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

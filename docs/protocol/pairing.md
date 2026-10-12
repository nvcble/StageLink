# Discovery and pairing

The contract StageLink Mobile implements to find a desktop, pair with it, and
join a session.

> **Phase status.** The service type, TXT-record keys and the payload grammar
> below are **frozen** — they are what a receiver will browse for and parse.
> Publishing the mDNS record and performing the pairing handshake is Phase 5
> work. `receivers.createPairingCode` and the approval methods currently return
> `NOT_IMPLEMENTED` naming that phase.

## Principles

1. **Discovery is a convenience, never an authority.** A receiver that finds the
   desktop by mDNS still has to pair and authenticate. Nothing is trusted
   because it answered a multicast query.
2. **Nothing secret is ever advertised.** No tokens, no keys, no pairing codes
   in a TXT record. A fingerprint is a hash, not a secret.
3. **The operator decides.** A device joins because somebody standing at the
   laptop approved it, having compared a fingerprint shown on both screens.
4. **A revoked device is remembered.** Its record is kept so it cannot return as
   an unrecognised newcomer.

## Service discovery

DNS-SD over mDNS.

```
service type : _stagelink._udp.local.
instance     : operator-configurable, default "StageLink Core", ≤ 63 bytes
```

TXT record keys (all values are ASCII strings):

| Key | Meaning |
| --- | --- |
| `cpv` | Control protocol version, e.g. `1.0.0` |
| `apv` | Audio protocol version, e.g. `1` |
| `cport` | Control channel TCP port |
| `sport` | Audio UDP port |
| `name` | Human-readable server name |
| `fp` | Fingerprint of the desktop's long-term public key |
| `pair` | `1` if pairing is required (always `1` in a release build) |

A receiver should **pin `fp`** after a successful pairing. If it changes, the
receiver is talking to a different desktop (or a machine pretending to be the
one it trusted) and must require fresh approval rather than reconnecting
silently.

If discovery is unavailable — many venue networks block multicast — the operator
can read the address off the Network page and the receiver can connect directly
to `cport`. Discovery failing must never be a dead end.

## Pairing

### 1. The operator mints a code

`receivers.createPairingCode` returns a 6-digit code valid for 120 seconds, plus
a QR payload encoding the same information:

```
stagelink://pair?host=10.0.0.1&cport=45871&sport=45872&code=418902&fp=AB%3ACD%3AEF%3A01&cpv=1.0.0
```

The code is short-lived and single-use. It proves the person holding the phone is
in the room with the operator; it is not a cryptographic secret and the protocol
does not treat it as one.

### 2. The receiver requests pairing

Over the control channel, presenting its long-term public key and the code.

```jsonc
{ "v": 1, "kind": "request", "id": "1", "method": "pair.request",
  "params": {
    "deviceName": "Pixel 8",
    "platform": "android",
    "platformVersion": "15",
    "appVersion": "0.1.0",
    "publicKey": "<hex Ed25519 public key>",
    "code": "418902"
  } }
```

Everything here except the observed source address is **self-reported and
untrusted**. The desktop surfaces it that way: the Receivers page labels the
source address as the one field it can vouch for.

### 3. The operator approves

The desktop raises `receivers.pairingRequest`, which includes a short
`keyFingerprint` derived from the presented public key. The receiver shows the
same fingerprint. The operator compares them and approves.

`presentedValidCode: false` means the device connected without a valid code —
possible, and shown prominently, because an approval is then resting on the
fingerprint comparison alone.

### 4. Session keys

On approval, desktop and receiver derive a per-session key from the
authenticated handshake. That key encrypts the audio payloads
(ChaCha20-Poly1305, see [`audio-packets.md`](audio-packets.md)). Keys are never
transmitted and never written to the SQLite database — the desktop's own key
material goes to the OS-protected store (DPAPI).

### 5. Reconnection

An approved receiver reconnects without operator interaction by proving
possession of its long-term key. Two things can change that:

- the operator enabled **Require approval every time a receiver joins**, or
- the desktop's fingerprint changed, in which case the receiver must insist on
  fresh approval.

## Receiver telemetry

Approved receivers report quality every 2 s and heartbeat every 1 s. Five
consecutive missed heartbeats move a receiver to `timedOut` and stop audio being
queued for it.

```jsonc
{ "v": 1, "kind": "event", "event": "receiver.telemetry",
  "payload": {
    "source": "receiver-reported",
    "reportedAt": 1791772004653,
    "packetsReceived": 12000, "packetsLost": 7, "lossPercent": 0.06,
    "jitterMs": 2.4, "bufferHealthMs": 42.0, "underruns": 0,
    "playbackState": "playing", "outputLatencyMs": 24.0
  } }
```

The `source` field is mandatory and literal. The desktop cannot verify any of
this, and the interface says so wherever it is shown.

## Receiver obligations

A conforming receiver must:

1. Validate every audio packet header before trusting the payload — see
   [`audio-packets.md`](audio-packets.md).
2. Use `sequenceDelta` semantics for loss accounting, so its figures agree with
   the desktop's.
3. Treat a `sessionId` change as a discontinuity and discard buffered audio.
4. Honour `END_OF_STREAM` by draining and stopping, not by waiting out its
   jitter buffer.
5. Duplicate the mono signal to both ears. **Never synthesise stereo.**
6. Apply its own volume, EQ, mute and hearing-safety limiter locally. The
   desktop sends one identical mix to everyone and has no per-receiver
   processing.
7. Pin the desktop's key fingerprint and require fresh approval if it changes.
8. Survive unrelated traffic on its ports without aborting.

## Hearing safety

This is a system that puts audio directly into people's ears, often louder than
they would choose, for an hour at a time. The limiter is a **receiver-side**
responsibility and is not optional: the desktop cannot know what earphones are
attached or how loud the output stage is. A receiver implementation without a
limiter is not conforming.

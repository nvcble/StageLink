# Joining a session

How StageLink Mobile finds a desktop and joins it.

> **Phase status.** The QR payload grammar and the join-code semantics below are
> **frozen** — build against them. Minting a code works today; accepting the
> connection needs the control server from Phase 3, so `receivers.rename`,
> `disconnect` and `revoke` currently return `NOT_IMPLEMENTED`.

## The model: scanning is the authorisation

There is **no approval step**. The desktop displays a QR code; a device that
scans it and presents the token inside is authorised and joins immediately.

The reasoning: the operator is holding a laptop on a stage with a band waiting.
An approval queue means they have to look at the screen and tap for each of
eight people, exactly when they are busiest. Showing a code and letting people
scan it moves the decision to the moment they chose to display it.

What this trades away, stated plainly: **the token is a bearer credential.**
Anyone who photographs the screen can join while the code lives. Two mitigations
are built in, and a third is operational:

| Mitigation | Mechanism |
| --- | --- |
| Codes expire | `JOIN_CODE_TTL_MS` = 5 minutes |
| Codes rotate | `receivers.createJoinCode` replaces the previous token immediately |
| Devices can be ejected | `receivers.revoke` withdraws one device regardless of code state |

Only show the code while people are joining.

## Service discovery

Optional. A receiver that has a QR does not need discovery at all — the payload
carries the address. Discovery exists for reconnecting a device that has joined
before and whose desktop may have moved to a new address.

```
service type : _stagelink._udp.local.
instance     : operator-configurable, default "StageLink Core", ≤ 63 bytes
```

| TXT key | Meaning |
| --- | --- |
| `cpv` | Control protocol version, e.g. `1.0.0` |
| `apv` | Audio protocol version, e.g. `1` |
| `cport` | Control channel TCP port |
| `sport` | Audio UDP port |
| `name` | Human-readable server name |
| `fp` | Fingerprint of the desktop's long-term public key |
| `pair` | `1` if a join code is required (always `1`) |

Nothing secret is ever advertised: no tokens, no keys, no join codes. A
fingerprint is a hash.

Many venue networks block multicast, so discovery failing must never be a dead
end — fall back to the address from the QR, or to manual entry.

## The QR payload

```
stagelink://join?h=<host>&c=<controlPort>&s=<streamPort>&t=<token>&v=<controlProtocolVersion>
```

| Key | Type | Meaning |
| --- | --- | --- |
| `h` | IPv4 string | Desktop address to connect to |
| `c` | `u16` | Control channel port |
| `s` | `u16` | Audio UDP port |
| `t` | 64 hex chars | Join token — 256 bits from the OS generator |
| `v` | semver | Control protocol version the desktop speaks |

Keys are one character on purpose. A payload under ~150 bytes keeps the QR at a
low version, which is the difference between scanning it from across a stage and
having to walk up to the laptop.

A receiver that cannot parse the payload, or whose `v` major differs from its
own, should say so rather than attempting the connection.

Example:

```
stagelink://join?h=10.0.0.1&c=45871&s=45872&t=9f2c...64hex...&v=1.0.0
```

## Joining

### 1. Scan and connect

Parse the payload, open the control channel to `h:c`, and present the token
along with the device's self-description.

```jsonc
{ "v": 1, "kind": "request", "id": "1", "method": "join.request",
  "params": {
    "token": "9f2c…",
    "deviceName": "Pixel 8",
    "platform": "android",
    "platformVersion": "15",
    "appVersion": "0.1.0",
    "publicKey": "<hex Ed25519 public key>"
  } }
```

Everything except the observed source address is self-reported and untrusted.
The desktop stores `publicKey` so the device can reconnect later without a fresh
code.

### 2. Admitted, or not

The desktop compares the token in constant time and checks it has not expired.
On success the receiver is `approved` and appears in the roster immediately.

| Failure | Error code |
| --- | --- |
| Wrong or expired token | `PAIRING_REJECTED` |
| Receiver limit reached | `RECEIVER_LIMIT_REACHED` |
| This device was revoked | `PAIRING_REJECTED` |

A rejected receiver should tell the user to ask for a fresh code, not retry in a
loop — the token will not start working.

### 3. Session keys

On admission, desktop and receiver derive a per-session key from the
authenticated handshake. That key encrypts audio payloads
(ChaCha20-Poly1305, see [`audio-packets.md`](audio-packets.md)). Keys are never
transmitted and never written to SQLite; the desktop's own key material lives in
the OS-protected store.

### 4. Reconnecting

A device that has joined keeps its authorisation until revoked, so it reconnects
by proving possession of its long-term key — no new QR needed. It must require a
fresh code if:

- the operator revoked it, or
- the desktop's `fp` changed, meaning this is a different machine.

**Pin `fp` after joining.** A changed fingerprint is the one signal that
distinguishes "my desktop got a new IP" from "something else is answering".

## Receiver telemetry

Admitted receivers report quality every 2 s and heartbeat every 1 s. Five
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
it, and the interface says so wherever it is shown.

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
7. Pin the desktop's key fingerprint and require a fresh join code if it changes.
8. Survive unrelated traffic on its ports without aborting.
9. Never persist a join token after admission. It is single-purpose and
   short-lived; the long-term key is what identifies the device afterwards.

## Hearing safety

This system puts audio directly into people's ears, often louder than they would
choose, for an hour at a time. The output limiter is a **receiver-side**
responsibility and is not optional: the desktop cannot know what earphones are
attached or how loud the output stage is. A receiver implementation without a
limiter is not conforming.

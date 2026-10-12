# Joining a session

How StageLink Mobile finds a desktop and joins it.

> **Phase status.** The QR payload grammar and the join-code semantics below are
> **frozen** — build against them. Minting a code works today; accepting the
> connection needs the control server from Phase 5, so `receivers.rename`,
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

**The SRV record's port is the audio UDP port**, matching the `_udp` service
type. Read `cport` from the TXT record for the control channel; do not connect
it to the SRV port. The duplication between the SRV port and `sport` is
deliberate — SRV must carry a port, and TXT is what a receiver is told to use.

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

Parse the payload, open the control channel to `h:c`, and send a `hello` — the
same first message the desktop's own local client sends, with the join token in
`token` and the device's self-description in `receiver`.

```jsonc
{ "v": 1, "kind": "hello",
  "token": "9f2c…",
  "client": "stagelink-mobile/0.1.0",
  "controlProtocolVersion": "1.0.0",
  "receiver": {
    "deviceName": "Pixel 8",
    "platform": "android",
    "platformVersion": "15",
    "appVersion": "0.1.0",
    "publicKey": "<hex Ed25519 public key>"
  } }
```

**There is no `join.request` method.** An earlier draft of this document showed
one, which could never have worked: the envelope contract requires `hello` to be
the first message on a connection and to carry the credential, so a second
credential-bearing frame could not have been first. Joining is a `hello` with a
`receiver` block — one round trip, and the token arrives in the field that is
already compared in constant time.

Everything in `receiver` except the observed source address is self-reported and
untrusted; the desktop trims and clamps each string on the way in. The desktop
stores `publicKey` so the device can reconnect later without a fresh code.

### 2. Admitted, or not

The desktop compares the token in constant time and checks it has not expired.
On success it replies with a `welcome` and the receiver is `approved` and in the
roster immediately. The `welcome` carries what the receiver cannot otherwise
know:

```jsonc
{ "v": 1, "kind": "welcome",
  "serviceVersion": "0.1.0",
  "controlProtocolVersion": "1.0.0",
  "audioProtocolVersion": 1,
  "availableAudioApis": ["wasapi"],
  "pid": 12345,
  "keyFingerprint": "AB:CD:EF:01",
  "stream": {
    "sessionId": 1589741,
    "sampleRate": 48000,
    "frameDurationMs": 5,
    "payloadFormat": "pcm_s16le",
    "transport": "udp-encrypted",
    "streamPort": 45872,
    "framesPerPacket": 240
  } }
```

**`stream.sampleRate` is the only place the rate is communicated.** The packet
header carries `channels` and `payloadFormat` but no rate, and the sender
permits 8–192 kHz, so a receiver that assumed 48 kHz and got 44.1 kHz would play
a semitone and a half sharp — in a performer's ears, live. A conforming receiver
must also derive the rate independently as
`framesPerPacket / (Δ timestampUs / 1e6)` across two consecutive in-order
packets and refuse to play if the two disagree. `stream` is re-sent whenever
`sessionId` changes, because the rate may change with it.

On failure the desktop closes the connection rather than answering: there is no
correlation id on a `hello`, so there is nothing to respond to. A rejected
receiver sees the socket close after its `hello`, and should tell the user to
ask for a fresh code rather than retry in a loop — the token will not start
working. The error codes below are what the desktop logs and what the operator
sees; they are not delivered to the receiver.

| Failure | Logged as |
| --- | --- |
| Wrong or expired token | `PAIRING_REJECTED` |
| Receiver limit reached | `RECEIVER_LIMIT_REACHED` |
| This device was revoked | `PAIRING_REJECTED` |
| Incompatible control protocol major | `PROTOCOL_MISMATCH` |

> **Open item.** A closed socket cannot distinguish "the session is full" from
> "that code expired", and those call for different words on the phone. Giving
> the receiver the reason needs a server→receiver message defined somewhere
> other than the renderer's event registry, which is a control-server design
> decision and is deliberately left open rather than guessed at here.

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

## Heartbeats and receiver telemetry

Admitted receivers heartbeat every 1 s (`RECEIVER_HEARTBEAT_INTERVAL_MS`) and
report quality every 2 s (`RECEIVER_TELEMETRY_INTERVAL_MS`). Five consecutive
missed heartbeats (`RECEIVER_HEARTBEAT_MISS_LIMIT`) move a receiver to
`timedOut` and stop audio being queued for it.

**Both are `request` methods, not events.** An earlier draft showed them as
client-sent events, which the control server discards by design — events travel
server→client only. As requests they also get an acknowledgement, which is what
makes a heartbeat worth sending: the ack confirms liveness in both directions
and hands the receiver its round-trip time without any local bookkeeping.

```jsonc
// -> receiver.heartbeat, every 1 s
{ "v": 1, "kind": "request", "id": "h-412", "method": "receiver.heartbeat",
  "params": { "sentAt": 1791772004653 } }

// <- the ack
{ "v": 1, "kind": "response", "id": "h-412", "ok": true,
  "result": { "at": 1791772004661, "sentAt": 1791772004653, "missLimit": 5 } }
```

```jsonc
// -> receiver.telemetry, every 2 s
{ "v": 1, "kind": "request", "id": "t-77", "method": "receiver.telemetry",
  "params": {
    "reportedAt": 1791772004653,
    "packetsReceived": 12000, "packetsLost": 7, "lossPercent": 0.06,
    "jitterMs": 2.4, "bufferHealthMs": 42.0, "underruns": 0,
    "playbackState": "playing", "outputLatencyMs": 24.0,
    "resampleRatioPpm": -14.2
  } }
```

`source: "receiver-reported"` and `receivedAt` are **not sent** — the desktop
stamps both on arrival, so a receiver cannot forge its own provenance. The
desktop cannot verify any of the rest, and the interface says so wherever it is
shown.

`resampleRatioPpm` is where the receiver's drift compensation has settled, or
null if it does not compensate. Report it: a value sitting at its clamp for
minutes is the only field that reveals clock drift exceeding budget before
somebody complains about a dropout.

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
10. Take the sample rate from `welcome.stream.sampleRate`, **and** check it
    against the rate implied by the packet timestamps. Refuse to play on
    disagreement rather than playing the mix at the wrong pitch.
11. Compensate for clock drift between the desktop's capture clock and its own
    DAC. At ±20 ppm each, the worst case of 40 ppm accumulates **216 ms** over a
    90-minute service — enough to starve or overflow any fixed buffer several
    times over. Report the correction as `resampleRatioPpm`.

## Hearing safety

This system puts audio directly into people's ears, often louder than they would
choose, for an hour at a time. The output limiter is a **receiver-side**
responsibility and is not optional: the desktop cannot know what earphones are
attached or how loud the output stage is. A receiver implementation without a
limiter is not conforming.

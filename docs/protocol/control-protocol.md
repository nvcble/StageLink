# StageLink control protocol, version 1.0.0

The control plane: everything that is not audio samples. Used on two links:

1. **Electron main ↔ Rust service** — a Windows named pipe carrying NDJSON.
2. **Desktop ↔ StageLink Mobile** — the same envelope and method vocabulary over
   an authenticated WebSocket (Phase 5).

Authoritative definition: `packages/shared-protocol/src/`. The Rust side mirrors
it in `crates/desktop-ipc/src/codec.rs`, and `tests/protocol` drives real
payloads through both.

## Versioning

| Version | Covers | Bumped when |
| --- | --- | --- |
| `IPC_ENVELOPE_VERSION` = `1` | The JSON framing | The envelope shape changes |
| `CONTROL_PROTOCOL_VERSION` = `1.0.0` | Methods, events, payloads | Any contract change |
| `AUDIO_PROTOCOL_VERSION` = `1` | The binary packet header | The wire format changes |

**Compatibility rule.** The same MAJOR is required; a differing MAJOR is refused
at handshake. A newer MINOR on either side is tolerated, and **unknown fields
must be ignored rather than rejected** — that is what lets a newer receiver talk
to an older desktop.

## Framing

Newline-delimited JSON. One object per line, UTF-8, no embedded newlines
(`serde_json` and `JSON.stringify` both escape control characters, so a message
containing a newline cannot break the framing). Maximum 1 MiB per line.

## Handshake

The client speaks first and must authenticate before anything else.

```jsonc
// client → service
{ "v": 1, "kind": "hello",
  "token": "<per-launch secret>",
  "client": "stagelink-desktop/0.1.0",
  "controlProtocolVersion": "1.0.0" }

// service → client
{ "v": 1, "kind": "welcome",
  "serviceVersion": "0.1.0",
  "controlProtocolVersion": "1.0.0",
  "audioProtocolVersion": 1,
  "availableAudioApis": ["wasapi"],
  "pid": 12345 }
```

The token is generated fresh on every service launch (192 bits), passed on the
service's command line, and compared in **constant time**. A failed handshake
drops the connection rather than allowing a retry loop. The pipe's DACL already
admits only the current user's SID, so the token is defence in depth.

**A receiver uses this same handshake.** Its join token goes in `token` and its
self-description in an added `receiver` block, and the `welcome` comes back with
an added `stream` block carrying the audio parameters — including the sample
rate, which appears nowhere else. Both additions are optional fields on the
existing messages, which is why a receiver needs no join method of its own; see
[`pairing.md`](pairing.md). The local pipe refuses a `hello` carrying
`receiver`, because `reject_remote_clients(true)` means no receiver can be on
the other end of it.

`availableAudioApis` reports what the running binary can *actually* use, not what
it was compiled against — ASIO is absent unless both a backend and a driver are
present.

## Requests and responses

```jsonc
// request
{ "v": 1, "kind": "request", "id": "7", "method": "audio.getStatus", "params": {} }

// success
{ "v": 1, "kind": "response", "id": "7", "ok": true, "result": { "state": "running" } }

// failure
{ "v": 1, "kind": "response", "id": "7", "ok": false,
  "error": { "code": "AUDIO_DEVICE_NOT_FOUND", "message": "…",
             "detail": "…", "retryable": false, "field": "params.deviceId" } }
```

`id` correlates the pair and is unique per connection. Every method has a
declared timeout; exceeding it yields `TIMEOUT` with `retryable: true`.

### The method registry is a whitelist

`METHODS` in `packages/shared-protocol/src/methods.ts` is the complete list of
operations. There is no generic `invoke(command, args)`. A name that is not
registered returns `UNKNOWN_METHOD` — it is not forwarded anywhere.

Each entry carries a params schema *and* a result schema. The Electron main
process validates params before the request reaches the service, and validates
the result before it reaches a Pinia store.

| Method | Mutates | Purpose |
| --- | --- | --- |
| `audio.listDevices` | | Enumerate Windows capture endpoints |
| `audio.selectDevice` | ● | Open a device, capture one mono channel |
| `audio.releaseDevice` | ● | Close the capture device |
| `audio.getStatus` | | Capture state, negotiated format, device health |
| `audio.test` | | Listen to the input and report what is arriving |
| `stream.start` | ● | Begin sending audio to authorised receivers |
| `stream.stop` | ● | Stop sending, draining unless `immediate` |
| `stream.getStatus` | | Stream state and desktop-measured counters |
| `receivers.list` | | Known receivers and the current join code |
| `receivers.rename` | ● | Set the operator-facing label |
| `receivers.disconnect` | ● | Drop from the current session |
| `receivers.revoke` | ● | Withdraw authorisation permanently |
| `receivers.createJoinCode` | ● | Mint a join code and QR payload, rotating the previous one |
| `receiver.heartbeat` | ● | **Called by a receiver.** Liveness, acked with the desktop clock |
| `receiver.telemetry` | ● | **Called by a receiver.** Its own quality figures, unverified |
| `network.getStatus` | | Adapters, bound address, discovery, warnings |
| `network.selectAdapter` | ● | Choose the streaming adapter and address |
| `network.checkFirewall` | | Report on firewall rules. Read-only. |
| `session.getCurrent` | | The active session, or `null` |
| `session.history` | | Paginated past sessions |
| `session.events` | | Operational event log for one session |
| `diagnostics.getSnapshot` | | One-shot snapshot with per-metric provenance |
| `diagnostics.subscribe` | ● | Turn periodic diagnostics pushes on or off |
| `settings.get` | | Persisted, fully-defaulted settings |
| `settings.update` | ● | Validate and persist a patch |
| `service.getInfo` | | Versions and supervision state |
| `service.ping` | | Liveness probe |
| `service.shutdown` | ● | Orderly shutdown with a drain grace period |

### Error codes

`INVALID_REQUEST`, `UNKNOWN_METHOD`, `SERVICE_UNAVAILABLE`, `TIMEOUT`,
`PROTOCOL_MISMATCH`, `UNAUTHORIZED`, `AUDIO_DEVICE_NOT_FOUND`,
`AUDIO_FORMAT_UNSUPPORTED`, `AUDIO_CHANNEL_OUT_OF_RANGE`,
`AUDIO_CAPTURE_FAILED`, `AUDIO_DEVICE_DISCONNECTED`, `STREAM_NO_INPUT`,
`STREAM_BIND_FAILED`, `STREAM_TRANSPORT_REFUSED`, `INVALID_STATE_TRANSITION`,
`OPERATION_IN_PROGRESS`, `RECEIVER_NOT_FOUND`, `RECEIVER_LIMIT_REACHED`,
`PAIRING_REJECTED`, `NETWORK_ADAPTER_UNAVAILABLE`, `SETTINGS_INVALID`,
`STORAGE_FAILURE`, `NOT_IMPLEMENTED`, `INTERNAL`.

Methods belonging to a later phase return `NOT_IMPLEMENTED` with the phase named
in `detail`, never a plausible-looking empty success. `NOT_IMPLEMENTED` is a
declared code for exactly that reason: an undeclared one fails the renderer's
enum validation and collapses into `INTERNAL: "Unknown error"`, turning an
honest "not built yet" into an apparent crash.

## Events

Pushed service → client only. A client-sent event is discarded, which is why
receiver heartbeats and telemetry are requests rather than the events an earlier
draft described.

Fire-and-forget: a full client queue drops the event rather than stalling
telemetry, and every piece of state an event carries is also available as a
request, so a dropped push is always recoverable.

```jsonc
{ "v": 1, "kind": "event", "event": "audio.levels",
  "payload": { "at": 1791772004653, "rmsDb": -18.2, "peakDb": -6.1,
                "peakHoldDb": -6.1, "clipping": false, "clipCount": 0,
                "windowFrames": 1920 },
  "at": 1791772004653 }
```

| Event | Cadence | Payload |
| --- | --- | --- |
| `audio.levels` | ~25 Hz | Metering frame |
| `audio.status` | on change | Capture state, format, device health |
| `stream.status` | on change | Stream state and counters |
| `receivers.changed` | on change | The full receiver roster |
| `receivers.joinCode` | on mint/rotate/expiry | The current join code, or null |
| `network.status` | on change | Adapters, address, warnings |
| `session.state` | on transition | The session |
| `session.event` | on append | One operational log entry |
| `diagnostics.snapshot` | ~2 Hz while subscribed | Full diagnostics |
| `service.state` | on change | Supervision state |
| `service.log` | as logged | Forwarded service log line |
| `service.error` | on fault | An operational error |

**No event carries audio samples.** `audio.levels` is throttled in Rust at
roughly a quarter of the audio buffer rate, so the IPC channel never sees
per-buffer traffic. The protocol tests assert that no event name matches
`sample|pcm|buffer|frame`.

On connect, the service immediately pushes a snapshot of every piece of state,
so a reconnecting client never has to poll.

## Reconnection

The service accepts clients **sequentially** but indefinitely. A client that
disconnects — Electron restarting, a renderer reload — does not stop audio and
does not stop the service. The next client presenting a valid token is welcomed
and receives a full snapshot.

Client-side, the Electron supervisor retries with the backoff in
`SERVICE_RESTART_BACKOFF_MS` (250 ms → 10 s over five attempts), resetting the
count after a run of at least `SERVICE_HEALTHY_RUN_MS` (60 s). After five
failures it reports `failed` and stops retrying rather than spinning.

## Telemetry provenance

Every diagnostics figure carries a `source`:

```jsonc
{ "value": 10.0, "unit": "ms", "source": "measured",
  "note": "Capture period granted by the audio driver." }
```

- `measured` — counted or timed by the service on this machine.
- `estimated` — derived arithmetically from configuration and measured values.
- `receiver-reported` — sent by a receiver; unverified by definition.

Receiver telemetry additionally carries a literal
`"source": "receiver-reported"` discriminator inside its own object, so it
cannot be mistaken for a desktop measurement anywhere downstream.

An unknown value is `null`, never `0`.

## Latency

`diagnostics.getSnapshot` returns a breakdown, not a total:

| Field | Source | Meaning |
| --- | --- | --- |
| `captureBufferMs` | measured | Capture period the driver granted |
| `packetFrameMs` | estimated | Configured packet frame duration |
| `sendQueueMs` | measured | Time audio spent queued |
| `desktopSubtotalMs` | estimated | Sum of the three above |
| `controlChannelRttMs` | measured | **Not audio latency** |
| `receiverReportedOutputMs` | receiver-reported | The receiver's own claim |

There is deliberately no end-to-end figure. Wi-Fi airtime, the receiver's
jitter buffer and the mobile audio output path are not measurable from here, and
a subtotal missing a term reads as optimistically low — so if any term of the
subtotal is unknown, the subtotal is `null` too. See
[`../latency-measurement.md`](../latency-measurement.md).

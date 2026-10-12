# StageLink architecture

## The signal, and what follows from it

StageLink receives **one combined mono monitor mix** from the FOH console's
AUX/monitor send. That single fact determines most of the design.

```
FOH mixer
  └─ AUX / monitor send
       └─ USB audio interface  ──┐
                                 │  one physical input channel
       Windows desktop ──────────┘
         └─ StageLink Rust audio service
              ├─ WASAPI capture (one mono channel, f32)
              ├─ metering (peak / RMS / peak-hold / clip)
              ├─ packetiser (PCM S16LE, 5 ms frames)
              └─ UDP transmission, one copy per receiver
                   └─ Ethernet
                        └─ dedicated Wi-Fi access point
                             └─ StageLink Mobile receivers
                                  └─ wired in-ear monitors
```

There are **no separable instrument channels in the signal**, so StageLink has
no mixer, no per-instrument faders and no panning. Every receiver gets the same
mono audio and may duplicate it to both ears. All personal adjustment —
volume, EQ, mute, hearing-safety limiting, presets — happens on the receiver,
where the person wearing the earphones can reach it.

No part of the live audio path requires internet access.

## Processes

Three processes, with deliberately different jobs.

| Process | Language | Owns |
| --- | --- | --- |
| **Electron main** | TypeScript | Desktop lifecycle, window, tray, custom protocol, supervising the audio service, validating every control request |
| **Nuxt renderer** | TypeScript / Vue | Drawing the interface. Nothing else. |
| **Audio service** | Rust | Capture, metering, packetisation, transmission, receivers, persistence |

```
┌──────────────────────── Electron main process ────────────────────────┐
│                                                                       │
│  ┌─ BrowserWindow (sandboxed, contextIsolated) ──────────────────┐    │
│  │  Nuxt 4 renderer                                             │    │
│  │    components → composables → Pinia stores                   │    │
│  │                                   │                          │    │
│  │                    window.stagelink (preload bridge)         │    │
│  └───────────────────────────────────┼──────────────────────────┘    │
│                                      │ 8 fixed IPC channels           │
│                          ┌───────────▼───────────┐                    │
│                          │  method + params      │                    │
│                          │  validated against    │                    │
│                          │  the shared registry  │                    │
│                          └───────────┬───────────┘                    │
│                            ServiceSupervisor                          │
└──────────────────────────────────────┼────────────────────────────────┘
                                       │ Windows named pipe, NDJSON,
                                       │ user-only DACL + token handshake
┌──────────────────────────────────────▼────────────────────────────────┐
│                       Rust audio service (child process)              │
│                                                                       │
│   capture thread          telemetry task        control task          │
│   (Pro Audio prio)        (25 Hz meters)        (requests, SQLite)    │
│        │                       ▲                      ▲               │
│        │  lock-free ring       │  atomics             │               │
│        └───────────────────────┴──────────────────────┘               │
└───────────────────────────────────────────────────────────────────────┘
```

**Audio never crosses a process boundary as samples.** The control channel
carries requests, responses and small telemetry events only. Metering is
computed in Rust from every captured sample and pushed at 25 Hz — about a
quarter of the audio buffer rate — so the IPC channel never sees per-buffer
traffic.

## Why the renderer can reload without stopping audio

The audio service is a child of the Electron main process, not of the window.
Reloading the renderer (or an HMR update in development) drops the event
subscriptions and nothing else: capture keeps running, and the service accepts
the next client that presents the handshake token, pushing a full state snapshot
on connect so the new page resynchronises without polling.

Quitting the application is different, and intentionally so — see
[Shutdown policy](#shutdown-policy).

## The real-time boundary

Exactly one thread in StageLink has real-time constraints: the WASAPI capture
thread in `crates/audio-capture/src/capture_thread.rs`. It requests the Windows
"Pro Audio" scheduling class (and reports honestly when that is refused), and it
obeys these rules:

- **No allocation.** Every buffer is allocated at start-up and reused.
- **No locks** that a non-real-time thread can hold.
- **No logging** in the loop; faults go into atomics.
- **No database or socket calls.**
- **Never blocks.** A full ring drops samples and counts an overrun, because
  stalling the driver callback is worse than losing a few milliseconds.

Everything else — telemetry, the control channel, SQLite, the UI — runs on
ordinary threads and cannot influence audio timing no matter how slow it is.

The hand-off is `audio-core`'s lock-free SPSC ring plus atomic counters. The
`ServiceState` type that owns the database is deliberately not reachable from
the capture thread at all.

## Crate layout

| Crate | Responsibility |
| --- | --- |
| `audio-core` | Real-time-safe primitives: formats, conversion, metering, lock-free ring |
| `audio-protocol` | Binary audio packet format, sequencing, transport policy |
| `audio-capture` | WASAPI enumeration, format negotiation, the capture thread |
| `stream-server` | Packetiser, per-receiver bounded queues, stream config validation |
| `receiver-session` | Session state machine, live receiver roster |
| `device-discovery` | Network adapter enumeration, warnings, mDNS contract |
| `telemetry` | Metrics with explicit provenance, latency accounting, system probe |
| `persistence` | SQLite migrations, typed repositories, DPAPI secret store |
| `desktop-ipc` | Authenticated named-pipe control channel |
| `stagelink-service` | The binary: wires it together, dispatches control methods |
| `test-receiver` | Headless receiver that proves audio actually arrives |

## One protocol definition

`packages/shared-protocol` is the single source of truth for the control
protocol. It is consumed by:

- the **Nuxt renderer**, for compile-time types,
- the **Electron main process**, for runtime validation of params *and* results,
- the **generated documentation** for StageLink Mobile.

The Rust crates mirror these definitions, and `tests/protocol` drives real
payloads through both implementations so they cannot drift silently.

The method registry (`METHODS`) is a whitelist. There is no generic
`invoke(command, args)` escape hatch: a method that is not listed does not
exist, and the renderer is told so. Params are validated against that method's
schema in the main process before the request reaches the service, and results
are validated on the way back — a misbehaving or version-mismatched service
cannot inject an unexpected shape into a Pinia store.

## Honesty invariants

These are design rules, enforced by tests, not aspirations.

1. **Nothing is simulated.** Levels come from real captured samples. Device
   lists come from WASAPI. Adapter lists come from `GetAdaptersAddresses`. If
   there is no data, the UI shows an empty state and instructions.
2. **`null` is not `0`.** An unknown figure renders as `—`. Reporting 0 ms of
   jitter when no receiver has reported is worse than reporting nothing.
3. **Provenance travels with every number.** Every diagnostics figure is tagged
   `measured`, `estimated` or `receiver-reported`, and the UI renders that badge.
4. **Receiver telemetry is never promoted to a measurement.** It carries a
   literal `source: "receiver-reported"` discriminator on the wire.
5. **No total latency figure.** The desktop publishes the terms it can measure
   and refuses to sum them into an end-to-end number it cannot know. Control
   channel RTT is reported and explicitly labelled as *not* audio latency.
6. **Unimplemented is not empty.** A method belonging to a later phase returns
   `NOT_IMPLEMENTED` naming the phase, never a plausible-looking success.

## Security model

| Boundary | Protection |
| --- | --- |
| Renderer → main | 8 fixed IPC channels; method whitelist; zod validation of params and results |
| Renderer capabilities | `contextIsolation`, `sandbox`, no `nodeIntegration`, all permissions denied, CSP with no remote sources and no `unsafe-inline` for scripts in production |
| Renderer navigation | Only the app protocol (or the dev server); external URLs restricted to `https:`/`mailto:` |
| Main → service | Windows named pipe with a DACL admitting only the current user's SID, plus a per-launch 192-bit token compared in constant time |
| Service → receivers | A short-lived join token presented from the QR (possession is the authorisation), plus an AEAD-encrypted audio transport (Phase 5); the plaintext transport is refused in release builds |
| Secrets at rest | DPAPI (`CryptProtectData`) with the secret's name bound in as entropy; never in a SQLite row |

What this does **not** defend against: malware already running as the same
Windows user. Nothing a desktop application can do protects against that, and
claiming otherwise would be dishonest.

## Shutdown policy

**Quitting StageLink Core shuts the audio service down.** This is a product
decision, not a limitation.

A monitor feed running with no operator interface attached is a hazard: nobody
can see clipping, nobody can stop it, and performers wearing in-ear monitors
have no way to intervene. So there is no background streaming mode. Quitting
during a live session is gated on explicit confirmation, after which the service
is given a grace period to notify receivers, drain send queues and release the
capture device before the process exits.

Renderer reloads are unaffected, as described above.

## Production rendering

The packaged renderer is served over a custom `stagelink://` scheme registered
as standard and secure, **not** `file://`. Three concrete reasons:

1. Nuxt emits absolute asset URLs (`/_nuxt/…`), which under `file://` resolve
   against the drive root.
2. History-mode routing means a refresh on `/receivers` must serve the SPA
   entry rather than looking for a directory.
3. A custom scheme gives the page a real security origin, so normal web security
   assumptions hold.

The protocol handler also refuses any request that escapes the renderer
directory, and 404s a missing asset rather than returning HTML — an HTML
response to a `.js` request produces a baffling syntax error instead of a clear
missing-file error.

## Related documents

- [`protocol/control-protocol.md`](protocol/control-protocol.md) — control methods and events
- [`protocol/audio-packets.md`](protocol/audio-packets.md) — the binary wire format
- [`protocol/pairing.md`](protocol/pairing.md) — join codes and the QR grammar for StageLink Mobile
- [`latency-measurement.md`](latency-measurement.md) — how to get a real end-to-end figure
- [`testing.md`](testing.md) — what is automated and what needs hardware
- [`development.md`](development.md) — building and running

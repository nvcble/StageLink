# StageLink Mobile — development plan

Status: **approved, in progress.** Written 2026-10-12. This document is the
reviewable record of the plan and the reasoning behind it.

## Context

StageLink Core captures a real mono monitor mix and meters it live, but it
**sends that audio nowhere**: there is no UDP sender, no control server, and
nothing in the repository calls `bind()`. The QR code the Receivers page
displays is real and final, and it points at nothing listening.

StageLink Mobile is the other half — the thing a performer actually wears.

**Intended outcome:** a performer scans the QR on the operator's laptop and
hears the monitor mix in their wired earphones, with their own volume, EQ, mute
and a hearing-safety limiter, at a latency we have measured rather than claimed.

### Decisions taken

| Decision | Choice |
| --- | --- |
| Audio path | Shared Rust `receiver-core` via FFI, reusing `audio-protocol` + `audio-core` |
| Platforms | Android first; iOS planned for, deferred (no Mac available) |
| Encryption | Plaintext for development, gated as the desktop gates it; closed before live use |
| Dart stack | Riverpod 3, freezed, Drift, ffi + ffigen — matching what is already in use |

---

## Defects found while planning

An architecture review of the existing code found four defects in shipped work
and several specification holes. These are listed first because they change the
design, and three of them would reach a musician's ears.

> **Status, 2026-10-12.** D1, D2, D4, D5 and D6 are fixed, each with a test that
> fails against the old behaviour. D3 is documented with the obligation stated
> at the one call site that can discharge it, and cannot be closed until the
> UDP sender exists to call it. All five specification contradictions are
> closed. See [M0](#milestones) for what remains: the UDP sender, the control
> server, and the reference transmitter — none of which are documentation.
>
> | Item | State | Evidence |
> | --- | --- | --- |
> | D1 EOS unparseable | Fixed | 3 tests in `crates/stream-server/src/packetiser.rs` |
> | D2 no sample rate | Fixed | `welcome.stream` in both implementations; receiver obligation 10 |
> | D3 `DISCONTINUITY` unset | Documented, open | Obligation recorded on `note_discontinuity()` |
> | D4 `NOT_IMPLEMENTED` undeclared | Fixed | 3 tests in `apps/desktop/test/ipc-contract.test.ts` |
> | D5 nonce underspecified | Fixed | `aead_nonce` / `aeadNonce`, vectors in both, cross-checked |
> | D6 latency claim | Fixed | README, `latency-measurement.md`, About page now say 40–90 ms |
> | Join handshake | Fixed | `hello.receiver`; `join.request` deleted from `pairing.md` |
> | Telemetry direction | Fixed | `receiver.heartbeat` / `receiver.telemetry` as requests |
> | Roster identity and playback fields | Fixed | `RosterEntry.identity`, schema-conformance test via the probe |
> | Phase labels | Fixed | Control server is Phase 5 everywhere |
> | mDNS SRV port | Fixed | `ServiceAdvertisement::srv_port()`; the dead doc link is gone |
>
> Running the desktop smoke test afterwards turned up two more shipped defects,
> described as D7 and D8 below. Both are fixed; both were invisible to the unit
> suites.

### D1 — The end-of-stream marker is unparseable when encrypted

`PacketBuilder::end_of_stream()` in `crates/stream-server/src/packetiser.rs`
emits a header-only datagram and sets `ENCRYPTED` when encryption is on. But
`parse_header()` in `crates/audio-protocol/src/packet.rs` rejects any encrypted
packet whose payload is shorter than the 16-byte AEAD tag.

So in a release build the EOS marker is dropped as malformed, and every receiver
"waits out its jitter buffer on sudden silence" — precisely what the comment
above `end_of_stream()` says it prevents. Worse, an unauthenticated EOS is a
forgery vector: anyone on the venue network could silence every performer.

**Fix:** seal a zero-length payload, giving a 16-byte tag-only payload that
parses and is authenticated.

### D2 — The packet header does not carry the sample rate

`AudioPacketHeader` has `channels` and `payloadFormat` but no rate, while
`StreamConfig` permits 8 000–192 000 Hz. A 44.1 kHz stream played as 48 kHz is
8.8% sharp — a semitone and a half, in a singer's ears, live.

**Fix without a header change (v1 stays stable):** the rate is negotiated on the
control channel at admission and re-sent on any `sessionId` change. The receiver
*independently* infers it as `frames_per_packet ÷ (Δtimestamp_us ÷ 1e6)` from two
consecutive in-order packets and **refuses to play** if the two disagree. This
becomes a receiver obligation.

### D3 — `DISCONTINUITY` is never set in production

`note_discontinuity()` is called only from a test. A desktop capture glitch
therefore reaches the receiver as continuous sequence numbers, continuous
timestamps and no flag — just packets arriving late. The receiver underruns with
zero loss indication, and an operator cannot tell a desktop stall from network
loss.

**Fix:** wire it to the capture xrun counter.

### D4 — `NOT_IMPLEMENTED` is not a declared error code

The Rust service emits it; `ServiceErrorCode` does not contain it; so
`toServiceError` fails its enum parse and collapses it to
`INTERNAL: "Unknown error"` before it reaches the renderer. Every Phase-3/5
method is therefore mislabelled in the UI, and the honesty invariant that
"unimplemented names its phase" is broken at the boundary. Only
`scripts/probe-service.mjs` asserted it, and that talks to the service directly
over the pipe, so nothing caught it.

### D5 — The AEAD nonce is 64 bits of a required 96, and can collide

ChaCha20-Poly1305 needs a 96-bit nonce. `sessionId` (32) + `sequence` (32) is
64, and `sessionId` is explicitly *truncated*, so it can collide between
sessions. Nonce reuse under Poly1305 is key recovery, not merely a
confidentiality loss.

**Fix:** specify bytewise as `0x00·4 ‖ sessionId_le(4) ‖ sequence_le(4)`, with
the per-session key derived by HKDF from the handshake so a `sessionId`
collision across sessions still means a different key. Sender rule: end the
session before `sequence` wraps — at 200 packets/sec that is 248 days, so it
costs nothing.

### D6 — The latency target is not achievable on Android

Measured reality for this signal path: capture 10 ms + packetise 5 ms + Wi-Fi
2–5 ms + jitter buffer 20 ms + Android output 15–25 ms = **52–65 ms median** on a
good phone, 80–110 ms on a mid-range one. Android output latency alone is
15–80 ms depending on device.

The documented 15–30 ms design target is not reachable with a phone as the
receiver. `docs/latency-measurement.md` hedges it correctly as a target, but the
mobile app must be **designed for a 40–90 ms reality**, and the product
documentation should say so. A guitarist at 60 ms is fine; a singer hearing
their own voice at 60 ms will complain about comb filtering against bone
conduction. That needs to be discovered on a bench, not at a service.

### D7 — The renderer's stores were empty on first launch

Found by the desktop smoke test while verifying the above.

`useStageLinkBridge()` reads `$stagelink` off the Nuxt app, but the bridge
plugin supplied it by *returning* `{ provide: { stagelink } }` — which does not
take effect until the plugin's setup resolves. So the initial loads the plugin
itself awaited all failed with `SERVICE_UNAVAILABLE`, and since they are
`allSettled` and each store records its own error, nothing was raised.

The visible result: on first launch the application showed its unconfigured
empty states — no selected adapter, no selected input, no join QR — until the
operator happened to open a page that refetches on mount. That is
indistinguishable from StageLink having lost its settings, thirty seconds before
a service.

**Fixed:** `nuxtApp.provide` before the loads, plus a reload when the service
transitions to usable (the renderer can win the race against the handshake, and
most state is pushed only when it *changes*, so a failed startup load was never
retried). Guarded by `apps/desktop/test/plugin-startup.test.ts` and two new
smoke checks.

### D8 — Minting a join code and displaying it disagreed

Also found by the smoke test. `receivers.createJoinCode` gated on the persisted
`bind_address`; everything else — the Network page, the join panel's own
warning — asks whether an adapter is *selected*. So a real, valid token could be
minted and never displayed, behind a warning telling the operator to do
something they appeared to have done. The same gap would have put the address of
an unplugged adapter into a QR that no device can connect to.

**Fixed:** one derivation, `serialize::selected_address`, used by the Network
page and by minting alike; it requires the selected adapter to still be present.
The host is then stored *with* the minted code, so what the operator is holding
up keeps saying what it said when it was issued.

### Specification contradictions to close

- **Two incompatible join handshakes.** `envelope.ts` requires `hello` first
  carrying a token; `pairing.md` shows `join.request`, which is not in the
  method registry. Resolution: extend `hello` with the device description and
  delete `join.request`.
- **Telemetry and heartbeats have no legal direction.** Documented as
  client→server *events*, but events are server→client only and the server
  discards client events by design. Resolution: `receiver.heartbeat` and
  `receiver.telemetry` become request methods. The heartbeat ack gives liveness
  confirmation and RTT in one frame.
- **`playbackState` and `outputLatencyMs`** are in the frozen protocol but have
  nowhere to live in the Rust roster; the serialiser hardcodes them.
- **Phase labels disagree** — the control server is Phase 3 in two files and
  Phase 5 in three.
- **mDNS SRV port is unspecified**, and `advertise.rs` references a
  `docs/protocol/discovery.md` that does not exist.

---

## Architecture

```
┌──────────────────────── Flutter app (Dart) ─────────────────────────┐
│  UI            Riverpod 3 + freezed                                 │
│                Join (mobile_scanner) · Monitor · Settings           │
│  Control       NDJSON semantics only — no keys, no tokens           │
│  Persistence   Drift: trusted servers, fingerprints, presets        │
└──────────────────────────────┬──────────────────────────────────────┘
                               │  dart:ffi via ffigen — CONTROL ONLY
                               │  no audio, no key material
┌──────────────────────────────▼──────────────────────────────────────┐
│  receiver-core  (Rust)                                              │
│                                                                      │
│   control thread:  WebSocket, handshake, key schedule, fingerprint   │
│                    pinning, 1 s heartbeat                            │
│   net thread:      recv ─► parse ─► decrypt ─► decode ─► slot store  │
│   audio callback:  de-jitter ─► resample ─► gain ─► EQ ─► limiter    │
│                    ─► hard clip ─► duplicate mono to L/R             │
│                                                                      │
│   reuses parse_header, sequence_delta, SequenceTracker (stats only), │
│   pcm_s16le_to_f32, LevelAccumulator, AudioFormat, constants         │
└──────────────────────────────┬──────────────────────────────────────┘
                               │  C++ shim → Oboe → AAudio
                               ▼  real-time callback, zero allocation
```

### Why Dart is excluded from the audio path

Not primarily GC pauses — the structural reasons are stronger and also apply to
the heartbeat: there is no Dart isolate on an AAudio callback thread and one
cannot be created per callback; Dart cannot request `SCHED_FIFO`; and the
Flutter engine lifecycle is not the audio lifecycle.

### Why Rust also owns the control channel

Two reasons, both decisive:

1. **Five missed 1-second heartbeats and the desktop stops sending audio.** If
   Dart owns the heartbeat, a performer's monitor feed depends on the Dart
   isolate being scheduled every second for 90 minutes — through Doze, OEM power
   management, and GC. Rust would keep rendering perfectly while the desktop cut
   the feed. That failure mode would be *created by the architecture*.
2. **Key material must not touch the GC heap.** Dart's GC copies objects and
   cannot be reliably zeroized, which makes "never persist a join token" and
   "keys never written to SQLite" untestable claims.

Dart keeps the semantic half: it sends and receives NDJSON lines for
non-security methods and never sees a key or a token.

### Deliberate departures from the first draft

| First draft | Revised | Why |
| --- | --- | --- |
| Dart owns the control channel | Rust owns socket, handshake, keys, pinning, heartbeat | Above |
| Reuse `audio-core::AudioRing` | Reuse everything **except** `ring.rs` | A jitter buffer needs sequence-indexed random access; `HeapRb` cannot resize |
| `SequenceTracker` drives the buffer | Stats only, plus a separate heard-audio tally | It mis-attributes old duplicates as reordered and *decrements* `lost` |
| Sink pulls from the jitter buffer | De-jitter **inside** the render callback | Two queues in series add latency and hide depth from the adaptive logic |
| receive → convert → EQ → limiter | **Add a resampler, first in the chain** | Drift is 540 ms over 90 min at 100 ppm. Not optional. |
| `oboe` Rust crate or `cpal` | C++ shim over Oboe; Rust stays pure | Keeps `cargo-ndk` trivial; Kotlin is needed anyway; cpal hides the controls that *are* the product |
| 4 Android ABIs | `arm64-v8a` + `x86_64` | minSdk 29 makes armv7 irrelevant; x86_64 is for the emulator |
| Desktop sender first | Reference transmitter first | The sender is ~150 lines; the hard parts need neither phone nor desktop |
| 15–30 ms target | Design for 40–90 ms | D6 |
| Rust boots with saved settings | Rust boots **muted** at the safe ceiling | Failure direction matters when audio goes into ears |

### Jitter buffer, concealment, drift

- **Structure:** 256 sequence-indexed slots (`seq & 255`), 246 KB total. Each
  slot carries an `AtomicU32` tag published `Release` by the net thread and read
  `Acquire` by the render thread. Gives duplicate rejection for free.
- **Target depth:** floor 12 ms, start-up 30 ms, ceiling 120 ms. Adaptation from
  `p99(d) − p5(d)` over a 2 s window where `d = arrival_monotonic − timestamp_us`.
  Increase immediately; decrease at most one packet per 10 s after 20 s clean.
  Presets: Tight 20 ms / Normal 40 ms / Safe 80 ms.
- **Gaps are concealed, never skipped.** Skipping permanently removes depth and
  leads to underrun. Exceptions: >200 ms gap, `DISCONTINUITY`, or `sessionId`
  change all trigger a faded resync.
- **Concealment:** ship a 5 ms fade-out/in first (20 lines, sounds like a soft
  dropout rather than a click); upgrade to pitch-synchronous repetition once the
  harness can measure the difference.
- **Drift:** 540 ms accumulates over 90 minutes at 100 ppm, so compensation is
  mandatory. Signal is `produced − consumed` frames, where consumed is counted
  by the render callback — `timestampUs` carries no drift information, and
  `CLOCK_MONOTONIC` is a different clock from the audio PLL. Mechanism is a
  32-tap polyphase resampler (1.54 MMAC/s, mono) driven by a PI controller with
  a 2 s filter and 10–20 s settling, clamped to 500 ppm steady state
  (0.87 cents — inaudible) and 2000 ppm for at most 5 s when retiring burst
  excess.

### Hearing safety

The limiter is not fully parameterisable from Dart: Rust clamps the ceiling to a
hard maximum no FFI call can exceed, and applies an unconditional,
non-configurable hard clip afterwards. The clip is also needed because
`pcm_s16le_to_f32` divides by 32767, so a foreign `i16::MIN` decodes to
−1.00003. Raising the ceiling at all goes through one coarse, auditable
`slr_set_safety_profile` call.

---

## Milestones

Ordered so that everything hard is proven **before** any hardware is required.

| # | Milestone | Needs |
| --- | --- | --- |
| **M0** | Spec fixes and shipped-defect fixes (D1–D6, contradictions), each with a failing test first. **Protocol work complete**; D3's fix waits on M6, and the UDP sender, control server and reference transmitter are M1/M6/M7 | — |
| **M1** | `crates/reference-transmitter` — WAV/tone replay with injectable loss, bursts, reorder, duplicates, outages, `sessionId` change, `--clock-ppm`, `--seed`. **Done**, and verified against `test-receiver` over a real socket | — |
| **M2** | `crates/receiver-core` as a pure library + `receiver-harness` with a simulated DAC clock. De-jitter, concealment, resampler, drift controller, EQ, limiter, telemetry. **~80% of the hard code. Done**, less adaptive depth, which is M4 | — |
| **M3** | C ABI, built as a **Windows DLL**, driven by Dart tests on desktop. **Done** — 18 Dart tests against the real library; bindings hand-written because libclang is absent | — |
| **M4** | Android toolchain: two ABIs cross-compiled, a release APK carrying them. **Done**, less `.so` loading on a device | Emulator |
| **M5** | Oboe C++ shim + route-change state machine. **Written, never run** | **Phone** |
| **M6** | Desktop UDP sender. **Done** and validated end to end | — |
| **M7** | Control channel: WebSocket server and client, handshake, heartbeat, telemetry. **Done**, less fingerprint pinning | — |
| **M8** | Encryption on, plaintext refused. **Done** and verified against a release build. Physical loopback latency measurement still needs a phone | Phone |
| **M9** | iOS | **Mac** |

Proving the C ABI as a Windows DLL (M3) before any Android toolchain exists is
the single largest de-risking step available without hardware.

**Recommendation:** buy two Android phones now — a Pixel (10–20 ms output) and a
mid-range MediaTek device (40–80 ms) bracket the real world and convert most of
the untestable list below into a testable one.

---

## Android specifics that will bite

| Issue | Handling |
| --- | --- |
| **Wi-Fi power save** — the number one killer; PSM buffers unicast at the AP, 100 ms+ spikes | `WifiLock(WIFI_MODE_FULL_LOW_LATENCY)`, API 29+. Still throttled by some OEMs, so show a network-quality readout |
| **Foreground service** | `foregroundServiceType="mediaPlayback"` — no timeout, unlike `mediaProcessing`. Must start while visible on Android 14+ |
| **Audio focus** | `USAGE_MEDIA`/`CONTENT_TYPE_MUSIC`, not `VOICE_COMMUNICATION`. **Do not auto-duck** — ducking a performer's monitor mid-song is a hazard. Warn loudly instead |
| **Route changes** | Oboe destroys the stream and makes a *new callback thread*. No thread-locals, no cached thread id, explicit stop→rebuild→start |
| **Bluetooth** | Refuse outright: +100–300 ms. Detect the route and say so |
| **Multicast lock** | Android silently drops multicast without `MulticastLock`; looks exactly like a protocol bug |
| **Socket binding with a SIM** | Default route is cellular. Bind to the Wi-Fi network explicitly, or it works on a tablet and fails on every real phone |
| **minSdk** | **29** — `WIFI_MODE_FULL_LOW_LATENCY` is the single most valuable API here |
| **`panic = "abort"`** | The workspace release profile must not be inherited by the mobile cdylib; `catch_unwind` at every `extern "C"` entry, and `slr_render_f32` infallible by construction |

---

## Verification

### Fully testable on this Windows machine — roughly 80% of the hard code

- All of `receiver-core` through `receiver-harness` with a simulated DAC clock.
- 90-minute drift soaks at ±10/30/50/100 ppm, deterministic, with sample-exact
  output-length assertions. *Better* than a phone, because the ppm is chosen.
- Loss, reorder, burst, outage and session-change behaviour, reproducibly —
  which real Wi-Fi cannot provide.
- Audio quality in CI: SNR against the clean reference plus a click detector on
  max |x[n] − x[n−1]| and spectral-flux spikes.
- **Limiter conformance as a unit test**, not a listening test: 0 dBFS square,
  +12 dB volume, an injected −1.00003 sample, assert nothing exceeds the ceiling.
- The whole C ABI via the Windows DLL: stats seqlock, event ring, lifecycle
  under concurrent calls, use-after-destroy, profile ordering.
- Dart: QR grammar including malformed and version-mismatch, NDJSON codec
  against `tests/protocol` fixtures, Drift migrations.
- Obligation 9 falsifiably: after admission, grep the Drift file's raw bytes for
  the token and assert it is absent.

### Not testable without a phone — stated bluntly

Real output latency and `getTimestamp()` accuracy; real DAC clock drift
magnitude; Oboe's low-latency/MMAP path and actual `framesPerBurst`; route
changes; Wi-Fi power save and real jitter distributions; thermal throttling and
battery over 90 minutes; OEM power management killing the service; audio focus
against real calls. And whether it sounds acceptable to a musician — which is
not a test, it is a rehearsal.

---

## Open items

- Concealment upgrade from fades to PSOLA, gated on the measurement harness.
- `DEFAULT_QUEUE_PACKETS = 64` is 320 ms per receiver; review once the sender
  exists.
- 802.11 retry bursts at the AP cause depth ratchet; the slow-trim mechanism is
  the defence and needs field validation.
- **Arrival jitter biases the buffer-depth measurement upwards.** The depth is
  measured as the span to the newest packet present, so an early arrival raises
  it although the audio in between has not arrived. Measured in the harness:
  under 15 ms of jitter, with two *identical* clocks, the depth reads ~7 ms
  above target and the drift controller holds ~460 ppm trying to drain it. The
  pitch offset is 0.8 cents and inaudible, but the correction is left with no
  headroom for real drift and the rate the receiver reports becomes a false
  ~90 ppm. The fix is to steer on a windowed *minimum* depth — how close the
  buffer came to starving — which needs the same percentile machinery as
  adaptive depth, so it is deferred to that work rather than bolted on.
- **Two findings corrected the reviewed design, both from the harness.** The
  plan had the drift controller steering on the contiguous (gap-free) run,
  reasoning that a buffer "will underrun on the hole no matter how full it
  looks" — untrue of a reader that conceals and moves on, and it made a stream
  with every third packet missing report a near-empty buffer. And resampling
  alone cannot retire a large backlog: at the 500 ppm steady clamp, the 200 ms
  an outage leaves takes seven minutes to drain, so a faded skip is now used
  above 40 ms of excess.
- **A rejected join tells the phone nothing.** The desktop closes the socket,
  which cannot distinguish "the session is full" from "that code expired" —
  and those want different words on screen. Delivering the reason needs a
  server→receiver message defined outside the renderer's event registry, which
  is a control-server design decision (M7), left open rather than guessed at.
- **Revocation does not re-key the audio stream.** The session key is derived
  from the join token, so every receiver admitted under one code shares it: a
  revoked device is ejected from the roster and no longer sent to, but can
  still decrypt packets it can see. The fix is a per-receiver key over a
  control channel that is itself encrypted and authenticated — an ephemeral
  X25519 exchange authenticated by the join token, with the desktop key
  pinned via the `fp` fingerprint discovery already advertises. Its own
  milestone; the operational answer meanwhile is to restart the stream, which
  takes a new session id and therefore a new key.

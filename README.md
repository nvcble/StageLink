<div align="center">
  <img src="assets/branding/StageLink-Logo.png" alt="StageLink" width="96" height="96">

  # StageLink

  **Your mix. Your stage. Your sound.**

  Wireless in-ear monitoring for stage and pulpit participants.
</div>

---

StageLink takes **one combined monitor mix** from your FOH console's AUX send and
delivers it to every performer's phone over a local network. Personal volume, EQ,
mute and hearing-safety limiting happen on each device, where the person wearing
the earphones can reach them.

No cloud. No internet connection required for live audio.

```
FOH mixer ─ AUX send ─ USB interface ─ Windows desktop ─ Ethernet ─ Wi-Fi AP ─ phones ─ IEMs
                                       └─ StageLink Core ─┘
```

| Component | Status |
| --- | --- |
| **StageLink Core** — Windows desktop (Electron + Nuxt 4 + Rust) | This repository |
| **StageLink Mobile** — receiver app (Flutter) | Protocol defined, app not started |

## What works today

- Real WASAPI capture of one mono channel from a multi-channel USB interface,
  on a dedicated real-time thread with lock-free hand-off.
- Live metering — peak, RMS, decaying peak-hold, clip counting — computed from
  **every captured sample** in Rust and pushed to the UI at 25 Hz.
- Real Windows audio device and network adapter enumeration, including devices
  that are unplugged or disabled, with the reason shown.
- An authenticated local control channel between Electron and the Rust service
  over a named pipe restricted to the current user.
- Join codes with locally generated QR codes: a device scans and admits itself,
  with no approval queue. Codes expire after five minutes and rotate on demand.
- Session state machine, SQLite persistence with versioned migrations, and
  diagnostics where every figure is tagged `measured`, `estimated` or
  `receiver-reported`.
- A packaged Windows installer, verified end to end.

## What does not work yet

Stated plainly, because a monitor system you cannot trust is worse than none:

- **Audio does not reach phones yet.** The packet format, packetiser and
  per-receiver queues are built and tested; the UDP sender is Phase 3. Real
  StageLink packets *do* now travel over a real socket between
  `reference-transmitter` and `test-receiver` — see
  [docs/testing.md](docs/testing.md) — which is how the receiver is being built
  ahead of the sender, but no audio leaves the desktop application.
- **Receivers cannot connect yet.** The join QR is real and final, but the
  control server that honours it is Phase 5.
- **The encrypted transport is not implemented.** Phase 5. The plaintext
  transport is refused in release builds.

Control methods for unfinished features return `NOT_IMPLEMENTED` naming the
phase, never a plausible-looking success. See
[docs/development.md](docs/development.md#current-phase-status).

## Latency

The design target is **40–90 ms** from desktop capture to a wired earphone on
Android, under favourable conditions. That is a target, not a measurement, and
nothing in this repository has measured it.

An earlier version of this file said 15–30 ms. That was not achievable and
should not have been written: the Android output path alone is typically
20–40 ms on a device with AAudio low-latency support and over 100 ms on cheap
hardware, before any jitter buffer. The desktop's own contribution (a 10 ms
WASAPI capture buffer plus a 5 ms packet) is the small part. A receiver's jitter
buffer has to absorb real Wi-Fi arrival spread, which is tens of milliseconds on
a shared access point.

Being wrong about this by a factor of three matters: a vocalist monitoring their
own voice notices around 15 ms, so the honest framing is that StageLink is built
for in-ear monitoring on a dedicated network and that per-device measurement
decides whether a given phone is usable.

StageLink reports the desktop-side contributions it can genuinely measure, each
labelled with how it was obtained, and deliberately does **not** present an
end-to-end total — Wi-Fi airtime, the receiver's jitter buffer and the mobile
audio output path are not knowable from the desktop. For a real figure, follow
the physical loopback procedure in
[docs/latency-measurement.md](docs/latency-measurement.md).

## Getting started

Requires Windows 10/11 x64, Node 22, Rust 1.99 and MSVC Build Tools.

```bash
npm install
npm run build --workspace @stagelink/shared-protocol
npm run build:service
npm run icons
npm run dev
```

Full instructions: [docs/development.md](docs/development.md).

## Recommended deployment

A **dedicated Wi-Fi access point, cabled to the desktop over Ethernet, with no
internet uplink.** The Network page recommends the right adapter and explains
why. StageLink will tell you about a firewall rule but will never change one on
your behalf.

## Testing

433 automated tests (326 Rust, 107 TypeScript), plus an end-to-end smoke test
that launches the real application and asserts it came up.

```bash
cargo test --manifest-path crates/Cargo.toml
npm test --workspaces --if-present
node scripts/smoke-test.mjs
```

Which tests need real hardware, and which claims are not yet verifiable, are
listed in [docs/testing.md](docs/testing.md).

## Documentation

| Document | Contents |
| --- | --- |
| [architecture.md](docs/architecture.md) | Processes, the real-time boundary, security model, shutdown policy |
| [development.md](docs/development.md) | Building, running, and the traps worth knowing about |
| [testing.md](docs/testing.md) | What is automated, what needs hardware, what is unverified |
| [latency-measurement.md](docs/latency-measurement.md) | How to obtain a real end-to-end figure |
| [protocol/control-protocol.md](docs/protocol/control-protocol.md) | Methods, events, errors, versioning |
| [protocol/audio-packets.md](docs/protocol/audio-packets.md) | The binary wire format |
| [protocol/pairing.md](docs/protocol/pairing.md) | Join codes, QR grammar and receiver obligations |

## A note on the signal

StageLink receives **one pre-mixed mono feed**. There are no separable
instrument channels in it, so there is no mixer here, no per-instrument faders
and no artificial stereo. Every receiver gets the same audio and duplicates it to
both ears. If performers need different mixes, that is a job for additional AUX
sends on the console — each one a separate StageLink instance.

## Hearing safety

This system puts audio directly into people's ears for an hour at a time. The
output limiter is a **receiver-side** responsibility and is not optional: the
desktop cannot know what earphones are attached or how loud the output stage is.
A receiver implementation without a limiter is not conforming.

## Licence

Unlicensed / all rights reserved. The StageLink logo in `assets/branding/` is
the project's visual identity and must be preserved as supplied.

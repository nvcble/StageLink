# Testing StageLink

## What is automated, and what is not

This distinction is kept explicit because "the tests pass" means very different
things for a buffer-arithmetic test and for a claim about audio reaching
someone's ears.

| Layer | Count | Needs hardware? |
| --- | --- | --- |
| Rust unit and integration | 352 | Mostly no — see below |
| `@stagelink/shared-protocol` | 43 | No |
| Electron / desktop | 62 | No |
| Cross-implementation protocol | 22 | No (spawns the Rust probe) |
| **Total automated** | **479** | |
| Desktop smoke test | 44 checks | Launches the real app |
| Physical audio verification | manual | **Yes** |
| Multi-receiver streaming | manual | **Yes, and Phases 3 + 5** |

## Running everything

```bash
# Rust
cargo test --manifest-path crates/Cargo.toml

# TypeScript (protocol, desktop, cross-implementation)
npm test --workspaces --if-present

# Typecheck
npm run typecheck --workspaces --if-present

# End-to-end: launches the real application and drives it over CDP
node scripts/smoke-test.mjs              # against the Nuxt dev server
node scripts/smoke-test.mjs --production # against the packaged build
```

## Hardware-dependent tests

Several Rust tests touch the real Windows audio stack. They are written to
**skip cleanly** rather than fail when the hardware is absent, so the suite
passes on a machine with no audio input — and they print why they skipped.

| Test | Needs |
| --- | --- |
| `wasapi::tests::enumeration_returns_consistent_device_records` | Nothing; asserts the *shape* of whatever the OS returns |
| `capture_thread::tests::captures_real_audio_from_the_default_device_when_one_exists` | An active capture endpoint |
| `audio_engine::tests::a_real_device_feeds_both_the_meter_and_the_ring` | An active capture endpoint |
| `adapters::tests::enumeration_returns_consistent_adapter_records` | Nothing; every machine has loopback |
| `secrets::tests::*` | Windows (DPAPI) |

A skipped test is not a passed test. To confirm capture really works on a given
machine:

```bash
cargo test -p audio-capture captures_real_audio -- --nocapture
```

If it prints `skipping:` the capture path was **not** exercised.

## Diagnosing an interface

The most common real-world failure is a driver refusing a format. This prints
exactly what each endpoint advertises and what it will accept:

```bash
cargo run -p audio-capture --example probe
```

```
── Line (NUX Audio)
   id            {0.0.1.00000000}.{...}
   state         active
   channels      4
   mix format    48000 Hz f32
   probed rates  [48000]
   period frames default=Some(480) min=Some(144)
   IsFormatSupported (shared):
      48000 Hz 4 ch f32  -> 0x00000000 OK
      44100 Hz 4 ch f32  -> 0x00000001
```

A note from building this, because it cost real time: `IsFormatSupported`
returns `E_POINTER` for *every* format if `ppClosestMatch` is NULL in shared
mode, which looks exactly like "this driver supports nothing". It also returns
`S_FALSE` — not an error HRESULT — when a format is unsupported but a closest
match exists, so only `S_OK` means yes.

## The control path, without Electron

Spawns the service, performs the authenticated handshake, and exercises real
methods including opening a device and watching live meters:

```bash
cargo build --manifest-path crates/Cargo.toml
node scripts/probe-service.mjs
```

It also checks the things that must *fail*: a wrong token, an incompatible
protocol major, an unregistered method, an invalid setting.

## The audio protocol, without a network

```bash
cargo run -p test-receiver --bin stagelink-test-receiver -- \
  --self-test --output ./received.wav
```

Drives 100 synthetic packets — one deliberately dropped — through the real
receive path, asserting sequencing, loss accounting and PCM round-trip accuracy,
and writing a WAV you can listen to.

## The desktop smoke test

`scripts/smoke-test.mjs` is the test that cannot be faked by unit tests. It
launches the application, attaches over the Chrome DevTools Protocol, and
asserts that the whole stack came up:

- the shell mounted and the sidebar rendered,
- the official logo loaded,
- the preload bridge exposes exactly its documented surface and nothing else,
- `require`, `ipcRenderer` and `Buffer` are absent from the renderer (and
  `process` too, in a packaged build),
- the Rust service reached `ready` and agrees on the protocol version,
- real device and adapter lists arrived **through the bridge**,
- the receiver list is empty rather than populated with demo devices,
- the join QR renders as locally generated inline SVG,
- the stores the UI reads agree with the service they read from — a panel
  saying "no adapter selected" while the service has one is how a stale store
  looks to an operator, and it is indistinguishable from lost settings,
- icons render from bundled data URIs, not from the network,
- an unregistered method is refused,
- client-side routing works on every page.

It captures a PNG of the **application window** (via CDP, not a screen grab)
into `tests/integration/artifacts/`.

Both modes matter. Several bugs found during development only appeared in one:

- the CSP blocking Nuxt's inline bootstrap script — blank window, dev and prod;
- `electron-log` excluded from the package — silent death before logging, **prod
  only**;
- `if (!isDevelopment)` missing its parentheses — protocol handler never
  registered, **prod only**, invisible to the typechecker;
- icons fetched from `api.iconify.design` — would fail on an isolated venue
  network, which is the *only* network that matters;
- the bridge plugin providing `$stagelink` by *returning* it, so every startup
  store load failed with SERVICE_UNAVAILABLE and the UI showed its unconfigured
  empty states on first launch — dev and prod, invisible to 59 passing unit
  tests because none of them went through the plugin. Now also guarded by
  `test/plugin-startup.test.ts`.

## What is not yet testable

Honest list, so nobody mistakes a green suite for a working monitor system:

| Claim | Status |
| --- | --- |
| Real audio is captured and metered | **Verified** on real hardware |
| Audio packets reach a receiver | **Not verified.** The packetiser and receive path are tested; the UDP sender is Phase 3 |
| Multiple concurrent receivers | **Not verified.** Needs the UDP sender (Phase 3) and the control server (Phase 5) |
| Receiver pairing and authorisation | **Not verified.** Phase 5 |
| Encrypted audio transport | **Not verified.** Phase 5 |
| End-to-end latency | **Not measurable in software.** See [`latency-measurement.md`](latency-measurement.md) |

## Manual verification before a service

1. Run `cargo run -p audio-capture --example probe` and confirm your interface
   appears as `active` with the expected channel count.
2. Launch StageLink Core. The Audio Input page should list your interface.
3. Select it and the input channel carrying the monitor send.
4. Play audio through that send. The meter must move. If it reads silence,
   use **Test the input** — it reports what is actually arriving.
5. On the Network page, confirm the Ethernet adapter facing your access point is
   selected and there are no error-severity warnings.
6. Check the Diagnostics page: `Driver glitches` and `Buffer overruns` should
   stay at zero over a few minutes. If `Real-time priority` says *Not granted*,
   glitches are more likely under load.

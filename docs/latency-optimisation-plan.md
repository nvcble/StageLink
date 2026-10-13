# StageLink latency optimisation — plan

Status: **all four phases complete, 2026-10-13. Unvalidated on real hardware.**
This document is the reviewable record of the plan, the reasoning behind it, and
what has actually been done.

Every code change is in and tested. What has *not* happened is a measurement:
nobody has yet put a phone on a stage and listened. The numbers below are a
code-path analysis plus a simulation, and the open items at the bottom say so.

Companion to [latency-measurement.md](latency-measurement.md), which defines
what can and cannot be measured. That document is about honesty; this one is
about getting the number down.

## Context

A performer on a phone reported latency "just like a video call" — a constant
offset in the 150 ms+ range, not dropouts or glitches. The design target in
`latency-measurement.md` is 40–90 ms.

An investigation of the full path found that the desktop was sending audio
**about a quarter of a second old**, and that every meter in the system read
healthy while it did it. The capture ring was created when a device was opened
and nothing consumed it until a stream started, so it saturated; the producer
drops the *newest* samples when full, so it froze holding the oldest audio; and
the sender consumes at exactly real time and deliberately never bursts to catch
up. The result was a permanent backlog that nothing downstream could detect,
because the packets were perfectly paced and perfectly sequenced — just late.

### Why nothing caught it

Worth recording, because it is the more useful lesson than the bug:

- **The receiver cannot see it.** The jitter buffer and PI drift controller
  steer on *their own* depth, which was healthy. Staleness entering the sender
  is invisible to them.
- **The tests cannot see it.** `ring.rs` had thorough tests that the ring is
  *correct* as a FIFO. None asserted that it stays *shallow*.
- **The dashboard could see it, and said so.** `sendQueueMs` on the Diagnostics
  page has always been the ring occupancy (`serialize.rs`), and it fed
  `desktopSubtotalMs`. The number was on screen the whole time and nobody read
  it.

### Measured budget

| Term | Before Phase 1 | Target | Source |
| --- | --- | --- | --- |
| Capture period | ~10 ms | ~3 ms | Driver-reported, shared-mode default |
| Capture ring (`sendQueueMs`) | **~250 ms** | ~5–10 ms | Measured ring occupancy |
| Packet frame | 5 ms | 5 ms | Configured |
| Network | 1–5 ms | — | Not measurable from the desktop |
| Jitter buffer | 40 ms | 20–40 ms | Receiver preset |
| Android output | 20–40 ms | 20–40 ms | Oboe `calculateLatencyMillis` |
| **End to end** | **~325–345 ms** | **~55–105 ms** | |

### What was already correct

Ruled out early, and recorded so nobody re-investigates:

- **Android output is already optimal** — Oboe with `PerformanceMode::LowLatency`
  and `SharingMode::Exclusive`, float, 2× burst buffer, `Usage::Media` (not
  `VoiceCommunication`), plus a `WIFI_MODE_FULL_LOW_LATENCY` lock.
- **The jitter buffer does not accumulate** — 40 ms default target, actively
  steered, and it skips forward with a fade when more than 40 ms over.
- **The per-receiver send queue does not accumulate** — fully drained every tick.

---

## Phase 1 — Bound the capture ring

**Status: done, 2026-10-13.** Recovers ~240 ms. This was the whole defect;
everything after it is refinement.

- [x] Clear the ring when a stream starts — `AudioEngine::reset_ring()`, called
      from `stream_task::start` as late as possible, after everything that can
      refuse to start has done so.
- [x] Bound the backlog on every packet, not only at start —
      `RingConsumer::trim_to`, called from `pop_frames`. Necessary because the
      sender abandons lost time after a stall rather than bursting, so each
      stall would otherwise add its own delay permanently.
- [x] Shrink `RING_MILLIS` from 250 ms to 60 ms, with the rationale rewritten:
      capacity here is not free insurance, it is latency whenever it is used.
- [x] Report trims as discontinuities — `CaptureSource::take_glitches` now sums
      driver xruns *and* ring trims, so a receiver conceals the seam instead of
      playing a click and feeding the jump to its drift controller as clock
      error.
- [x] Regression tests, including one that runs against a real WASAPI endpoint.

### Deviation from the original plan

The plan said to invert the **producer's** overrun policy to drop oldest. That
is unsound: `ringbuf` 0.5 has no producer-side overwrite, and it should not — a
producer must never move the consumer's read position in an SPSC ring. The
policy therefore lives on the consumer as `RingConsumer::trim_to`, which is the
only place it can correctly sit. The producer still drops newest when full, but
with the trim in place that is self-healing within one packet rather than
wedging the ring. The trap is documented in the `ring.rs` module header.

### Evidence

| Item | Evidence |
| --- | --- |
| Ring trims oldest, counts it, is a no-op within bound | 4 tests in `crates/audio-core/src/ring.rs` |
| Reset and trim survive a saturated ring on real hardware | `a_stream_starting_on_a_saturated_ring_still_begins_near_live`, `crates/stagelink-service/src/audio_engine.rs` |
| Idle engine tolerates reset and pop | 2 tests, same file |
| No regressions | Full workspace suite, 0 failures; clippy clean on changed files |

Verified on hardware: the regression test was re-run with `--nocapture` to
confirm it exercised a real capture endpoint rather than taking its skip path.

### Not yet confirmed

The mechanism is verified; the **audible** end-to-end result is not. Confirming
it needs a stream running with a phone connected, reading `sendQueueMs` on the
Diagnostics page. Expected: single digits, down from ~250 ms.

---

## Phase 2 — Cut the capture period

**Status: done, 2026-10-13.** Recovers up to ~7 ms, hardware permitting. Only
meaningful once Phase 1 had landed, which it had.

The capture period was ~10 ms, the Windows shared-mode default, and the largest
remaining desktop-side term. Two routes existed; the first was taken because it
does not lock the device away from other applications.

- [x] **`IAudioClient3::InitializeSharedAudioStream`.** Queries
      `GetSharedModeEnginePeriod`, rounds the reported minimum up to the
      fundamental period, and asks for that — often 2.67 ms at 48 kHz. Falls
      back to the previous path, cleanly, in all four cases where it cannot be
      used: no `IAudioClient3`, no period below the driver's default, a locked
      engine periodicity, or a refusal.
- [x] **Report the period actually granted.** `periodFrames` was
      `GetDevicePeriod`'s *default*, so the figure on the Diagnostics page
      would not have moved even when the real period did. It now prefers
      `GetCurrentSharedModeEnginePeriod` (what the engine is really running at)
      and falls back down a ladder.
- [x] **Stop clobbering the persisted share mode.** An absent `shareMode` now
      means "leave it as it is" rather than "shared". Extracted to a free
      function, `resolve_share_mode`, so the decision is testable without a
      capture device.
- [x] **Handle `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`.** Asks the failed client
      what size it wanted, discards it — a client that fails this way cannot be
      reinitialised — and retries once on a fresh one, with the periodicity
      matched to the buffer as exclusive mode requires.

### Found while implementing

The low-latency path cannot resample: `AUTOCONVERTPCM` is not supported with
`InitializeSharedAudioStream`. This sharpens existing advice rather than
complicating it — running the interface natively at the stream's rate is now
what *unlocks* the small period, not merely what avoids a resampler.

More seriously, **the engine-resampling warning never fired.** `engineConverted`
was sent by the service but not declared in `captureFormatSchema`, and
`audio.status` events are validated against that schema with `parsed.data`
forwarded to the renderer — so zod stripped the field in transit and
`engineConverting` was always false on a pushed event. Declaring it fixed a
latency-relevant warning that had never been seen, and let the store drop a
type-cast escape hatch. `lowLatency` was declared properly at the same time.

### Evidence

| Item | Evidence |
| --- | --- |
| Smallest legal period, including the round-up and the unsatisfiable cases | 2 tests in `crates/audio-capture/src/wasapi.rs` |
| Frame-to-reference-time conversion rounds up, tolerates a zero rate | `frames_convert_to_reference_time_rounding_up`, same file |
| Absent share mode preserves the stored preference | 3 tests in `crates/stagelink-service/src/handler.rs` |
| No regressions | 665 Rust tests, 43 protocol tests, 62 desktop tests, 0 failures; typecheck clean; no new clippy warnings |

### Not yet confirmed

How much this actually recovers is **hardware-dependent and unmeasured**. Many
drivers report a minimum equal to their default, in which case the low-latency
path is correctly declined and nothing changes. The honest check is the
`period_frames` / `period_ms` pair on the `capture started` log line, and the
Capture period row on the Audio page, which now says `(low latency)` beside the
sharing mode when the fast path was granted.

### Deliberately deferred

- **A share-mode control in the UI.** Exclusive mode is fully plumbed in the
  backend and now survives a device selection, but is still reachable only by
  editing `settings.json`. With the low-latency shared path delivering a
  comparable period *without* taking the device away from every other
  application, putting a device-locking toggle in front of an operator is a
  product decision worth making deliberately — not a side effect of a latency
  fix. The backend work above means the path is no longer a trap for anyone who
  does enable it.
- **Forcing 48 kHz.** The dashboard warns when resampling is happening, and as
  of this phase that warning actually works.

---

## Phase 3 — Tune the receiver

**Status: done, 2026-10-13.** Recovers ~20 ms for a listener who selects
`Tight`. Only meaningful with Phase 1 landed, which it is.

The preset itself needed no plumbing — it is already a three-way control on the
monitor screen, wired through `slr_set_preset` to `AudioOutput::set_preset`.
What it needed was for the two quantities derived from the preset target to
actually be derived from it.

- [x] **`startup_ms` is now relative to the preset target.** It was an absolute
      30 ms against targets of 20/40/80 ms, so the drift controller corrected
      *downwards* for Tight and *upwards* for Normal and Safe. Safe — chosen
      precisely because the room is bad — began 50 ms short of what it promised
      and spent its first minute with less protection than Normal gives.
- [x] **The skip threshold is now one whole target buffer.** `skip_above_frames`
      was a fixed 40 ms, and its own doc comment described the intent as "more
      than an entire buffer over target" — which was only true at Normal. A
      Tight listener could sit at 60 ms, three times the 20 ms they asked for,
      without ever crossing it, and at the steady clamp that takes 80 seconds
      to drain. Deriving it from the target keeps Normal bit-for-bit identical
      (40 ms *is* one Normal buffer) and makes the other two mean what they say.
- [x] **Both move when the preset changes.** `set_preset` now recomputes the
      cached startup depth, and `set_target_frames` moves the skip threshold
      with the target. Without the first, a listener who changed preset would
      have the *next* stream primed to the depth of their previous choice,
      because the receiver re-primes on a new stream or a resync.
- [x] **A resync lands where priming is satisfied.** It landed one *target*
      depth behind the newest packet while priming asked for the startup depth,
      so the two disagreed and playback stalled for the difference — and in a
      stream that had stopped, stalled permanently.

### The startup margin is zero, and that is a measured result

The obvious reading of "startup should clear the target" is to start a little
deeper. That was implemented, measured, and rejected.

Starting above target adds a standing drain, and the depth the controller
steers on **counts holes** (`JitterBuffer::span_from`), so under loss it already
reads higher than the audio that can actually be played. The two compound: the
controller drains real audio to retire a surplus that was never there. In the
harness, a 10 ms margin collapsed the buffer to 0.5 ms under 2% burst loss and
pushed the reported clock estimate to 65 ppm when the true difference was 40.
A 5 ms margin passed, but with no measured headroom — passing is not the same
as being right.

Nothing is given up by starting level, because priming requires its depth as a
**gap-free** run, which is a stronger guarantee than the span-based depth the
stream is held at afterwards. A stream that has just started therefore has at
least as much real runway as one that has been playing for an hour. The margin
was insuring against something contiguity already covers.

The knob remains, defaulting to zero, with the finding recorded at the constant
so the next person to reach for it sees the measurement first. Raising it is
the wrong half of the fix: the depth signal has to stop counting holes first,
which is the windowed-minimum work `DriftController` already notes it wants.

### Evidence

| Item | Evidence |
| --- | --- |
| Every preset starts at the depth it promises, on a common margin | `every_preset_starts_at_the_depth_it_promises` |
| The startup depth is required gap-free | `the_startup_depth_is_required_gap_free` |
| Startup depth moves with the preset | `changing_the_preset_moves_the_startup_depth_with_it` |
| Skip threshold is one target buffer, for all three presets | `the_skip_threshold_is_one_whole_target_buffer` |
| It moves with the target | `moving_the_target_moves_the_skip_threshold_with_it` |
| Tight skips where Normal resamples, for the same excess | `a_tighter_preset_asks_for_a_skip_at_a_depth_a_looser_one_tolerates` |
| Startup waits for the configured depth, not a hard-coded count | `playback_waits_for_the_startup_depth_rather_than_starting_on_one_packet`, rewritten to assert the relationship |
| No regressions | 671 Rust tests, 0 failures, including all 24 end-to-end harness scenarios; clippy clean |

### Tight, measured in simulation

`receiver-harness`, 600 s, 8 ms uniform jitter, clocks 40 ppm apart:

| | Result |
| --- | --- |
| Buffer depth | min 13.1 ms, mean 20.1 ms, max 29.3 ms, against a 20 ms target |
| Concealment | 0 runs, 0 ms |
| Longest silence | 0.0 ms |
| Drift | learned 41.7 ppm against a true 40.0 |

So `Tight` holds its target and converges. **This is a simulation, not a
measurement** — it models jitter and clock error, and does not model Wi-Fi
airtime, contention or a phone's scheduler. Recommending `Tight` to operators
still needs a real run on a dedicated AP, which remains the open item below.

---

## Phase 4 — Stop this regressing

**Status: done, 2026-10-13.**

- [x] A regression test asserting the ring stays shallow while streaming. The
      existing tests verified the ring was *correct*; none asserted it was
      *shallow*, which is how this got through. (Phase 1.)
- [x] Count trims, so latency being paid back is visible rather than silent —
      `RingStats::trimmed_samples` and `trim_events`. (Phase 1.)
- [x] **Surface the trim counters.** `latencyTrims` and `latencyTrimmedMs` now
      reach the Diagnostics page, declared in `audioDiagnosticsSchema` and shown
      beside the overrun and underrun counts. A trim is not a fault in itself —
      it is latency being paid back rather than carried — but it only happens
      when this machine fell behind the send cadence, and each one is a seam a
      listener heard.
- [x] **Warn when the send queue backs up.** Routed through the existing
      `healthWarnings` channel, which already renders at the top of the
      Diagnostics page, rather than inventing a new affordance. Two warnings:
      one when `latencyTrims` is non-zero (the measured fact that audio had to
      be discarded) and one when the queue is currently more than three packets
      deep (the live state). The threshold is `packetFrameMs * 3` rather than a
      hard-coded 15 ms, so it mirrors `MAX_BACKLOG_PACKETS` and stays correct if
      the packet duration changes.

### Added: a guard for the defect class that hid the resampling warning

Phase 2 turned up something worse than the bug itself. Zod strips undeclared
keys, and it is the *parsed* payload that is forwarded to the renderer — so a
field the service sends but the schema does not declare simply vanishes. No
validation error, no log, no sign anything happened. A missing *declared* field
is loud; an undeclared *extra* field is invisible.

`forwardServiceEvents` now compares the payload against the parsed result and
logs any field the protocol discarded, with a dotted path. Recursive, because
the field that motivated it (`format.engineConverted`) was nested two levels
down and a shallow comparison would have missed it. Extracted to
`electron/ipc/payload-shape.ts` so it can be tested without Electron, following
`electron/main/protocol.ts`.

The payload is still forwarded, not dropped: the rest of it is valid and a
performer's monitor must not depend on a schema being exhaustive.

### Evidence

| Item | Evidence |
| --- | --- |
| Trim counters are published and present when idle | `the_backlog_trim_counters_are_published` |
| Undeclared nested field is detected, with its path | 5 tests in `apps/desktop/test/ipc-contract.test.ts` |
| Arrays are not descended into; nulls and primitives tolerated | same |
| No regressions | 672 Rust, 43 protocol, 67 desktop tests, 0 failures; typecheck clean; clippy clean |

### What this does and does not buy

The guard catches the *next* instance of a field going undeclared, in the main
process log. It does not prove the current schema is exhaustive, because nothing
here compares the Rust serialiser against the TypeScript schemas — that needs a
real cross-language contract fixture, which is the open item below.

---

## Open items

- [ ] **Measure the result.** The whole point of the exercise, and still
      outstanding: run a stream to a phone, read `sendQueueMs` on the
      Diagnostics page, and listen. Expected single digits where it was ~250 ms.
      Then redo the physical loopback in
      [latency-measurement.md](latency-measurement.md) and record the new figure
      with its conditions.
- [ ] **A cross-language contract fixture.** Nothing compares the Rust
      serialiser against the TypeScript schemas, so a field can be sent and
      never declared. Phase 4's guard logs the next occurrence but cannot prove
      the current schemas are exhaustive, and two fields had already slipped
      through (`engineConverted`, and `lowLatency` would have). A fixture
      captured from the service and validated in the protocol test suite would
      close it.
- [ ] **Validate `Tight` on a dedicated AP** before recommending it. The
      simulation above says it holds; only a real room says whether it holds
      there. Until then `Normal` remains the default.
- [ ] **Make the depth signal immune to holes.** `span_from` counts holes, so
      under loss the drift controller steers on a figure larger than the
      playable audio. This is the root cause behind the zero startup margin,
      and `DriftController` already names the fix: steer on a windowed minimum
      of the depth. Until it exists, the buffer cannot safely be started deeper
      than its target, which forecloses one otherwise reasonable option.

## Found in passing

Not latency, but found during the investigation and worth not losing:

- [ ] **An unattributed intermittent test failure.** One test failed in two
      separate full-workspace runs during this work, both times reported as
      "372 passed, 1 failed" with the run aborting before the details were
      captured. It did not reproduce in 19 consecutive full runs afterwards,
      nor in 15 runs of the two crates that open real audio hardware, so it
      could not be identified and may well predate this work. Recorded rather
      than dismissed. If it recurs, capture the full output — `cargo test
      --workspace > log 2>&1` — before anything else.
- [ ] `pop_frames` takes the engine mutex on the send thread, shared with
      `status()`, `open()` and `close()` — and the telemetry task polls
      `status()` regularly. Not a steady-state latency term, but it can inject
      jitter into the 5 ms send cadence and it is a priority inversion on the
      hot path.
- [x] `docs/latency-measurement.md` claimed the capture period came from
      `GetDevicePeriod`, and recommended exclusive mode for a lower period.
      Both corrected in Phase 2.
- [ ] The worked example in `docs/latency-measurement.md` still quotes a 10 ms
      capture period and a measured median of 31 ms. Those were recorded under
      the old behaviour and should be re-measured, not edited — a number
      without its conditions is not a measurement, and inventing a replacement
      would be worse than leaving a dated one.

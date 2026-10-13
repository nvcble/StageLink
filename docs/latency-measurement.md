# Measuring StageLink latency

## The honest position

StageLink's **design target** is 40–90 ms from this desktop to a wired earphone
on Android, under favourable conditions.

That is a target, not a measurement, and this document exists because the
difference matters. Nothing in the application reports an end-to-end latency
figure, because the desktop genuinely cannot know one.

> **Revised from 15–30 ms.** That earlier figure was not achievable and was
> never measured. The desktop's own contribution is the small part of the
> budget: a 10 ms capture buffer plus a 5 ms packet. The large parts are the
> Android output path (20–40 ms with AAudio low-latency support, over 100 ms on
> cheap hardware) and a jitter buffer deep enough to absorb real Wi-Fi arrival
> spread. A target that ignored the two largest terms was not a target.

### What the desktop can measure

| Term | How |
| --- | --- |
| Capture period | The period actually granted — `IAudioClient3::GetCurrentSharedModeEnginePeriod` where available, `GetDevicePeriod` otherwise |
| Packet frame duration | A configured value; arithmetic, not measurement |
| Send queue time | Timed on this machine |

The Diagnostics page shows these three and their sum, each labelled with its
provenance.

### What it cannot

- **Audio interface analogue stage** — converter and driver delay beyond the
  reported period.
- **Windows scheduling** — a late wake-up adds delay invisibly.
- **Wi-Fi airtime** — contention and 802.11 retransmission, the largest and most
  variable term in a busy room.
- **Receiver jitter buffer** — chosen by the receiver to trade latency against
  dropout resilience.
- **Mobile audio output path** — frequently the single largest term, and on
  Android it varies by an order of magnitude between devices.
- **The earphones themselves.**

This is why `desktopSubtotalMs` is labelled a *subtotal* and why a missing term
makes it `null` rather than a smaller number.

### Control channel RTT is not audio latency

The Diagnostics page shows `controlChannelRttMs` and explicitly says it is not
audio latency. A TCP round trip on an idle network tells you almost nothing
about one-way UDP audio delay through a jitter buffer, and presenting the two as
equivalent is the most common way these systems get misrepresented.

## Physical loopback: the real measurement

The only trustworthy end-to-end figure comes from measuring the acoustic or
electrical output against the original signal.

### What you need

- The StageLink desktop with its audio interface.
- A receiver phone with wired earphones, paired and receiving.
- A second audio interface with **two inputs** and a recorder
  (Audacity, Reaper, anything sample-accurate).
- Two cables, or one cable plus a measurement microphone.

### Method A — electrical (recommended, most accurate)

```
                    ┌──────────────────────────────┐
   test signal ─────┤ splitter                     │
                    └───┬──────────────────────┬───┘
                        │                      │
          ┌─────────────▼──────────┐    ┌──────▼──────────────────┐
          │ StageLink interface in │    │ measurement input 1     │  ← reference
          └─────────────┬──────────┘    └─────────────────────────┘
                        │ StageLink
                        ▼
          ┌────────────────────────┐    ┌─────────────────────────┐
          │ receiver phone         ├────┤ measurement input 2     │  ← delayed
          │ headphone output       │    └─────────────────────────┘
          └────────────────────────┘
```

1. Generate a sharp transient — a click, or a 1 kHz tone burst. Avoid a
   continuous sine: you cannot identify which cycle you are looking at.
2. Feed it to **both** the StageLink interface input and measurement input 1.
3. Take the receiver phone's headphone output into measurement input 2, at a
   safe level. Do not put earphones in anyone's ears while doing this.
4. Record both measurement inputs simultaneously on the same device, in the same
   file. Using two devices introduces clock drift and invalidates the result.
5. Measure the sample offset between the transient on the two channels.
6. `latency_ms = offset_samples / sample_rate × 1000`.

Record **at least 20 transients over a minute**. Wi-Fi latency is not a
constant; report the median and the 95th percentile, not a single number. A
system with a 20 ms median and a 120 ms 95th percentile is not a 20 ms system.

### Method B — acoustic (quicker, less precise)

Place a measurement microphone against the earphone driver instead of cabling
the headphone output. Subtract the acoustic path: at 343 m/s, every centimetre
is about 29 µs, so keep the microphone within a centimetre and note it as a
source of error.

### Isolating the terms

Run the measurement three times to find out where the time actually goes:

1. **Desktop only** — compare the reference against the StageLink interface's
   own loopback, if it has one. Gives capture + driver delay.
2. **Wired receiver** — run the receiver on a machine cabled to the access point
   rather than on Wi-Fi. The difference from (3) is Wi-Fi airtime.
3. **Full path over Wi-Fi** — the real figure.

Compare the result to `desktopSubtotalMs` on the Diagnostics page. The
difference is everything the desktop cannot see, and knowing its size is the
point of the exercise.

## Tuning

The settings worth touching, in order of effect:

| Setting | Effect | Risk |
| --- | --- | --- |
| Packet frame duration (5 ms default) | Lower adds less delay per packet | 2.5 ms doubles the packet rate and is less tolerant of loss |
| Receiver jitter buffer | Dominant term on a congested network | Too small means dropouts, which are worse than latency |
| Interface sample rate | Running the interface natively at the stream rate avoids engine resampling — and *unlocks* the low-latency capture path, which cannot resample | — |
| WASAPI exclusive mode | Rarely needed now: the low-latency shared path gets a comparable period without locking the device | Locks the device; nothing else can use it. Not exposed in the UI |
| Access point isolation | Removes contention — often the biggest single win | Needs a dedicated AP |

The dashboard warns when Windows is resampling the input, because that is a
latency cost an operator chasing milliseconds should know about.

## Recording the result

Note the figure **with its conditions**, or it is not reproducible:

```
Date:               2026-10-12
Interface:          Focusrite Scarlett 2i2, 48 kHz, shared mode
Capture period:     10.0 ms (driver-reported)
Packet frame:       5 ms
Access point:       dedicated, 5 GHz, no uplink, 4 receivers
Receiver:           Pixel 8 / Android 15, wired earphones
Jitter buffer:      40 ms
Measured (n=24):    median 31 ms, p95 48 ms, min 27 ms, max 61 ms
Method:             electrical loopback, two inputs, single interface
```

A number without its conditions is not a measurement.

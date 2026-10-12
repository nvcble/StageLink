# StageLink Mobile

The performer's end of a wireless in-ear monitor feed.

The audio path and the control channel are **Rust** — `crates/receiver-core`
and `crates/receiver-client` — behind a narrow C ABI. This Flutter app is the
screen in front of them. No audio and no key material crosses into Dart.

## Building

Two steps, because Gradle does not know how to drive Cargo and teaching it
would couple two build systems for no gain.

```powershell
# 1. Cross-compile the Rust receiver into android/app/src/main/jniLibs
pwsh tool/build_android.ps1

# 2. Build the app
flutter build apk --release --target-platform android-arm64,android-x64
```

`--target-platform` is not optional for a release you intend to distribute:
without it Flutter builds a 32-bit ARM slice that the receiver is not compiled
for. The Gradle config also drops any `armeabi-v7a` libraries plugins drag in,
because Android decides which ABIs an APK supports by looking at the libraries
present — so an armv7 phone would otherwise install it and crash on
`System.loadLibrary`.

First time only:

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
```

The NDK path lives in `crates/.cargo/config.toml`. `cargo-ndk` would normally
set it; version 4.1.2 panics on this workspace, and configuring it directly
means a wrong path is an error you can read.

## Testing

```bash
# Everything, including the real native library over FFI
flutter test

# The FFI seam alone, which needs the desktop build of the library:
#   cd ../../crates && cargo build -p receiver-ffi --profile mobile
flutter test test/ffi_test.dart
```

`test/ffi_test.dart` drives the **real** library through `dart:ffi` on the
desktop: the ABI version, every struct layout, the error mapping, and the
null-safety of every entry point. That is deliberate — it is the largest
de-risking step available with no phone attached, and it catches the class of
bug that is otherwise invisible until a device is in your hand.

It skips itself with a clear message when the library has not been built, so
somebody working on the UI is not blocked by a missing Rust build.

## Layout

```
lib/
  main.dart                     two screens, no navigation stack
  src/
    ffi/bindings.dart           raw C ABI — hand-written, see below
    ffi/receiver.dart           the safe wrapper; the only file with pointers
    audio/sink_channel.dart     starts and stops the platform audio output
    data/identity.dart          the device's long-term keypair
    data/database.dart          Drift: trusted desktops, monitor preferences
    state/                      Riverpod providers and the session controller
    ui/                         the screens
android/app/src/main/
  cpp/stagelink_sink.cpp        the Oboe callback: calls slr_render_stereo
  kotlin/.../AudioSink.kt       the Wi-Fi lock and the JNI surface
  jniLibs/                      built by tool/build_android.ps1 (not in git)
```

## Decisions worth knowing

**The bindings are hand-written, not generated.** `ffigen` needs libclang,
which is not present on every machine that has to build this — including the
one it was written on. The risk that buys is layout drift, and two things
guard it instead, both in the test suite: `slr_abi_version()` is checked at
load, and `sizeOf<SlrStats>()` is asserted against the size Rust pins. A
mismatch fails a test rather than putting a wrong buffer figure in a
performer's hand.

**No Riverpod code generation.** `riverpod_generator` needs an analyzer
version `freezed` does not, and resolving that would mean a prerelease
`freezed`. Providers are written by hand, which is idiomatic and a few lines
more.

**`minSdk` is 29.** `WIFI_MODE_FULL_LOW_LATENCY` arrives there and is the
single most valuable Android API for this product: without the Wi-Fi lock,
power save buffers unicast packets at the access point and delivers them in
bursts well over 100 ms, which no sane jitter buffer absorbs.

**Audio never touches Dart or the JVM.** Oboe's callback calls into Rust
directly. A garbage-collected language on that path would cost tens of
milliseconds of extra buffer to absorb a single pause, which is most of the
latency budget the product is built around.

**The `mobile` Cargo profile, never `--release`.** The workspace release
profile sets `panic = "abort"`, which is right for a standalone service and
wrong here: `receiver-ffi` catches panics at every `extern "C"` entry point so
one cannot unwind into Dart or Kotlin, and `catch_unwind` does nothing under
`abort`.

## What has not been verified

**Nothing in this app has run on a phone.** It compiles, the FFI seam is
driven by tests against the real library on the desktop, and the APK contains
the right libraries for both ABIs — but no audio has come out of a device.
Until it has, these remain unverified:

- that Oboe opens a low-latency stream on real hardware,
- what the output latency actually is, on any device,
- that the Wi-Fi lock does what it claims on a given OEM's build,
- that a route change (earphones in or out) recovers,
- that audio survives the screen locking.

See `docs/mobile-plan.md` for the measurement plan and
`docs/latency-measurement.md` for how a real figure has to be obtained.

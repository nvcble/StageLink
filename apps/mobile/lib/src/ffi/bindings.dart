// Low-level bindings to crates/receiver-ffi.
//
// Hand-written, not generated. `ffigen` needs libclang, which is not present
// on every machine that has to build this app — including the one it was
// written on — and a developer should not need an LLVM install to compile a
// Flutter project.
//
// The risk that buys is layout drift: Dart and Rust cannot check each other,
// so a field inserted on one side silently shifts every field after it and
// the symptom is nonsense on screen rather than an error. Two things guard it
// instead, and both run in the test suite:
//
//   * `slr_abi_version()` is checked at load. It is bumped whenever a
//     signature or a layout changes.
//   * `sizeOf<SlrStats>()` is asserted against the size Rust pins in
//     `crates/receiver-ffi/src/types.rs`. A mismatch fails a test rather than
//     producing a wrong buffer figure in a performer's hand.
//
// Keep in step with `crates/receiver-ffi/include/stagelink_receiver.h`, which
// is the authority and carries the ownership and thread rules.
//
// ignore_for_file: non_constant_identifier_names, camel_case_types
// ignore_for_file: library_private_types_in_public_api

import 'dart:ffi';

import 'package:ffi/ffi.dart';

/// Status codes. Negative on failure, so the render functions can return
/// either a frame count or a status in one value.
abstract final class SlrStatus {
  static const int ok = 0;
  static const int invalidArgument = -1;

  /// Not a usable StageLink join code. The user should rescan.
  static const int badJoinCode = -2;

  /// Different major protocol version. Rescanning will not help.
  static const int incompatibleVersion = -3;

  /// The desktop did not answer. Same network?
  static const int couldNotConnect = -4;

  /// The desktop refused the code. Ask the operator for a fresh one.
  static const int refused = -5;
  static const int audioPortUnavailable = -6;
  static const int internal = -7;

  /// A panic was caught at the boundary. Always a bug in the library.
  static const int panicked = -8;
}

/// Opaque receiver handle.
final class SlrReceiver extends Opaque {}

/// What the app tells the library about itself when joining.
final class SlrConfig extends Struct {
  external Pointer<Utf8> device_name;
  external Pointer<Utf8> platform;
  external Pointer<Utf8> platform_version;
  external Pointer<Utf8> app_version;

  /// Long-term public key, hex. Not a secret: it is how a desktop recognises
  /// this device across reconnects, so the operator's label and any
  /// revocation stick to it.
  external Pointer<Utf8> public_key;

  @Uint32()
  external int preset;

  @Bool()
  external bool ephemeral_audio_port;
}

/// Three-band EQ, as the performer set it.
final class SlrEq extends Struct {
  @Float()
  external double low_hz;
  @Float()
  external double low_db;
  @Float()
  external double low_q;
  @Float()
  external double mid_hz;
  @Float()
  external double mid_db;
  @Float()
  external double mid_q;
  @Float()
  external double high_hz;
  @Float()
  external double high_db;
  @Float()
  external double high_q;
}

/// Everything the UI needs, in one poll.
///
/// Field order and types must match `SlrStats` in
/// `crates/receiver-ffi/src/types.rs` exactly.
final class SlrStats extends Struct {
  /// 0 disconnected, 1 connecting, 2 connected, 3 failed.
  @Uint32()
  external int connection_state;

  /// True once the desktop has sent audio parameters. Before that, silence is
  /// expected rather than a fault — the operator has not started the stream.
  @Bool()
  external bool has_stream;

  @Bool()
  external bool playing;

  @Uint64()
  external int packets_received;
  @Uint64()
  external int packets_lost;
  @Float()
  external double loss_percent;
  @Uint64()
  external int duplicates;
  @Uint64()
  external int reordered;

  /// Datagrams that were not valid StageLink packets.
  @Uint64()
  external int rejected;

  /// Datagrams from an address other than the desktop we joined.
  @Uint64()
  external int foreign_datagrams;

  /// Jitter-buffer depth, in milliseconds.
  @Float()
  external double buffer_ms;

  /// Separate gaps concealed. More telling than a sample count: fifty short
  /// gaps and one long one sound nothing alike.
  @Uint64()
  external int concealed_runs;
  @Float()
  external double concealed_ms;
  @Uint64()
  external int resyncs;

  /// Clock difference the drift controller settled on, in ppm.
  @Float()
  external double drift_ppm;

  /// Samples the safety clip caught. Should always be zero; non-zero means
  /// the limiter is misbehaving.
  @Uint64()
  external int clipped;

  @Uint64()
  external int heartbeats;
  @Uint64()
  external int missed_heartbeats;

  /// Control-channel round-trip time, or −1 if none has been measured.
  ///
  /// **Not audio latency.** It is the control path over TCP. A performer
  /// reading "4 ms" would conclude their monitor is 4 ms behind, and it is
  /// not.
  @Float()
  external double control_rtt_ms;
}

// -- function signatures ----------------------------------------------------

typedef _AbiVersionNative = Uint32 Function();
typedef _AbiVersion = int Function();

typedef _InitLoggingNative = Void Function();
typedef _InitLogging = void Function();

typedef _JoinNative = Int32 Function(
    Pointer<Utf8>, Pointer<SlrConfig>, Pointer<Pointer<SlrReceiver>>);
typedef _Join = int Function(
    Pointer<Utf8>, Pointer<SlrConfig>, Pointer<Pointer<SlrReceiver>>);

typedef _LeaveNative = Void Function(Pointer<SlrReceiver>);
typedef _Leave = void Function(Pointer<SlrReceiver>);

typedef _RenderNative = Int32 Function(
    Pointer<SlrReceiver>, Pointer<Float>, Uint32);
typedef _Render = int Function(Pointer<SlrReceiver>, Pointer<Float>, int);

typedef _SetGainNative = Int32 Function(Pointer<SlrReceiver>, Float);
typedef _SetGain = int Function(Pointer<SlrReceiver>, double);

typedef _SetMutedNative = Int32 Function(Pointer<SlrReceiver>, Bool);
typedef _SetMuted = int Function(Pointer<SlrReceiver>, bool);

typedef _SetEqNative = Int32 Function(Pointer<SlrReceiver>, Pointer<SlrEq>);
typedef _SetEq = int Function(Pointer<SlrReceiver>, Pointer<SlrEq>);

typedef _SetUint32Native = Int32 Function(Pointer<SlrReceiver>, Uint32);
typedef _SetUint32 = int Function(Pointer<SlrReceiver>, int);

typedef _StatsNative = Int32 Function(
    Pointer<SlrReceiver>, Pointer<SlrStats>);
typedef _Stats = int Function(Pointer<SlrReceiver>, Pointer<SlrStats>);

/// The loaded library.
///
/// The function fields use private typedefs: they are an implementation
/// detail of the binding layer, and `receiver.dart` is what the rest of the
/// app uses. Exposing them publicly would invite calling the raw ABI from a
/// widget, which is exactly what the wrapper exists to prevent.
class StageLinkBindings {
  StageLinkBindings(DynamicLibrary library)
      : slr_abi_version =
            library.lookupFunction<_AbiVersionNative, _AbiVersion>('slr_abi_version'),
        slr_init_logging =
            library.lookupFunction<_InitLoggingNative, _InitLogging>('slr_init_logging'),
        slr_join = library.lookupFunction<_JoinNative, _Join>('slr_join'),
        slr_leave = library.lookupFunction<_LeaveNative, _Leave>('slr_leave'),
        slr_render = library.lookupFunction<_RenderNative, _Render>('slr_render'),
        slr_render_stereo =
            library.lookupFunction<_RenderNative, _Render>('slr_render_stereo'),
        slr_set_gain_db =
            library.lookupFunction<_SetGainNative, _SetGain>('slr_set_gain_db'),
        slr_set_muted =
            library.lookupFunction<_SetMutedNative, _SetMuted>('slr_set_muted'),
        slr_set_eq = library.lookupFunction<_SetEqNative, _SetEq>('slr_set_eq'),
        slr_set_safety_profile = library
            .lookupFunction<_SetUint32Native, _SetUint32>('slr_set_safety_profile'),
        slr_set_preset =
            library.lookupFunction<_SetUint32Native, _SetUint32>('slr_set_preset'),
        slr_stats = library.lookupFunction<_StatsNative, _Stats>('slr_stats');

  final _AbiVersion slr_abi_version;
  final _InitLogging slr_init_logging;
  final _Join slr_join;
  final _Leave slr_leave;
  final _Render slr_render;
  final _Render slr_render_stereo;
  final _SetGain slr_set_gain_db;
  final _SetMuted slr_set_muted;
  final _SetEq slr_set_eq;
  final _SetUint32 slr_set_safety_profile;
  final _SetUint32 slr_set_preset;
  final _Stats slr_stats;
}

/// ABI version this app was built against.
///
/// Checked at load, because a stale `.so` from a previous build would
/// otherwise read the wrong bytes out of these structs and the symptom would
/// be nonsense rather than an error.
const int expectedAbiVersion = 1;

/// Size Rust pins for [SlrStats], from `crates/receiver-ffi/src/types.rs`.
///
/// Asserted in the tests. This is the cheap half of what `ffigen` would have
/// bought, and it catches the failure that actually matters.
const int expectedStatsSize = 136;

// The safe wrapper around the C ABI.
//
// Everything above this file works in Dart types and never touches a pointer.
// The ownership and thread rules from
// `crates/receiver-ffi/include/stagelink_receiver.h` are enforced here, in one
// place, rather than trusted to every caller.

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

import 'bindings.dart';

/// Why a join failed, in terms of what the person holding the phone should do.
enum JoinFailure {
  /// Not a usable StageLink code. Rescan it.
  badCode,

  /// The desktop speaks a different major protocol version. Rescanning will
  /// not help; one of the two needs updating.
  incompatibleVersion,

  /// The desktop did not answer. Usually the wrong network.
  couldNotConnect,

  /// The desktop refused the code — expired, revoked, or the session is full.
  refused,

  /// The audio port is already in use on this device.
  audioPortBusy,

  /// Something went wrong inside the library.
  internal;

  static JoinFailure fromStatus(int status) => switch (status) {
        SlrStatus.badJoinCode => JoinFailure.badCode,
        SlrStatus.incompatibleVersion => JoinFailure.incompatibleVersion,
        SlrStatus.couldNotConnect => JoinFailure.couldNotConnect,
        SlrStatus.refused => JoinFailure.refused,
        SlrStatus.audioPortUnavailable => JoinFailure.audioPortBusy,
        _ => JoinFailure.internal,
      };

  /// What to tell the user.
  ///
  /// Phrased as an action, because a performer reading this is standing on a
  /// stage and needs to know what to do, not what went wrong.
  String get message => switch (this) {
        JoinFailure.badCode =>
          'That is not a StageLink code. Point the camera at the QR code on '
              'the operator screen.',
        JoinFailure.incompatibleVersion =>
          'This app and that desktop are different versions. One of them '
              'needs updating.',
        JoinFailure.couldNotConnect =>
          'Could not reach the desktop. Check that this phone is on the same '
              'Wi-Fi network.',
        JoinFailure.refused =>
          'The code was refused. It may have expired, so ask for a fresh one.',
        JoinFailure.audioPortBusy =>
          'The audio port is already in use on this device.',
        JoinFailure.internal => 'Something went wrong joining the desktop.',
      };
}

/// Thrown when joining fails.
class JoinException implements Exception {
  const JoinException(this.failure);

  final JoinFailure failure;

  @override
  String toString() => 'JoinException(${failure.name}): ${failure.message}';
}

/// Thrown when the native library cannot be used at all.
class ReceiverLibraryException implements Exception {
  const ReceiverLibraryException(this.message);

  final String message;

  @override
  String toString() => 'ReceiverLibraryException: $message';
}

/// Buffer preset: the trade between latency and resilience.
enum BufferPreset {
  /// 20 ms. A clean, dedicated access point and a short distance.
  tight(0, '20 ms'),

  /// 40 ms. The default.
  normal(1, '40 ms'),

  /// 80 ms. A busy room, a shared network, or the back of a hall.
  safe(2, '80 ms');

  const BufferPreset(this.code, this.label);

  final int code;
  final String label;
}

/// How loud the receiver is permitted to get.
///
/// Three coarse choices rather than a decibel field, which would invite being
/// set to the maximum and forgotten. The library clamps the output ceiling to
/// whichever is chosen, and hard-clips afterwards regardless.
enum SafetyProfile {
  /// Minus 6 dBFS. The default, and what in-ear monitoring should use.
  protected(0),

  /// Minus 3 dBFS. For a performer who needs more and understands the trade.
  standard(1),

  /// Full scale. Not for in-ear use.
  unrestricted(2);

  const SafetyProfile(this.code);

  final int code;
}

/// A snapshot of what the receiver is doing.
class ReceiverStats {
  const ReceiverStats({
    required this.connectionState,
    required this.hasStream,
    required this.playing,
    required this.packetsReceived,
    required this.packetsLost,
    required this.lossPercent,
    required this.duplicates,
    required this.reordered,
    required this.rejected,
    required this.foreignDatagrams,
    required this.bufferMs,
    required this.concealedRuns,
    required this.concealedMs,
    required this.resyncs,
    required this.driftPpm,
    required this.clipped,
    required this.heartbeats,
    required this.missedHeartbeats,
    required this.controlRttMs,
  });

  const ReceiverStats.disconnected()
      : connectionState = 0,
        hasStream = false,
        playing = false,
        packetsReceived = 0,
        packetsLost = 0,
        lossPercent = 0,
        duplicates = 0,
        reordered = 0,
        rejected = 0,
        foreignDatagrams = 0,
        bufferMs = 0,
        concealedRuns = 0,
        concealedMs = 0,
        resyncs = 0,
        driftPpm = 0,
        clipped = 0,
        heartbeats = 0,
        missedHeartbeats = 0,
        controlRttMs = null;

  final int connectionState;

  /// True once the desktop has sent audio parameters.
  ///
  /// Before that, silence is expected rather than a fault — the operator has
  /// not started the stream. Showing "no audio" to a performer before the
  /// band has started would be wrong.
  final bool hasStream;

  final bool playing;
  final int packetsReceived;
  final int packetsLost;
  final double lossPercent;
  final int duplicates;
  final int reordered;
  final int rejected;
  final int foreignDatagrams;
  final double bufferMs;
  final int concealedRuns;
  final double concealedMs;
  final int resyncs;
  final double driftPpm;

  /// Samples the safety clip caught. Should always be zero.
  final int clipped;

  final int heartbeats;
  final int missedHeartbeats;

  /// Control-channel round-trip time, or null if none has been measured.
  ///
  /// **Not audio latency.** Never label it as such in the UI.
  final double? controlRttMs;

  bool get isConnected => connectionState == 2;
  bool get hasFailed => connectionState == 3;
}

/// Loads the native library for this platform.
DynamicLibrary _open() {
  if (Platform.isAndroid) {
    return DynamicLibrary.open('libstagelink_receiver.so');
  }
  if (Platform.isIOS || Platform.isMacOS) {
    // Statically linked into the app binary on iOS, so the process itself
    // holds the symbols.
    return DynamicLibrary.process();
  }
  if (Platform.isWindows) {
    // The desktop build exists so the FFI seam can be driven from Dart tests
    // with no phone attached, which is where most of its bugs are found.
    const candidates = <String>[
      'stagelink_receiver.dll',
      r'..\..\crates\target\mobile\stagelink_receiver.dll',
      r'crates\target\mobile\stagelink_receiver.dll',
    ];
    for (final candidate in candidates) {
      try {
        return DynamicLibrary.open(candidate);
      } on ArgumentError {
        continue;
      }
    }
    throw const ReceiverLibraryException(
      'stagelink_receiver.dll was not found. Build it with: '
      'cargo build -p receiver-ffi --profile mobile',
    );
  }
  if (Platform.isLinux) {
    return DynamicLibrary.open('libstagelink_receiver.so');
  }
  throw ReceiverLibraryException(
      'unsupported platform ${Platform.operatingSystem}');
}

/// The native receiver, wrapped.
///
/// One instance per app. [join] may be called again after [leave].
class NativeReceiver {
  NativeReceiver._(this._bindings);

  /// Loads the library and checks the ABI.
  ///
  /// The checks are not ceremony. A stale library from a previous build reads
  /// the wrong bytes out of the stats struct, and the symptom is a
  /// plausible-looking wrong number in a performer's hand rather than a
  /// crash.
  factory NativeReceiver.load({DynamicLibrary? library}) {
    final bindings = StageLinkBindings(library ?? _open());
    final version = bindings.slr_abi_version();
    if (version != expectedAbiVersion) {
      throw ReceiverLibraryException(
        'the native library speaks ABI version $version, this app expects '
        '$expectedAbiVersion. Rebuild one of them.',
      );
    }
    if (sizeOf<SlrStats>() != expectedStatsSize) {
      throw ReceiverLibraryException(
        'SlrStats is ${sizeOf<SlrStats>()} bytes here and $expectedStatsSize '
        'bytes in Rust. The bindings are out of step with the header.',
      );
    }
    return NativeReceiver._(bindings);
  }

  final StageLinkBindings _bindings;
  Pointer<SlrReceiver> _handle = nullptr;

  bool get isJoined => _handle != nullptr;

  /// The handle as an integer, for passing to the platform audio sink.
  ///
  /// The sink *borrows* it: ownership stays here, which is why the sink has to
  /// be stopped before [leave] is called. Freeing the receiver while Oboe is
  /// still calling `slr_render` on it would be a use-after-free in somebody's
  /// ears.
  int get handleAddress => _handle.address;

  /// Turns on the library's own logging. Safe to call more than once.
  void initLogging() => _bindings.slr_init_logging();

  /// Joins a desktop from a scanned QR payload.
  ///
  /// Blocks for up to a second or two, so call it off the UI isolate.
  void join({
    required String qrPayload,
    required String deviceName,
    required String platform,
    String? platformVersion,
    required String appVersion,
    required String publicKey,
    BufferPreset preset = BufferPreset.normal,
    bool ephemeralAudioPort = false,
  }) {
    if (isJoined) {
      throw const ReceiverLibraryException('already joined; leave first');
    }

    // Every allocation is freed in the `finally`, including on the throw
    // paths. The library copies anything it keeps, so none of this outlives
    // the call.
    final qr = qrPayload.toNativeUtf8();
    final name = deviceName.toNativeUtf8();
    final platformName = platform.toNativeUtf8();
    final platformVersionPtr = platformVersion?.toNativeUtf8();
    final appVersionPtr = appVersion.toNativeUtf8();
    final key = publicKey.toNativeUtf8();
    final config = calloc<SlrConfig>();
    final out = calloc<Pointer<SlrReceiver>>();

    try {
      config.ref
        ..device_name = name
        ..platform = platformName
        ..platform_version = platformVersionPtr ?? nullptr
        ..app_version = appVersionPtr
        ..public_key = key
        ..preset = preset.code
        ..ephemeral_audio_port = ephemeralAudioPort;

      final status = _bindings.slr_join(qr, config, out);
      if (status != SlrStatus.ok) {
        throw JoinException(JoinFailure.fromStatus(status));
      }
      _handle = out.value;
    } finally {
      calloc.free(out);
      calloc.free(config);
      calloc.free(key);
      calloc.free(appVersionPtr);
      if (platformVersionPtr != null) calloc.free(platformVersionPtr);
      calloc.free(platformName);
      calloc.free(name);
      calloc.free(qr);
    }
  }

  /// Leaves, so the desktop drops the roster entry at once rather than after
  /// five missed heartbeats.
  void leave() {
    if (!isJoined) return;
    _bindings.slr_leave(_handle);
    _handle = nullptr;
  }

  /// Reads the current statistics, and services the control channel.
  ///
  /// Poll this at about 10 Hz: it is also what keeps the telemetry the
  /// desktop sees up to date, so an app that stops polling stops reporting.
  ReceiverStats stats() {
    if (!isJoined) return const ReceiverStats.disconnected();
    final out = calloc<SlrStats>();
    try {
      if (_bindings.slr_stats(_handle, out) != SlrStatus.ok) {
        return const ReceiverStats.disconnected();
      }
      final raw = out.ref;
      return ReceiverStats(
        connectionState: raw.connection_state,
        hasStream: raw.has_stream,
        playing: raw.playing,
        packetsReceived: raw.packets_received,
        packetsLost: raw.packets_lost,
        lossPercent: raw.loss_percent,
        duplicates: raw.duplicates,
        reordered: raw.reordered,
        rejected: raw.rejected,
        foreignDatagrams: raw.foreign_datagrams,
        bufferMs: raw.buffer_ms,
        concealedRuns: raw.concealed_runs,
        concealedMs: raw.concealed_ms,
        resyncs: raw.resyncs,
        driftPpm: raw.drift_ppm,
        clipped: raw.clipped,
        heartbeats: raw.heartbeats,
        missedHeartbeats: raw.missed_heartbeats,
        // Negative means "not measured", which is not the same as zero.
        controlRttMs: raw.control_rtt_ms < 0 ? null : raw.control_rtt_ms,
      );
    } finally {
      calloc.free(out);
    }
  }

  /// Sets the performer's volume. Clamped by the library to a safe range.
  void setGainDb(double gainDb) {
    if (isJoined) _bindings.slr_set_gain_db(_handle, gainDb);
  }

  /// Mutes or unmutes. Ramped over 5 ms, so it does not click.
  void setMuted(bool muted) {
    if (isJoined) _bindings.slr_set_muted(_handle, muted);
  }

  void setPreset(BufferPreset preset) {
    if (isJoined) _bindings.slr_set_preset(_handle, preset.code);
  }

  /// The one coarse call that can raise the output ceiling.
  void setSafetyProfile(SafetyProfile profile) {
    if (isJoined) _bindings.slr_set_safety_profile(_handle, profile.code);
  }

  /// Sets the three-band EQ. Every parameter is clamped by the library.
  void setEq({
    required double lowDb,
    required double midDb,
    required double highDb,
    double lowHz = 150,
    double midHz = 1000,
    double highHz = 4000,
    double lowQ = 0.707,
    double midQ = 0.9,
    double highQ = 0.707,
  }) {
    if (!isJoined) return;
    final eq = calloc<SlrEq>();
    try {
      eq.ref
        ..low_hz = lowHz
        ..low_db = lowDb
        ..low_q = lowQ
        ..mid_hz = midHz
        ..mid_db = midDb
        ..mid_q = midQ
        ..high_hz = highHz
        ..high_db = highDb
        ..high_q = highQ;
      _bindings.slr_set_eq(_handle, eq);
    } finally {
      calloc.free(eq);
    }
  }

  /// Renders mono frames into a native buffer.
  ///
  /// **Not for the UI isolate.** On Android the real audio callback calls
  /// `slr_render` from C++ without coming through Dart at all; this exists so
  /// the render path can be exercised from a test.
  int renderForTest(Pointer<Float> out, int frames) {
    if (!isJoined) return SlrStatus.invalidArgument;
    return _bindings.slr_render(_handle, out, frames);
  }

  /// The raw bindings, for tests that need to poke the ABI directly.
  StageLinkBindings get bindings => _bindings;
}

// The app's state, and the only place that talks to the native receiver.
//
// Providers are written by hand rather than generated: `riverpod_generator`
// needs an analyzer version `freezed` does not, and the generated code would
// have been these same twenty lines.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../audio/sink_channel.dart';
import '../data/identity.dart';
import '../ffi/receiver.dart';
import 'monitor_settings.dart';

/// How often the stats are polled.
///
/// Also what keeps the telemetry the desktop sees up to date — `slr_stats`
/// services the control channel — so an app that stops polling stops
/// reporting, and a 10 Hz poll keeps the operator's roster live without
/// costing anything measurable.
const statsInterval = Duration(milliseconds: 100);

/// Where the session is.
enum SessionPhase {
  /// Nothing scanned yet.
  idle,

  /// Joining, which takes a second or two.
  joining,

  /// Joined. Audio may or may not be flowing — the operator may not have
  /// started the stream yet, and that is not a fault.
  joined,

  /// The join failed, or the connection dropped.
  failed,
}

/// Everything a screen needs to know.
class SessionState {
  const SessionState({
    this.phase = SessionPhase.idle,
    this.stats = const ReceiverStats.disconnected(),
    this.settings = const MonitorSettings(),
    this.failure,
    this.desktopName,
  });

  final SessionPhase phase;
  final ReceiverStats stats;
  final MonitorSettings settings;

  /// Why it failed, phrased as something to do about it.
  final JoinFailure? failure;

  final String? desktopName;

  bool get isJoined => phase == SessionPhase.joined;

  /// True when the operator has started a stream and audio should be audible.
  ///
  /// Distinct from [isJoined]: a performer who has joined before the band
  /// starts should be told "waiting for the desktop", not "no audio".
  bool get isReceiving => isJoined && stats.hasStream;

  /// True when something is wrong that a person should act on.
  ///
  /// Deliberately narrow. A buffer that dipped once is not a fault, and an app
  /// that cries wolf during a service is worse than one that says nothing.
  bool get hasAudibleProblem =>
      isReceiving && (stats.lossPercent > 2 || stats.missedHeartbeats > 0);

  SessionState copyWith({
    SessionPhase? phase,
    ReceiverStats? stats,
    MonitorSettings? settings,
    JoinFailure? failure,
    bool clearFailure = false,
    String? desktopName,
  }) {
    return SessionState(
      phase: phase ?? this.phase,
      stats: stats ?? this.stats,
      settings: settings ?? this.settings,
      failure: clearFailure ? null : (failure ?? this.failure),
      desktopName: desktopName ?? this.desktopName,
    );
  }
}

/// Drives the native receiver.
class SessionController extends Notifier<SessionState> {
  SessionController({
    NativeReceiver? receiver,
    IdentityStore? identityStore,
    AudioSinkChannel? sink,
    this.appVersion = '0.1.0',
    this.deviceName = 'StageLink Mobile',
    this.platform = 'android',
    this.ephemeralAudioPort = false,
  })  : _injectedReceiver = receiver,
        _identityStore = identityStore ?? IdentityStore(),
        _sink = sink ??
            AudioSinkChannel(
              // Only Android has a native sink. On the desktop the join and
              // the UI still work, which is what the tests need.
              supported: defaultTargetPlatform == TargetPlatform.android,
            );

  final NativeReceiver? _injectedReceiver;
  final IdentityStore _identityStore;
  final AudioSinkChannel _sink;
  final String appVersion;
  final String deviceName;
  final String platform;

  /// Only for running more than one receiver on one machine, which a phone
  /// never does.
  final bool ephemeralAudioPort;

  NativeReceiver? _receiver;
  Timer? _poll;
  DeviceIdentity? _identity;

  @override
  SessionState build() {
    ref.onDispose(() {
      _poll?.cancel();
      // Same ordering as `leave`, and for the same reason: the sink must stop
      // before the handle is freed, or Oboe is left rendering from freed
      // memory. Not awaited — dispose cannot be async — so `stop` is
      // deliberately synchronous on the native side.
      _sink.stop();
      _receiver?.leave();
    });
    return const SessionState();
  }

  /// The native library, loaded on first use.
  ///
  /// Lazily, so a UI test that never joins does not need the library present.
  NativeReceiver _library() {
    final existing = _receiver ?? _injectedReceiver;
    if (existing != null) {
      _receiver = existing;
      return existing;
    }
    final loaded = NativeReceiver.load()..initLogging();
    _receiver = loaded;
    return loaded;
  }

  /// This device's identity, created on first run.
  Future<DeviceIdentity> identity() async =>
      _identity ??= await _identityStore.loadOrCreate();

  /// Joins a desktop from a scanned QR payload.
  Future<void> join(String qrPayload) async {
    if (state.phase == SessionPhase.joining) return;
    state = state.copyWith(phase: SessionPhase.joining, clearFailure: true);

    try {
      final identity = await this.identity();
      final receiver = _library();

      // `slr_join` blocks for up to a second or two. Pushed off the UI
      // isolate so the spinner keeps turning.
      await Future(() => receiver.join(
            qrPayload: qrPayload,
            deviceName: deviceName,
            platform: platform,
            appVersion: appVersion,
            publicKey: identity.publicKeyHex,
            preset: state.settings.preset,
            ephemeralAudioPort: ephemeralAudioPort,
          ));

      // Settings are re-applied after joining: the handle is new, so whatever
      // the performer had set last time has to be pushed into it.
      _applySettings(state.settings);

      // The sink starts last, and only once the receiver exists: Oboe begins
      // calling `slr_render` the moment the stream starts, and starting it
      // before there is a handle to render from would be a null dereference on
      // the audio thread.
      await _startSink(receiver);

      state = state.copyWith(phase: SessionPhase.joined, clearFailure: true);
      _startPolling();
    } on JoinException catch (error) {
      state = state.copyWith(
        phase: SessionPhase.failed,
        failure: error.failure,
      );
    } on ReceiverLibraryException {
      state = state.copyWith(
        phase: SessionPhase.failed,
        failure: JoinFailure.internal,
      );
    }
  }

  /// Starts the platform audio output.
  ///
  /// A failure here is reported but does not undo the join: the performer is
  /// connected and the operator can see them, which is more useful than
  /// pretending the whole thing failed. They are told the output could not be
  /// opened.
  Future<void> _startSink(NativeReceiver receiver) async {
    try {
      await _sink.start(
        handleAddress: receiver.handleAddress,
        // 48 kHz is what every supported configuration uses. The desktop's
        // real rate arrives in the welcome; if it differs, the receiver
        // rebuilds its audio path around it and the sink follows on rejoin.
        sampleRate: 48000,
      );
    } on AudioSinkException {
      state = state.copyWith(failure: JoinFailure.internal);
    }
  }

  /// Leaves, so the desktop drops the roster entry at once.
  ///
  /// The order is load-bearing: the sink stops **before** the receiver is
  /// freed. Oboe may be inside `slr_render` at any moment, and freeing the
  /// handle out from under it would be a use-after-free on the audio thread —
  /// in somebody's ears.
  Future<void> leave() async {
    _poll?.cancel();
    _poll = null;
    await _sink.stop();
    _receiver?.leave();
    state = const SessionState();
  }

  void setGainDb(double gainDb) =>
      _update(state.settings.copyWith(gainDb: gainDb));

  void setGainFraction(double fraction) =>
      _update(state.settings.withGainFraction(fraction));

  void toggleMute() =>
      _update(state.settings.copyWith(muted: !state.settings.muted));

  void setMuted(bool muted) =>
      _update(state.settings.copyWith(muted: muted));

  void setEq({double? lowDb, double? midDb, double? highDb}) =>
      _update(state.settings.copyWith(
        lowDb: lowDb ?? state.settings.lowDb,
        midDb: midDb ?? state.settings.midDb,
        highDb: highDb ?? state.settings.highDb,
      ));

  void setPreset(BufferPreset preset) =>
      _update(state.settings.copyWith(preset: preset));

  /// The one coarse call that can raise the output ceiling.
  void setSafetyProfile(SafetyProfile profile) =>
      _update(state.settings.copyWith(safety: profile));

  void _update(MonitorSettings settings) {
    final clamped = settings.clamped();
    state = state.copyWith(settings: clamped);
    _applySettings(clamped);
  }

  void _applySettings(MonitorSettings settings) {
    final receiver = _receiver;
    if (receiver == null || !receiver.isJoined) return;
    receiver
      ..setSafetyProfile(settings.safety)
      ..setGainDb(settings.gainDb)
      ..setMuted(settings.muted)
      ..setPreset(settings.preset)
      ..setEq(
        lowDb: settings.lowDb,
        midDb: settings.midDb,
        highDb: settings.highDb,
      );
  }

  void _startPolling() {
    _poll?.cancel();
    _poll = Timer.periodic(statsInterval, (_) => _pollOnce());
  }

  void _pollOnce() {
    final receiver = _receiver;
    if (receiver == null || !receiver.isJoined) return;
    final stats = receiver.stats();

    // A dropped control channel is a failure the performer must see: the
    // desktop will stop sending audio five heartbeats later, and silence with
    // no explanation is the worst thing this app can do.
    if (stats.hasFailed) {
      _poll?.cancel();
      _poll = null;
      state = state.copyWith(
        phase: SessionPhase.failed,
        stats: stats,
        failure: JoinFailure.couldNotConnect,
      );
      return;
    }

    state = state.copyWith(stats: stats);
  }

  /// For tests: runs one poll synchronously instead of waiting on the timer.
  void pollForTest() => _pollOnce();
}

/// The session.
final sessionProvider = NotifierProvider<SessionController, SessionState>(
  SessionController.new,
);

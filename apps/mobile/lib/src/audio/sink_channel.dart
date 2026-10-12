// The platform audio sink, from Dart's side.
//
// Five calls, no audio. The samples go from Rust to Oboe without passing
// through Dart or the JVM at all; this starts and stops the stream and asks
// for the one figure only the device can measure.

import 'package:flutter/services.dart';

/// Why the sink could not start.
class AudioSinkException implements Exception {
  const AudioSinkException(this.message);

  final String message;

  @override
  String toString() => 'AudioSinkException: $message';
}

/// The Android audio output.
///
/// On platforms without one — the desktop, where this app also runs for
/// testing — every call is a no-op and [isPlaying] stays false, so a test can
/// exercise the join and the UI without an audio device.
class AudioSinkChannel {
  AudioSinkChannel({MethodChannel? channel, this.supported = true})
      : _channel = channel ?? const MethodChannel(_name);

  static const _name = 'nz.co.stagelink/audio_sink';

  final MethodChannel _channel;

  /// False on platforms with no native sink.
  final bool supported;

  bool _playing = false;
  bool get isPlaying => _playing;

  /// Starts playback from a receiver handle.
  ///
  /// `handleAddress` is the pointer `slr_join` produced. Ownership stays in
  /// Dart: the sink borrows it for as long as it plays, which is why [stop]
  /// must be called before the receiver is left.
  Future<int> start({
    required int handleAddress,
    required int sampleRate,
  }) async {
    if (!supported) return sampleRate;
    if (handleAddress == 0) {
      throw const AudioSinkException('no receiver to play from');
    }

    try {
      final actual = await _channel.invokeMethod<int>('start', <String, Object>{
        'handle': handleAddress,
        'sampleRate': sampleRate,
      });
      _playing = true;
      // The device may not give the rate that was asked for. The caller needs
      // to know, because a receiver resampling to the wrong rate plays the mix
      // at the wrong pitch.
      return actual ?? sampleRate;
    } on PlatformException catch (error) {
      throw AudioSinkException(error.message ?? 'the audio output refused');
    } on MissingPluginException {
      // Running on a platform with no sink. Not an error: the join and the UI
      // still work, which is what a desktop test needs.
      return sampleRate;
    }
  }

  /// Stops playback. Must be called before the receiver handle is freed.
  Future<void> stop() async {
    if (!supported || !_playing) return;
    _playing = false;
    try {
      await _channel.invokeMethod<void>('stop');
    } on MissingPluginException {
      // Nothing to stop.
    }
  }

  /// Whether a route change tore the stream down.
  ///
  /// Polled rather than pushed: a push would mean a JNI call up from Oboe's
  /// error callback, and the audio side deliberately never calls into the JVM.
  Future<bool> needsRebuild() async {
    if (!supported || !_playing) return false;
    try {
      return await _channel.invokeMethod<bool>('needsRebuild') ?? false;
    } on MissingPluginException {
      return false;
    }
  }

  /// Output latency as the platform reports it, or null if it cannot say.
  ///
  /// The largest single term in the end-to-end latency, and the only one
  /// measurable on the device. Null rather than zero when unavailable: a
  /// receiver that cannot measure something must say so.
  Future<double?> outputLatencyMs() async {
    if (!supported || !_playing) return null;
    try {
      return await _channel.invokeMethod<double>('outputLatencyMs');
    } on MissingPluginException {
      return null;
    }
  }

  /// Render failures since the stream opened. Should always be zero.
  Future<int> renderFailures() async {
    if (!supported || !_playing) return 0;
    try {
      return await _channel.invokeMethod<int>('renderFailures') ?? 0;
    } on MissingPluginException {
      return 0;
    }
  }
}

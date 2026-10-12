// The performer's own monitor settings.
//
// Immutable, so a widget rebuild cannot half-apply a change, and clamped here
// as well as in Rust — not out of distrust, but because a slider bound to an
// unclamped value would let the UI show a figure the audio path refuses, and
// the two disagreeing on screen is its own bug.

import 'package:freezed_annotation/freezed_annotation.dart';

import '../ffi/receiver.dart';

part 'monitor_settings.freezed.dart';

/// Volume limits, matching `receiver-core`'s own clamps.
///
/// The upper bound is not arbitrary: +40 dB on a quiet mix is not a volume
/// control, it is a hazard, and the limiter should not be the only thing in
/// the way.
const double minGainDb = -60;
const double maxGainDb = 12;

/// EQ band limits, matching `receiver-core`.
const double minBandDb = -18;
const double maxBandDb = 18;

@freezed
abstract class MonitorSettings with _$MonitorSettings {
  const factory MonitorSettings({
    @Default(-6.0) double gainDb,
    @Default(false) bool muted,
    @Default(0.0) double lowDb,
    @Default(0.0) double midDb,
    @Default(0.0) double highDb,
    @Default(BufferPreset.normal) BufferPreset preset,
    @Default(SafetyProfile.protected) SafetyProfile safety,
  }) = _MonitorSettings;

  const MonitorSettings._();

  /// True when the EQ is doing nothing, so the UI can say "flat" rather than
  /// showing three zeroes.
  bool get isEqFlat => lowDb == 0 && midDb == 0 && highDb == 0;

  /// Volume as a 0..1 fraction, for a slider.
  ///
  /// Linear in decibels rather than in amplitude: a fader that is linear in
  /// amplitude spends most of its travel in the top few decibels and is
  /// unusable for setting a monitor level.
  double get gainFraction => ((gainDb - minGainDb) / (maxGainDb - minGainDb))
      .clamp(0.0, 1.0);

  MonitorSettings withGainFraction(double fraction) => copyWith(
        gainDb: minGainDb + fraction.clamp(0.0, 1.0) * (maxGainDb - minGainDb),
      );

  /// Clamped to what the audio path will actually accept.
  MonitorSettings clamped() => copyWith(
        gainDb: gainDb.clamp(minGainDb, maxGainDb),
        lowDb: lowDb.clamp(minBandDb, maxBandDb),
        midDb: midDb.clamp(minBandDb, maxBandDb),
        highDb: highDb.clamp(minBandDb, maxBandDb),
      );
}

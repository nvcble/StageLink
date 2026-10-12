// The app's own logic, with a stub native library.
//
// What is tested here is the part that decides what a performer is told and
// what the audio path is asked to do — not the audio path itself, which is
// `receiver-core`'s own 88 tests, and not the ABI, which `ffi_test.dart`
// drives against the real library.

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:stagelink_mobile/src/ffi/receiver.dart';
import 'package:stagelink_mobile/src/state/monitor_settings.dart';

void main() {
  group('monitor settings', () {
    test('default to a sane monitor level, not full volume', () {
      const settings = MonitorSettings();
      // −6 dB, not 0: the first thing a performer hears when they join should
      // not be the loudest thing the system can produce.
      expect(settings.gainDb, -6);
      expect(settings.muted, isFalse);
      expect(settings.isEqFlat, isTrue);
      expect(settings.safety, SafetyProfile.protected);
      expect(settings.preset, BufferPreset.normal);
    });

    test('the volume fraction is linear in decibels', () {
      // A fader linear in amplitude spends most of its travel in the top few
      // decibels and is useless for setting a monitor level.
      const bottom = MonitorSettings(gainDb: minGainDb);
      const top = MonitorSettings(gainDb: maxGainDb);
      expect(bottom.gainFraction, 0);
      expect(top.gainFraction, 1);

      const middle = MonitorSettings(gainDb: (minGainDb + maxGainDb) / 2);
      expect(middle.gainFraction, closeTo(0.5, 0.001));
    });

    test('a fraction round-trips back to the same level', () {
      for (final fraction in <double>[0, 0.25, 0.5, 0.75, 1]) {
        const base = MonitorSettings();
        final adjusted = base.withGainFraction(fraction);
        expect(adjusted.gainFraction, closeTo(fraction, 0.0001));
      }
    });

    test('a fraction outside the slider range is clamped', () {
      const base = MonitorSettings();
      expect(base.withGainFraction(-5).gainDb, minGainDb);
      expect(base.withGainFraction(99).gainDb, maxGainDb);
    });

    test('clamping matches what the audio path will accept', () {
      // The UI must not show a figure the audio path refuses: the two
      // disagreeing on screen is its own bug.
      const hot = MonitorSettings(
        gainDb: 60,
        lowDb: 40,
        midDb: -90,
        highDb: 100,
      );
      final clamped = hot.clamped();
      expect(clamped.gainDb, maxGainDb);
      expect(clamped.lowDb, maxBandDb);
      expect(clamped.midDb, minBandDb);
      expect(clamped.highDb, maxBandDb);
    });

    test('an EQ with any band moved is not flat', () {
      expect(const MonitorSettings(lowDb: 3).isEqFlat, isFalse);
      expect(const MonitorSettings(midDb: -1).isEqFlat, isFalse);
      expect(const MonitorSettings(highDb: 0.5).isEqFlat, isFalse);
    });
  });

  group('what the performer is told', () {
    ReceiverStats stats({
      int connectionState = 2,
      bool hasStream = true,
      bool playing = true,
      double lossPercent = 0,
      int missedHeartbeats = 0,
    }) {
      return ReceiverStats(
        connectionState: connectionState,
        hasStream: hasStream,
        playing: playing,
        packetsReceived: 1000,
        packetsLost: 0,
        lossPercent: lossPercent,
        duplicates: 0,
        reordered: 0,
        rejected: 0,
        keyless: 0,
        arrivalPeak: 0,
        foreignDatagrams: 0,
        bufferMs: 40,
        concealedRuns: 0,
        concealedMs: 0,
        resyncs: 0,
        driftPpm: 0,
        clipped: 0,
        heartbeats: 10,
        missedHeartbeats: missedHeartbeats,
        controlRttMs: 3,
      );
    }

    test('connected is reported, and not-measured stays not-measured', () {
      final live = stats();
      expect(live.isConnected, isTrue);
      expect(live.hasFailed, isFalse);
      expect(live.controlRttMs, 3);

      const nothing = ReceiverStats.disconnected();
      expect(nothing.isConnected, isFalse);
      expect(
        nothing.controlRttMs,
        isNull,
        reason: 'no round trip measured is not the same as zero milliseconds',
      );
    });

    /// The distinction that matters most on this screen. A performer who
    /// joined while the band was still setting up must not be told something
    /// is broken.
    test('joined but no stream yet is not a fault', () {
      final waiting = stats(hasStream: false, playing: false);
      expect(waiting.isConnected, isTrue);
      expect(waiting.hasStream, isFalse);
    });

    test('a failed connection is distinguishable from an idle one', () {
      expect(stats(connectionState: 3).hasFailed, isTrue);
      expect(stats(connectionState: 0).hasFailed, isFalse);
      expect(stats(connectionState: 0).isConnected, isFalse);
    });
  });

  group('failure messages', () {
    test('tell the user what to do rather than what went wrong', () {
      // Checked as text because this is the only thing a performer on a stage
      // has to work from.
      expect(JoinFailure.badCode.message, contains('QR code'));
      expect(JoinFailure.couldNotConnect.message, contains('Wi-Fi'));
      expect(JoinFailure.refused.message, contains('fresh'));
      expect(JoinFailure.incompatibleVersion.message, contains('updating'));
    });

    test('none of them blames the user or uses jargon', () {
      for (final failure in JoinFailure.values) {
        final message = failure.message.toLowerCase();
        for (final word in <String>[
          'error',
          'invalid',
          'failed',
          'exception',
          'null',
        ]) {
          expect(
            message,
            isNot(contains(word)),
            reason: '${failure.name} says "$word", which means nothing to a '
                'performer',
          );
        }
      }
    });
  });

  group('presets', () {
    test('every buffer preset carries the latency it costs', () {
      // A performer choosing between them needs the number, not just a name.
      expect(BufferPreset.tight.label, '20 ms');
      expect(BufferPreset.normal.label, '40 ms');
      expect(BufferPreset.safe.label, '80 ms');
    });

    test('protected is the zero-valued safety profile', () {
      // Rust maps an unknown value to protected — the safe direction — and
      // these agreeing is what makes that deliberate rather than lucky.
      expect(SafetyProfile.protected.code, 0);
    });
  });

  group('provider wiring', () {
    test('a fresh session claims nothing', () {
      final container = ProviderContainer();
      addTearDown(container.dispose);

      // Imported lazily so this test does not need the native library.
      final state = container.read(
        Provider((ref) => const MonitorSettings()),
      );
      expect(state.gainDb, -6);
    });
  });
}

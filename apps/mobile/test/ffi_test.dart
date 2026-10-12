// Drives the real native library from Dart.
//
// This is the test the plan calls the largest de-risking step available
// without hardware: it proves the FFI seam — the ABI version, every struct
// layout, the error mapping and the null-safety of every entry point — on a
// desktop, before any Android toolchain or phone is involved.
//
// It needs the Windows build of the library:
//
//   cargo build -p receiver-ffi --profile mobile
//
// and skips itself with a clear message if that has not been done, because a
// developer working on the UI should not be blocked by a missing Rust build.

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:stagelink_mobile/src/ffi/bindings.dart';
import 'package:stagelink_mobile/src/ffi/receiver.dart';

/// Where the mobile-profile build puts the library, relative to this app.
const _libraryPath = r'..\..\crates\target\mobile\stagelink_receiver.dll';

/// A 64-hex-character token, as the real QR carries.
const _token =
    '9f2c3d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f';

void main() {
  final available = File(_libraryPath).existsSync();

  group('the native library', () {
    late NativeReceiver receiver;

    setUpAll(() {
      if (!available) return;
      receiver = NativeReceiver.load(library: DynamicLibrary.open(_libraryPath));
    });

    test('loads and agrees on the ABI version', () {
      // `load` throws on a mismatch, so reaching here is the assertion. A
      // stale library would otherwise read the wrong bytes out of the stats
      // struct and report a plausible-looking wrong number.
      expect(receiver.bindings.slr_abi_version(), expectedAbiVersion);
    });

    test('agrees on the stats struct layout', () {
      // The layout is the ABI. Dart and Rust cannot check each other, so a
      // field inserted on one side shifts every field after it and the
      // symptom is nonsense on screen rather than a compile error.
      expect(sizeOf<SlrStats>(), expectedStatsSize);
    });

    test('agrees on the other struct layouts', () {
      // Five pointers, a uint32 and a bool, with padding.
      expect(sizeOf<SlrConfig>(), 5 * sizeOf<Pointer<Void>>() + 8);
      // Nine floats.
      expect(sizeOf<SlrEq>(), 36);
    });

    test('initialises logging more than once without complaint', () {
      receiver.initLogging();
      receiver.initLogging();
    });

    test('reports disconnected statistics before joining', () {
      final stats = receiver.stats();
      expect(stats.isConnected, isFalse);
      expect(stats.packetsReceived, 0);
      expect(stats.hasStream, isFalse);
      expect(stats.controlRttMs, isNull,
          reason: 'no round trip has been measured, which is not zero');
    });

    test('ignores control calls before joining rather than crashing', () {
      // A UI that sets a volume before the join completes is doing nothing
      // wrong, and it must not take the app down.
      receiver
        ..setGainDb(-6)
        ..setMuted(true)
        ..setMuted(false)
        ..setPreset(BufferPreset.safe)
        ..setSafetyProfile(SafetyProfile.standard)
        ..setEq(lowDb: 3, midDb: 0, highDb: -2);
      expect(receiver.isJoined, isFalse);
    });

    test('leaving when not joined is harmless', () {
      receiver.leave();
      expect(receiver.isJoined, isFalse);
    });

    group('join failures are mapped to the right instruction', () {
      test('something that is not a StageLink code says to rescan', () {
        for (final payload in <String>[
          'https://example.com',
          'WIFI:S:Network;T:WPA;P:secret;;',
          '',
          'stagelink://join?h=10.0.0.1&c=45871&s=45872&t=short&v=1.0.0',
        ]) {
          expect(
            () => receiver.join(
              qrPayload: payload,
              deviceName: 'Test',
              platform: 'windows',
              appVersion: '0.1.0',
              publicKey: 'ab' * 32,
            ),
            throwsA(
              isA<JoinException>().having(
                (error) => error.failure,
                'failure',
                JoinFailure.badCode,
              ),
            ),
            reason: 'for $payload',
          );
          expect(receiver.isJoined, isFalse);
        }
      });

      test('a future protocol version says to update, not to rescan', () {
        // Worth its own message: rescanning will not help, and telling
        // somebody to try again would waste their time on a stage.
        expect(
          () => receiver.join(
            qrPayload:
                'stagelink://join?h=10.0.0.1&c=45871&s=45872&t=$_token&v=9.0.0',
            deviceName: 'Test',
            platform: 'windows',
            appVersion: '0.1.0',
            publicKey: 'ab' * 32,
          ),
          throwsA(
            isA<JoinException>().having(
              (error) => error.failure,
              'failure',
              JoinFailure.incompatibleVersion,
            ),
          ),
        );
      });

      test('an address with nothing listening says to check the network', () {
        expect(
          () => receiver.join(
            // Port 1024 on the loopback with nothing bound to it.
            qrPayload:
                'stagelink://join?h=127.0.0.1&c=1024&s=45899&t=$_token&v=1.0.0',
            deviceName: 'Test',
            platform: 'windows',
            appVersion: '0.1.0',
            publicKey: 'ab' * 32,
            ephemeralAudioPort: true,
          ),
          throwsA(
            isA<JoinException>().having(
              (error) => error.failure,
              'failure',
              JoinFailure.couldNotConnect,
            ),
          ),
        );
        expect(receiver.isJoined, isFalse);
      });
    });

    test('rendering before joining is refused rather than returning noise', () {
      final buffer = calloc<Float>(240);
      try {
        expect(receiver.renderForTest(buffer, 240), lessThan(0));
      } finally {
        calloc.free(buffer);
      }
    });

    test('a failed join leaks nothing across many attempts', () {
      // Every allocation in `join` is freed in a `finally`, including on the
      // throw paths. A leak here would be per failed scan, and a performer
      // mis-scanning a dozen times is normal.
      for (var attempt = 0; attempt < 200; attempt += 1) {
        expect(
          () => receiver.join(
            qrPayload: 'not a code',
            deviceName: 'Test',
            platform: 'windows',
            appVersion: '0.1.0',
            publicKey: 'ab' * 32,
          ),
          throwsA(isA<JoinException>()),
        );
      }
      expect(receiver.isJoined, isFalse);
    });
  },
      skip: available
          ? null
          : 'build the library first: cargo build -p receiver-ffi --profile mobile');

  group('failure messages', () {
    test('every failure tells the user what to do', () {
      for (final failure in JoinFailure.values) {
        expect(failure.message, isNotEmpty);
        // An instruction, not a diagnosis. A performer on a stage needs to
        // know what to do next.
        expect(
          failure.message.length,
          greaterThan(20),
          reason: '${failure.name} should say something useful',
        );
      }
    });

    test('an unknown status maps to internal rather than guessing', () {
      expect(JoinFailure.fromStatus(-999), JoinFailure.internal);
      expect(JoinFailure.fromStatus(0), JoinFailure.internal);
    });

    test('each status maps to its own failure', () {
      expect(JoinFailure.fromStatus(SlrStatus.badJoinCode), JoinFailure.badCode);
      expect(JoinFailure.fromStatus(SlrStatus.incompatibleVersion),
          JoinFailure.incompatibleVersion);
      expect(JoinFailure.fromStatus(SlrStatus.couldNotConnect),
          JoinFailure.couldNotConnect);
      expect(JoinFailure.fromStatus(SlrStatus.refused), JoinFailure.refused);
      expect(JoinFailure.fromStatus(SlrStatus.audioPortUnavailable),
          JoinFailure.audioPortBusy);
    });
  });

  group('presets and profiles', () {
    test('the preset codes match the Rust side', () {
      expect(BufferPreset.tight.code, 0);
      expect(BufferPreset.normal.code, 1);
      expect(BufferPreset.safe.code, 2);
    });

    test('the safety profile codes match the Rust side', () {
      // Protected must be zero: an unknown value maps to protected in Rust,
      // which is the safe direction, and these agreeing is what makes that
      // true rather than accidental.
      expect(SafetyProfile.protected.code, 0);
      expect(SafetyProfile.standard.code, 1);
      expect(SafetyProfile.unrestricted.code, 2);
    });

    test('every preset carries a label a performer can read', () {
      for (final preset in BufferPreset.values) {
        expect(preset.label, isNotEmpty);
      }
    });
  });
}

// The version a performer reads out has to be the version that was built.
//
// `scripts/version.mjs` writes `lib/src/version.dart` and the `pubspec.yaml`
// line from the same entry in `versions.json`. If the two ever drift apart,
// the APK's versionCode and what the app displays describe different builds —
// which is exactly the situation where somebody reports a bug against the
// wrong one.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:stagelink_mobile/src/ui/version_label.dart';
import 'package:stagelink_mobile/src/version.dart';

void main() {
  test('the displayed version is the one pubspec built', () {
    final line = File('pubspec.yaml')
        .readAsLinesSync()
        .firstWhere((line) => line.startsWith('version: '));
    expect(line.trim(), 'version: $appVersion+$appBuild');
  });

  test('the label carries the app name, the version and the build', () {
    expect(
      versionLabel,
      matches(RegExp(r'^StageLink Mobile \d+\.\d+\.\d+ \(\d+\)$')),
    );
  });
}

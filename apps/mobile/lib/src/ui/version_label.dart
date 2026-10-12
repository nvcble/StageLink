// The app's own version, somewhere a performer can read it out over the noise
// of a soundcheck.
//
// Two screens show it because they answer different questions: the join screen
// before anything is connected ("what is this phone running?"), and the
// diagnostics sheet during a service. StageLink Mobile and StageLink Core
// version independently and will not usually match — see `versions.json`.

import 'package:flutter/material.dart';

import '../version.dart';

/// `StageLink Mobile 0.1.0 (1)` — the build number is what identifies an APK.
String get versionLabel => 'StageLink Mobile $appVersion ($appBuild)';

class VersionLabel extends StatelessWidget {
  const VersionLabel({super.key});

  @override
  Widget build(BuildContext context) => Text(
        versionLabel,
        textAlign: TextAlign.center,
        style: Theme.of(context).textTheme.bodySmall,
      );
}

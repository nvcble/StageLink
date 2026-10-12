// StageLink Mobile.
//
// The performer's end of a wireless in-ear monitor feed. The audio path and
// the control channel are Rust (`receiver-core`, `receiver-client`) behind a
// narrow C ABI; this app is the screen in front of them.
//
// Nothing here touches audio. The render callback runs in C++ on the audio
// thread and calls into Rust directly — a garbage-collected language on that
// path would cost tens of milliseconds of extra buffer to absorb a single
// pause, which is most of the latency budget the product is built around.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'src/state/session_controller.dart';
import 'src/ui/join_screen.dart';
import 'src/ui/monitor_screen.dart';
import 'src/ui/theme.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();

  // The screen stays on while a performer is monitoring: a phone that sleeps
  // mid-song is one they cannot mute without unlocking it first.
  SystemChrome.setSystemUIOverlayStyle(const SystemUiOverlayStyle(
    statusBarColor: Colors.transparent,
    statusBarIconBrightness: Brightness.light,
  ));

  runApp(const ProviderScope(child: StageLinkApp()));
}

class StageLinkApp extends StatelessWidget {
  const StageLinkApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'StageLink',
      debugShowCheckedModeBanner: false,
      theme: stageTheme(),
      home: const _Root(),
    );
  }
}

/// Shows the viewfinder until joined, then the monitor.
///
/// No navigation stack: there are two states and a performer should never be
/// able to end up on a screen that does not match what the audio is doing.
class _Root extends ConsumerWidget {
  const _Root();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final joined = ref.watch(sessionProvider).isJoined;
    return joined ? const MonitorScreen() : const JoinScreen();
  }
}

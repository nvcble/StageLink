// The monitor screen: the one a performer actually looks at.
//
// Three decisions shape it:
//
// * **Volume and mute are the whole top half.** They are what somebody
//   reaches for mid-song, in the dark, without looking. Everything else is
//   below the fold.
// * **Status says what is true, in words.** "Waiting for the desktop" is not
//   the same as "no audio", and a performer who has joined before the band
//   starts must not be told something is wrong.
// * **Nothing is labelled as latency.** The only round-trip figure available
//   is the control channel over TCP, which is not what a performer would
//   understand by the word, and reporting it as such would be a measurement
//   of the wrong thing.

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../ffi/receiver.dart';
import '../state/monitor_settings.dart';
import '../state/session_controller.dart';
import 'theme.dart';
import 'version_label.dart';

class MonitorScreen extends ConsumerWidget {
  const MonitorScreen({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final session = ref.watch(sessionProvider);
    final controller = ref.read(sessionProvider.notifier);

    return Scaffold(
      appBar: AppBar(
        title: const Text('My monitor'),
        actions: [
          IconButton(
            icon: const Icon(Icons.tune),
            tooltip: 'Sound and buffering',
            onPressed: () => _showSettings(context, ref),
          ),
          IconButton(
            icon: const Icon(Icons.insights_outlined),
            tooltip: 'Connection detail',
            onPressed: () => _showDiagnostics(context, ref),
          ),
        ],
      ),
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            children: [
              _StatusBanner(session: session),
              const SizedBox(height: 28),
              Expanded(
                child: _VolumeControl(
                  settings: session.settings,
                  onChanged: controller.setGainFraction,
                ),
              ),
              const SizedBox(height: 20),
              _MuteButton(
                muted: session.settings.muted,
                onPressed: controller.toggleMute,
              ),
              const SizedBox(height: 12),
              OutlinedButton.icon(
                icon: const Icon(Icons.logout),
                label: const Text('Leave'),
                onPressed: () => controller.leave(),
              ),
            ],
          ),
        ),
      ),
    );
  }

  void _showSettings(BuildContext context, WidgetRef ref) {
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: StageColours.surface,
      isScrollControlled: true,
      builder: (_) => const _SoundSheet(),
    );
  }

  void _showDiagnostics(BuildContext context, WidgetRef ref) {
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: StageColours.surface,
      isScrollControlled: true,
      builder: (_) => const _DiagnosticsSheet(),
    );
  }
}

/// What is happening, in words rather than icons.
class _StatusBanner extends StatelessWidget {
  const _StatusBanner({required this.session});

  final SessionState session;

  @override
  Widget build(BuildContext context) {
    final (colour, headline, detail) = _describe(session);

    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: colour.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: colour.withValues(alpha: 0.35)),
      ),
      child: Row(
        children: [
          Container(
            width: 12,
            height: 12,
            decoration: BoxDecoration(color: colour, shape: BoxShape.circle),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(headline,
                    style: const TextStyle(
                        fontSize: 16,
                        fontWeight: FontWeight.w600,
                        color: StageColours.text)),
                if (detail != null) ...[
                  const SizedBox(height: 2),
                  Text(detail,
                      style: const TextStyle(
                          fontSize: 13, color: StageColours.muted)),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }

  /// The honest reading of the state.
  ///
  /// Static and exhaustive so it can be tested without building a widget —
  /// the words here are the most important thing on the screen.
  static (Color, String, String?) _describe(SessionState session) {
    if (session.phase == SessionPhase.failed) {
      return (
        StageColours.error,
        'Disconnected',
        session.failure?.message ?? 'The connection to the desktop was lost.',
      );
    }
    if (!session.isJoined) {
      return (StageColours.faint, 'Not connected', null);
    }
    if (!session.stats.hasStream) {
      // Joined, but the operator has not started the stream. Silence here is
      // expected, and calling it a fault would be wrong.
      return (
        StageColours.warning,
        'Waiting for the desktop',
        'Connected. The operator has not started the mix yet.',
      );
    }
    if (session.settings.muted) {
      return (StageColours.warning, 'Muted', 'You have muted your own feed.');
    }
    if (session.hasAudibleProblem) {
      return (
        StageColours.warning,
        'Connection is struggling',
        '${session.stats.lossPercent.toStringAsFixed(1)}% of packets are '
            'being lost. Move closer to the access point.',
      );
    }
    return (StageColours.success, 'Receiving', 'Your mix is live.');
  }
}

/// The big one.
class _VolumeControl extends StatelessWidget {
  const _VolumeControl({required this.settings, required this.onChanged});

  final MonitorSettings settings;
  final ValueChanged<double> onChanged;

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Text('Volume', style: Theme.of(context).textTheme.bodySmall),
        const SizedBox(height: 4),
        Text(
          settings.muted
              ? 'muted'
              : '${settings.gainDb >= 0 ? '+' : ''}'
                  '${settings.gainDb.toStringAsFixed(1)} dB',
          style: Theme.of(context).textTheme.displayLarge?.copyWith(
                color:
                    settings.muted ? StageColours.faint : StageColours.text,
              ),
        ),
        const SizedBox(height: 24),
        // Tall and vertical: a thumb already on the screen slides up and down
        // more reliably than it hits a horizontal track.
        SliderTheme(
          data: SliderTheme.of(context).copyWith(
            trackHeight: 48,
            thumbShape:
                const RoundSliderThumbShape(enabledThumbRadius: 26),
            overlayShape: const RoundSliderOverlayShape(overlayRadius: 34),
          ),
          child: Slider(
            value: settings.gainFraction,
            onChanged: onChanged,
          ),
        ),
      ],
    );
  }
}

/// Mute, sized so it cannot be missed.
class _MuteButton extends StatelessWidget {
  const _MuteButton({required this.muted, required this.onPressed});

  final bool muted;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    return FilledButton.icon(
      style: FilledButton.styleFrom(
        minimumSize: const Size.fromHeight(72),
        backgroundColor: muted ? StageColours.warning : StageColours.surfaceRaised,
        foregroundColor: muted ? StageColours.background : StageColours.text,
      ),
      icon: Icon(muted ? Icons.volume_off : Icons.volume_up, size: 28),
      label: Text(muted ? 'Unmute' : 'Mute',
          style: const TextStyle(fontSize: 20, fontWeight: FontWeight.w600)),
      onPressed: onPressed,
    );
  }
}

/// EQ, buffering and the safety ceiling.
class _SoundSheet extends ConsumerWidget {
  const _SoundSheet();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final settings = ref.watch(sessionProvider).settings;
    final controller = ref.read(sessionProvider.notifier);

    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 20, 20, 32),
      child: ListView(
        shrinkWrap: true,
        children: [
          Text('Sound', style: Theme.of(context).textTheme.titleLarge),
          const SizedBox(height: 16),
          _BandSlider(
            label: 'Low',
            valueDb: settings.lowDb,
            onChanged: (value) => controller.setEq(lowDb: value),
          ),
          _BandSlider(
            label: 'Mid',
            valueDb: settings.midDb,
            onChanged: (value) => controller.setEq(midDb: value),
          ),
          _BandSlider(
            label: 'High',
            valueDb: settings.highDb,
            onChanged: (value) => controller.setEq(highDb: value),
          ),
          const Divider(height: 36, color: StageColours.border),
          Text('Buffering', style: Theme.of(context).textTheme.titleLarge),
          const SizedBox(height: 6),
          Text(
            'More buffer survives a worse network, at the cost of feeling '
            'slightly later.',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          const SizedBox(height: 12),
          SegmentedButton<BufferPreset>(
            segments: BufferPreset.values
                .map((preset) => ButtonSegment(
                      value: preset,
                      label: Text('${_presetName(preset)}\n${preset.label}',
                          textAlign: TextAlign.center),
                    ))
                .toList(),
            selected: {settings.preset},
            onSelectionChanged: (selection) =>
                controller.setPreset(selection.first),
          ),
          const Divider(height: 36, color: StageColours.border),
          Text('Hearing safety',
              style: Theme.of(context).textTheme.titleLarge),
          const SizedBox(height: 6),
          Text(
            'This app puts sound directly into your ears for a long time. '
            'The limiter cannot be turned off; this sets how loud it is '
            'allowed to let the output get.',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          const SizedBox(height: 12),
          SegmentedButton<SafetyProfile>(
            segments: const [
              ButtonSegment(
                  value: SafetyProfile.protected,
                  label: Text('Protected\n−6 dB', textAlign: TextAlign.center)),
              ButtonSegment(
                  value: SafetyProfile.standard,
                  label: Text('Standard\n−3 dB', textAlign: TextAlign.center)),
              ButtonSegment(
                  value: SafetyProfile.unrestricted,
                  label: Text('Full\n0 dB', textAlign: TextAlign.center)),
            ],
            selected: {settings.safety},
            onSelectionChanged: (selection) =>
                controller.setSafetyProfile(selection.first),
          ),
        ],
      ),
    );
  }

  static String _presetName(BufferPreset preset) => switch (preset) {
        BufferPreset.tight => 'Tight',
        BufferPreset.normal => 'Normal',
        BufferPreset.safe => 'Safe',
      };
}

class _BandSlider extends StatelessWidget {
  const _BandSlider({
    required this.label,
    required this.valueDb,
    required this.onChanged,
  });

  final String label;
  final double valueDb;
  final ValueChanged<double> onChanged;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        SizedBox(
          width: 46,
          child: Text(label, style: Theme.of(context).textTheme.bodyMedium),
        ),
        Expanded(
          child: Slider(
            value: valueDb.clamp(minBandDb, maxBandDb),
            min: minBandDb,
            max: maxBandDb,
            onChanged: onChanged,
          ),
        ),
        SizedBox(
          width: 62,
          child: Text(
            '${valueDb > 0 ? '+' : ''}${valueDb.toStringAsFixed(0)} dB',
            textAlign: TextAlign.right,
            style: Theme.of(context).textTheme.bodySmall,
          ),
        ),
      ],
    );
  }
}

/// The numbers, for somebody debugging a bad night.
class _DiagnosticsSheet extends ConsumerWidget {
  const _DiagnosticsSheet();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final stats = ref.watch(sessionProvider).stats;

    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 20, 20, 32),
      child: ListView(
        shrinkWrap: true,
        children: [
          Text('Connection detail',
              style: Theme.of(context).textTheme.titleLarge),
          const SizedBox(height: 16),
          _Row('Packets received', '${stats.packetsReceived}'),
          _Row('Packets lost',
              '${stats.packetsLost}  (${stats.lossPercent.toStringAsFixed(2)}%)'),
          _Row('Arrived late or twice',
              '${stats.reordered} late, ${stats.duplicates} repeated'),
          _Row('Buffer', '${stats.bufferMs.toStringAsFixed(0)} ms'),
          _Row('Gaps filled in',
              '${stats.concealedRuns}  (${stats.concealedMs.toStringAsFixed(0)} ms)'),
          _Row('Resynchronised', '${stats.resyncs}'),
          _Row('Clock difference',
              '${stats.driftPpm.toStringAsFixed(0)} ppm'),
          _Row('Heartbeats',
              '${stats.heartbeats} sent, ${stats.missedHeartbeats} missed'),
          _Row(
            'Control round trip',
            stats.controlRttMs == null
                ? 'not measured'
                : '${stats.controlRttMs!.toStringAsFixed(1)} ms',
          ),
          if (stats.rejected > 0)
            _Row('Unrecognised packets', '${stats.rejected}'),
          if (stats.foreignDatagrams > 0)
            _Row('From another device', '${stats.foreignDatagrams}'),
          if (stats.clipped > 0)
            _Row('Clipped samples', '${stats.clipped}  (report this)'),
          const SizedBox(height: 16),
          Text(
            'The control round trip is the time to reach the desktop and back '
            'over the control channel. It is not how far behind the audio is: '
            'that depends on this phone’s audio output and cannot be '
            'measured from here.',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          const SizedBox(height: 16),
          const VersionLabel(),
        ],
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row(this.label, this.value);

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Text(label, style: Theme.of(context).textTheme.bodySmall),
          Text(value, style: Theme.of(context).textTheme.bodyMedium),
        ],
      ),
    );
  }
}

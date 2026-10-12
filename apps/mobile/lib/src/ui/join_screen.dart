// Joining: point the camera at the QR code on the operator's screen.
//
// There is no approval step and no code to type. Scanning *is* the
// authorisation — see `docs/protocol/pairing.md` — so the whole screen is a
// viewfinder and one line of instruction.

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../state/session_controller.dart';
import 'theme.dart';
import 'version_label.dart';

class JoinScreen extends ConsumerStatefulWidget {
  const JoinScreen({super.key});

  @override
  ConsumerState<JoinScreen> createState() => _JoinScreenState();
}

class _JoinScreenState extends ConsumerState<JoinScreen> {
  final _controller = MobileScannerController(
    // One format, so a barcode on a flight case or a Wi-Fi QR taped to a wall
    // does not get picked up while somebody is trying to join.
    formats: const [BarcodeFormat.qrCode],
    detectionSpeed: DetectionSpeed.noDuplicates,
  );

  /// Guards against the detector firing repeatedly while a join is in flight.
  bool _handling = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Future<void> _onDetect(BarcodeCapture capture) async {
    if (_handling) return;
    final payload = capture.barcodes
        .map((barcode) => barcode.rawValue)
        .firstWhere((value) => value != null && value.isNotEmpty,
            orElse: () => null);
    if (payload == null) return;

    // Only a StageLink code stops the camera. Anything else is ignored
    // silently rather than shown as an error, because a performer sweeping a
    // phone across a cluttered stage will catch other codes on the way.
    if (!payload.startsWith('stagelink://join?')) return;

    setState(() => _handling = true);
    await ref.read(sessionProvider.notifier).join(payload);
    if (mounted) setState(() => _handling = false);
  }

  @override
  Widget build(BuildContext context) {
    final session = ref.watch(sessionProvider);

    return Scaffold(
      appBar: AppBar(title: const Text('Join a monitor mix')),
      body: Column(
        children: [
          Expanded(
            child: Stack(
              fit: StackFit.expand,
              children: [
                MobileScanner(controller: _controller, onDetect: _onDetect),
                _ScanFrame(),
                if (session.phase == SessionPhase.joining)
                  const ColoredBox(
                    color: Color(0xCC080B16),
                    child: Center(
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          CircularProgressIndicator(
                              color: StageColours.cyan),
                          SizedBox(height: 20),
                          Text('Joining…',
                              style: TextStyle(
                                  fontSize: 18, color: StageColours.text)),
                        ],
                      ),
                    ),
                  ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              children: [
                Text(
                  'Point the camera at the QR code on the operator screen.',
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.bodyMedium,
                ),
                const SizedBox(height: 8),
                Text(
                  'You will be connected straight away. There is nothing to '
                  'approve and no code to type.',
                  textAlign: TextAlign.center,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
                if (session.failure != null) ...[
                  const SizedBox(height: 20),
                  _FailureNotice(message: session.failure!.message),
                ],
                const SizedBox(height: 16),
                const VersionLabel(),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A viewfinder frame, so it is obvious where to point.
class _ScanFrame extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Center(
      child: Container(
        width: 240,
        height: 240,
        decoration: BoxDecoration(
          border: Border.all(color: StageColours.cyan, width: 3),
          borderRadius: BorderRadius.circular(20),
        ),
      ),
    );
  }
}

class _FailureNotice extends StatelessWidget {
  const _FailureNotice({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: StageColours.error.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: StageColours.error.withValues(alpha: 0.4)),
      ),
      child: Row(
        children: [
          const Icon(Icons.error_outline, color: StageColours.error),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              message,
              style: const TextStyle(color: StageColours.text, fontSize: 14),
            ),
          ),
        ],
      ),
    );
  }
}

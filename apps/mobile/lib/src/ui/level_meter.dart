// A meter for the mix arriving from StageLink Core.
//
// It reads the feed *before* this performer's volume, mute and EQ, which is
// the only reading that answers the question somebody actually has on stage:
// "is the desk still sending me anything?" A meter fed from the output would
// sit at silence whenever they muted themselves and tell them nothing.
//
// Scaled in decibels, not amplitude. A linear bar spends most of its length on
// the top 6 dB and shows nothing at all for a quiet mix, which is exactly the
// case you need to see.

import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'theme.dart';

/// Bottom of the scale. Below this a monitor feed is inaudible anyway.
const double _floorDb = -60;

/// Converts a linear peak to a 0..1 position on the dB scale.
double meterFraction(double peak) {
  if (peak <= 0) return 0;
  final db = 20 * math.log(peak) / math.ln10;
  return ((db - _floorDb) / -_floorDb).clamp(0.0, 1.0);
}

/// `-12.3 dB`, or `--` for digital silence — never a misleading `-inf`.
String meterLabel(double peak) =>
    peak <= 0 ? '--' : '${(20 * math.log(peak) / math.ln10).toStringAsFixed(1)} dB';

class LevelMeter extends StatelessWidget {
  const LevelMeter({super.key, required this.peak, this.label = 'From the desk'});

  /// Linear absolute peak, where 1.0 is full scale.
  final double peak;
  final String label;

  @override
  Widget build(BuildContext context) {
    final fraction = meterFraction(peak);
    // Amber near the top: a monitor feed that is clipping is a hearing risk,
    // and it is the operator's gain to fix, not the performer's.
    final colour = fraction > 0.95 ? StageColours.warning : StageColours.cyan;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            Text(label, style: Theme.of(context).textTheme.bodySmall),
            Text(meterLabel(peak), style: Theme.of(context).textTheme.bodyMedium),
          ],
        ),
        const SizedBox(height: 6),
        ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: LinearProgressIndicator(
            value: fraction,
            minHeight: 10,
            backgroundColor: StageColours.surfaceRaised,
            valueColor: AlwaysStoppedAnimation<Color>(colour),
          ),
        ),
      ],
    );
  }
}

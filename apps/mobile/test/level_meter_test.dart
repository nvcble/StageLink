// The meter's scale, which is the part that can be wrong without looking wrong.
//
// A bar is easy to misread: what matters is that digital silence reads as
// nothing rather than as a floor value, that full scale reads as full, and that
// the scale is decibels — on a linear one a −20 dB mix, which is a perfectly
// normal monitor level, would sit at a tenth of the bar and look like a fault.

import 'package:flutter_test/flutter_test.dart';
import 'package:stagelink_mobile/src/ui/level_meter.dart';

void main() {
  test('digital silence reads as nothing, not as the floor', () {
    expect(meterFraction(0), 0);
    expect(meterLabel(0), '--');
  });

  test('full scale fills the bar', () {
    expect(meterFraction(1), 1);
    expect(meterLabel(1), '0.0 dB');
  });

  test('the scale is decibels, not amplitude', () {
    // −20 dB is a tenth of full scale linearly, but two thirds of a 60 dB bar.
    final fraction = meterFraction(0.1);
    expect(fraction, closeTo(2 / 3, 0.01));
    expect(meterLabel(0.1), '-20.0 dB');
  });

  test('below the floor it bottoms out rather than going negative', () {
    expect(meterFraction(0.0001), 0);
  });

  test('a signal over full scale is clamped rather than overflowing the bar', () {
    expect(meterFraction(2), 1);
  });
}

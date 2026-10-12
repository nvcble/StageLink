// The look.
//
// Dark, high contrast, large targets. Not a style preference: this is read on
// a stage, in low light, often one-handed, sometimes mid-song. A bright
// interface is a light in a performer's face and in the audience's eyes, and a
// small control is one that gets missed when it matters.

import 'package:flutter/material.dart';

/// Brand colours, matching the desktop.
abstract final class StageColours {
  static const background = Color(0xFF080B16);
  static const surface = Color(0xFF10141F);
  static const surfaceRaised = Color(0xFF181D2B);
  static const border = Color(0xFF242A3A);

  static const text = Color(0xFFE8ECF5);
  static const muted = Color(0xFF9AA4BC);
  static const faint = Color(0xFF5C6678);

  static const cyan = Color(0xFF22D3EE);
  static const success = Color(0xFF34D399);
  static const warning = Color(0xFFFBBF24);
  static const error = Color(0xFFF87171);
}

ThemeData stageTheme() {
  const scheme = ColorScheme.dark(
    primary: StageColours.cyan,
    onPrimary: StageColours.background,
    secondary: StageColours.cyan,
    surface: StageColours.surface,
    onSurface: StageColours.text,
    error: StageColours.error,
  );

  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    scaffoldBackgroundColor: StageColours.background,
    fontFamily: 'Roboto',
    appBarTheme: const AppBarTheme(
      backgroundColor: StageColours.background,
      foregroundColor: StageColours.text,
      elevation: 0,
      centerTitle: false,
    ),
    cardTheme: CardThemeData(
      color: StageColours.surface,
      elevation: 0,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(16),
        side: const BorderSide(color: StageColours.border),
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        // 56 high: a thumb on a dark stage, not a mouse pointer.
        minimumSize: const Size.fromHeight(56),
        textStyle: const TextStyle(fontSize: 17, fontWeight: FontWeight.w600),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        minimumSize: const Size.fromHeight(52),
        foregroundColor: StageColours.text,
        side: const BorderSide(color: StageColours.border),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
    ),
    sliderTheme: const SliderThemeData(
      activeTrackColor: StageColours.cyan,
      inactiveTrackColor: StageColours.border,
      thumbColor: StageColours.cyan,
      // Deliberately fat. A performer adjusting their own monitor between
      // verses is not looking at the screen closely.
      trackHeight: 10,
    ),
    textTheme: const TextTheme(
      displayLarge: TextStyle(
        fontSize: 64,
        fontWeight: FontWeight.w300,
        color: StageColours.text,
        // Tabular, so a changing number does not shift the layout under a
        // thumb that is already moving towards it.
        fontFeatures: [FontFeature.tabularFigures()],
      ),
      titleLarge: TextStyle(
        fontSize: 22,
        fontWeight: FontWeight.w600,
        color: StageColours.text,
      ),
      bodyMedium: TextStyle(fontSize: 15, color: StageColours.text),
      bodySmall: TextStyle(fontSize: 13, color: StageColours.muted),
    ),
  );
}

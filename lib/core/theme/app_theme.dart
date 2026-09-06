import 'package:flutter/material.dart';

const Color _seed = Color(0xFF00AAFF);

class AppTheme {
  AppTheme._();

  // ── Dark Theme ─────────────────────────────────────────────
  static ThemeData get darkTheme {
    final cs = ColorScheme.fromSeed(
      seedColor: _seed,
      brightness: Brightness.dark,
    ).copyWith(
      primary: const Color(0xFF00AAFF),
      onPrimary: const Color(0xFF000000),
      primaryContainer: const Color(0xFF003355),
      onPrimaryContainer: const Color(0xFF88DDFF),
      secondary: const Color(0xFFFF8800),
      onSecondary: const Color(0xFF000000),
      surface: const Color(0xFF0F0F0F),
      surfaceContainerLowest: const Color(0xFF0A0A0A),
      surfaceContainerLow: const Color(0xFF141416),
      surfaceContainer: const Color(0xFF1A1A1E),
      surfaceContainerHigh: const Color(0xFF222228),
      surfaceContainerHighest: const Color(0xFF2A2A32),
      onSurface: const Color(0xFFE8E8EC),
      onSurfaceVariant: const Color(0xFF8888A0),
      outline: const Color(0xFF555570),
      outlineVariant: const Color(0xFF2A2A35),
      error: const Color(0xFFFF5252),
      shadow: const Color(0xFF000000),
      scrim: const Color(0xFF000000),
    );

    return _buildTheme(cs, Brightness.dark);
  }

  // ── Light theme ──
  static ThemeData get lightTheme {
    final cs = ColorScheme.fromSeed(
      seedColor: _seed,
      brightness: Brightness.light,
    ).copyWith(
      primary: const Color(0xFF0088DD),
      onPrimary: const Color(0xFFFFFFFF),
      primaryContainer: const Color(0xFFCCE5FF),
      onPrimaryContainer: const Color(0xFF001D33),
      secondary: const Color(0xFFFF7700),
      onSecondary: const Color(0xFFFFFFFF),
      surface: const Color(0xFFF8F9FA),
      surfaceContainerLowest: const Color(0xFFFFFFFF),
      surfaceContainerLow: const Color(0xFFF0F1F3),
      surfaceContainer: const Color(0xFFE8EAED),
      surfaceContainerHigh: const Color(0xFFDEE0E4),
      surfaceContainerHighest: const Color(0xFFD0D3D9),
      onSurface: const Color(0xFF1A1C1E),
      onSurfaceVariant: const Color(0xFF44474F),
      outline: const Color(0xFF74777F),
      outlineVariant: const Color(0xFFC4C6D0),
      error: const Color(0xFFD32F2F),
      shadow: const Color(0xFF000000),
      scrim: const Color(0xFF000000),
    );
    return _buildTheme(cs, Brightness.light);
  }

  static ThemeData _buildTheme(ColorScheme cs, Brightness brightness) {
    final isDark = brightness == Brightness.dark;

    return ThemeData(
      useMaterial3: true,
      fontFamily: 'Plangothic',
      colorScheme: cs,
      scaffoldBackgroundColor: cs.surface,
      appBarTheme: AppBarTheme(
        backgroundColor: cs.surface,
        foregroundColor: cs.onSurface,
        elevation: 0,
        scrolledUnderElevation: 0,
      ),
      dividerTheme: DividerThemeData(
        color: cs.outlineVariant,
        thickness: 1,
        space: 0,
      ),
      iconTheme: const IconThemeData(size: 16),
      sliderTheme: SliderThemeData(
        activeTrackColor: cs.primary,
        inactiveTrackColor: cs.outlineVariant,
        thumbColor: cs.primary,
        overlayColor: cs.primary.withAlpha(20),
        trackHeight: 2,
        thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: cs.surfaceContainerHigh,
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(6),
          borderSide: BorderSide(color: cs.outlineVariant),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(6),
          borderSide: BorderSide(color: cs.outlineVariant),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(6),
          borderSide: BorderSide(color: cs.primary),
        ),
        contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      ),
      tooltipTheme: TooltipThemeData(
        decoration: BoxDecoration(
          color: cs.surfaceContainerHighest,
          borderRadius: BorderRadius.circular(4),
        ),
        textStyle: TextStyle(color: cs.onSurface, fontSize: 10),
        padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
      ),
      snackBarTheme: SnackBarThemeData(
        backgroundColor: cs.surfaceContainerHigh,
        contentTextStyle: TextStyle(color: cs.onSurface, fontSize: 12),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(6)),
        behavior: SnackBarBehavior.floating,
      ),
      dialogTheme: DialogThemeData(
        backgroundColor: cs.surfaceContainer,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
      popupMenuTheme: PopupMenuThemeData(
        color: cs.surfaceContainerHigh,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(6)),
        elevation: 6,
      ),
      dropdownMenuTheme: DropdownMenuThemeData(
        inputDecorationTheme: InputDecorationTheme(
          filled: true,
          fillColor: cs.surfaceContainerHigh,
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(6),
            borderSide: BorderSide(color: cs.outlineVariant),
          ),
        ),
      ),
      checkboxTheme: CheckboxThemeData(
        fillColor: WidgetStateProperty.resolveWith((states) {
          if (states.contains(WidgetState.selected)) return cs.primary;
          return cs.surfaceContainerHigh;
        }),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(3)),
      ),
      switchTheme: SwitchThemeData(
        thumbColor: WidgetStateProperty.resolveWith((states) {
          if (states.contains(WidgetState.selected)) return cs.primary;
          return cs.onSurfaceVariant;
        }),
        trackColor: WidgetStateProperty.resolveWith((states) {
          if (states.contains(WidgetState.selected)) return cs.primary.withAlpha(80);
          return cs.surfaceContainerHighest;
        }),
      ),
    );
  }
}

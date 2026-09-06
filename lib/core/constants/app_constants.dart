import 'package:flutter/material.dart';

class AppConstants {
  AppConstants._();

  static const String appName = '卓声';
  static const String appNameEn = 'ZENITH AUDIO';

  // Compact layout constants
  static const double trackPanelWidth = 280;
  static const double trackTileHeight = 52;
  static const double timelineHeight = 24;
  static const double menuBarHeight = 28;
  static const double toolbarHeight = 34;
  static const double mixerPanelHeight = 160;
  static const double mixerPanelCollapsedHeight = 28;
  static const double browserPanelWidth = 200;

  static const double transportBarHeight = 36;
  static const double mobileBreakpoint = 600;
  static const double tabletBreakpoint = 1024;

  static const String projectExtension = '.zap';
  // v2: adds TrackType.synth (synth-family presets get their own track type).
  static const int projectFormatVersion = 2;
  static const String projectAssetsDir = 'assets/';
  static const String projectInstrumentsDir = 'instruments/';
  static const String projectEffectsDir = 'effects/';

  static const List<String> supportedAudioFormats = [
    '.wav', '.mp3', '.flac', '.aac', '.ogg', '.m4a',
  ];

  static const List<String> supportedMidiFormats = [
    '.mid', '.midi',
  ];
}

class AppColors {
  AppColors._();

  // Neon accent palette
  static const Color accent = Color(0xFF00AAFF);
  static const Color accentDim = Color(0xFF0077BB);
  static const Color neonGreen = Color(0xFF00FF44);
  static const Color neonOrange = Color(0xFFFF8800);
  static const Color neonPink = Color(0xFFFF2288);
  static const Color neonYellow = Color(0xFFFFDD00);

  // State colors
  static const Color solo = Color(0xFFFFD740);
  static const Color mute = Color(0xFFFF5252);
  static const Color record = Color(0xFFFF3333);
  static const Color playhead = Color(0xFFFF5252);
  static const Color ledOn = Color(0xFF00FF44);
  static const Color ledOff = Color(0xFF333333);

  // Waveform colors
  static const Color waveform = Color(0xFF40C4FF);
  static const Color waveformFill = Color(0x3340C4FF);
  static const Color waveformAudio = Color(0xFF4488FF);
  static const Color waveformMidi = Color(0xFF44FF88);

  // Step sequencer
  static const Color stepActive = Color(0xFF00AAFF);
  static const Color stepActiveAlt = Color(0xFFFF8800);
  static const Color stepInactive = Color(0xFF2A2A2A);

  // Mixer
  static const Color mixerBackground = Color(0xFF141416);
  static const Color muteStrip = Color(0xFF1E1E22);
  static const Color masterStrip = Color(0xFF1A1A1E);

  // 16 channel colors
  static const List<Color> trackColors = [
    Color(0xFF40C4FF), // cyan
    Color(0xFF69F0AE), // green
    Color(0xFFFFD740), // yellow
    Color(0xFFFF8A65), // orange
    Color(0xFFCE93D8), // purple
    Color(0xFF4DB6AC), // teal
    Color(0xFFF06292), // pink
    Color(0xFFAED581), // lime
    Color(0xFF4DD0E1), // light cyan
    Color(0xFFFFAB91), // salmon
    Color(0xFFA1887F), // brown
    Color(0xFF90A4AE), // blue grey
    Color(0xFF81D4FA), // light blue
    Color(0xFFA5D6A7), // light green
    Color(0xFFE6EE9C), // pale lime
    Color(0xFFFFCC80), // light orange
  ];
}

import 'dart:typed_data';

import 'soundfont_parser.dart';

/// Singleton holder for the loaded SoundFont bank.
///
/// Load via [loadBytes] (callers read the file — dart:io on desktop,
/// bytes from file_picker on web). Rendering goes through [SynthVoice]
/// in synth_engine.dart using the 'sample' engine.
class SoundFontService {
  SoundFontService._();
  static final SoundFontService instance = SoundFontService._();

  SoundFontBank? _bank;
  String? sourceLabel;

  SoundFontBank? get bank => _bank;
  bool get isLoaded => _bank != null;
  List<SoundFontPreset> get presets => _bank?.presets ?? const [];

  void loadBytes(Uint8List bytes, {String? label}) {
    final bank = SoundFontBank.parse(bytes);
    _bank = bank;
    sourceLabel = label ?? bank.name;
  }

  void unload() {
    _bank = null;
    sourceLabel = null;
  }
}

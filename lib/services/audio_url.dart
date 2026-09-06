import 'dart:typed_data';

import 'audio_url_io.dart'
    if (dart.library.js_util) 'audio_url_web.dart'
    if (dart.library.html) 'audio_url_web.dart';

/// Create a playable URL for raw audio bytes.
/// Desktop: writes a temp file and returns a file:// URL.
/// Web: returns a blob: URL.
Future<String> createAudioUrl(Uint8List bytes, {String mime = 'audio/wav'}) =>
    createAudioUrlImpl(bytes, mime);

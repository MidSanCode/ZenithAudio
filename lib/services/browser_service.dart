import 'dart:io';
import 'package:flutter/foundation.dart' show kIsWeb;

class BrowserEntry {
  final String name;
  final String path;
  final bool isDirectory;

  const BrowserEntry({
    required this.name,
    required this.path,
    this.isDirectory = false,
  });
}

class BrowserService {
  Future<List<BrowserEntry>> scanDirectory(String dirPath) async {
    if (kIsWeb) return [];
    final dir = Directory(dirPath);
    if (!await dir.exists()) return [];
    try {
      final entities = await dir.list().toList();
      final entries = <BrowserEntry>[];
      for (final e in entities) {
        final isDir = await FileSystemEntity.isDirectory(e.path);
        entries.add(BrowserEntry(
          name: e.path.split(Platform.pathSeparator).last,
          path: e.path,
          isDirectory: isDir,
        ));
      }
      entries.sort((a, b) {
        if (a.isDirectory && !b.isDirectory) return -1;
        if (!a.isDirectory && b.isDirectory) return 1;
        return a.name.toLowerCase().compareTo(b.name.toLowerCase());
      });
      return entries;
    } catch (_) {
      return [];
    }
  }

  static const audioExtensions = ['.wav', '.mp3', '.flac', '.ogg', '.aac', '.m4a'];
  static const presetExtensions = ['.lgdf', '.zap', '.mid', '.midi', '.fxp', '.fxb'];

  bool isAudioFile(String path) {
    final ext = path.split('.').last.toLowerCase();
    return audioExtensions.contains('.$ext');
  }

  bool isPresetFile(String path) {
    final ext = path.split('.').last.toLowerCase();
    return presetExtensions.contains('.$ext');
  }
}

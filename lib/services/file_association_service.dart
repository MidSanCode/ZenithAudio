import 'dart:io' show Platform, Process;

import 'package:flutter/foundation.dart' show kIsWeb;

import '../core/utils/logger.dart';

/// Registers the app as the handler for `.zaproj` project files.
///
/// Uses `HKCU\Software\Classes`, so it is a per-user association that needs no
/// administrator rights and never touches another user's settings. Registering
/// is best-effort: a locked-down machine must not stop the app from starting.
class FileAssociationService {
  FileAssociationService._();

  /// The document extension this app owns.
  static const String extension = '.zaproj';

  /// ProgID written to the registry.
  static const String progId = 'ZenithAudio.Project';

  /// Registers `.zaproj` to open with the running executable.
  ///
  /// Windows-only, and only meaningful for a release install — the debug
  /// build lives in `build/`, so we skip it to avoid leaving a broken
  /// association behind after a `flutter clean`.
  static Future<void> ensureRegistered() async {
    if (kIsWeb || !Platform.isWindows) return;
    try {
      final exePath = Platform.resolvedExecutable;
      if (exePath.contains('${Platform.pathSeparator}build${Platform.pathSeparator}')) {
        AppLogger.d('Skipping file association for a build-tree executable');
        return;
      }

      final hive = r'HKCU\Software\Classes';
      final command = '"$exePath" "%1"';

      // ProgID → description + open command.
      _reg(['add', '$hive\\$progId', '/ve', '/d', 'Zenith Audio 工程文件', '/f']);
      _reg(['add', '$hive\\$progId\\DefaultIcon', '/ve', '/d', '"$exePath",0', '/f']);
      _reg(['add', '$hive\\$progId\\shell\\open\\command', '/ve', '/d', command, '/f']);

      // Extension → ProgID. The empty value also marks the extension as ours
      // so Explorer shows the friendly name instead of "ZAPROJ 文件".
      final extKey = '$hive\\$extension';
      _reg(['add', extKey, '/ve', '/d', progId, '/f']);
      _reg(['add', '$extKey\\OpenWithProgids', '/v', progId, '/t', 'REG_NONE', '/d', '', '/f']);

      AppLogger.i('Registered $extension file association for $exePath');
    } catch (e) {
      AppLogger.w('File association registration skipped: $e');
    }
  }

  /// Removes the association (used by an uninstaller).
  static Future<void> unregister() async {
    if (kIsWeb || !Platform.isWindows) return;
    try {
      final hive = r'HKCU\Software\Classes';
      _reg(['delete', '$hive\\$extension', '/f']);
      _reg(['delete', '$hive\\$progId', '/f']);
      AppLogger.i('Removed $extension file association');
    } catch (e) {
      AppLogger.w('Could not remove file association: $e');
    }
  }

  /// Runs one `reg.exe` invocation, ignoring the result.
  static void _reg(List<String> args) {
    try {
      final result = Process.runSync('reg', args, runInShell: true);
      if (result.exitCode != 0) {
        AppLogger.w('reg ${args.first} ${args[1]} exited ${result.exitCode}');
      }
    } catch (e) {
      AppLogger.w('reg.exe unavailable: $e');
    }
  }
}

import 'dart:io' show File, Platform;

import 'package:flutter/foundation.dart' show kIsWeb;

import '../core/utils/logger.dart';

/// Reads the file the OS asked us to open (file association / "Open with").
///
/// On Windows and Linux the Flutter runner forwards the process arguments, so
/// double-clicking a `.zaproj` in Explorer launches us with its path as
/// `argv[1]`. macOS delivers the same information through the app delegate
/// instead, which this build does not wire up yet.
class LauncherArgs {
  LauncherArgs._();

  /// First argument that looks like a project archive path, if any.
  ///
  /// Returns null when the app was launched normally (no file to open).
  static String? initialProjectPath(List<String> args) {
    if (kIsWeb || args.isEmpty) return null;

    for (final arg in args) {
      if (arg.isEmpty) continue;
      // Flutter's own switches (`--dart-...`) are never file paths.
      if (arg.startsWith('-')) continue;
      if (!_isProjectArchive(arg)) continue;
      final file = File(arg);
      if (!file.existsSync()) {
        AppLogger.w('Launch argument is not an existing file: $arg');
        continue;
      }
      AppLogger.i('Launched to open: $arg');
      return arg;
    }
    return null;
  }

  /// True when [path] carries a known project archive extension.
  static bool _isProjectArchive(String path) {
    final lower = path.toLowerCase();
    return lower.endsWith('.zaproj') ||
        lower.endsWith('.lgdf') ||
        lower.endsWith('.zap');
  }

  /// Whether this platform delivers launch arguments to the Dart entrypoint.
  static bool get usesEntrypointArguments {
    if (kIsWeb) return false;
    try {
      return Platform.isWindows || Platform.isLinux;
    } catch (_) {
      return false;
    }
  }
}

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:path_provider/path_provider.dart';

import '../core/utils/logger.dart';

/// Keeps a single running instance and hands opened files to it.
///
/// A file association on Windows starts a *new* process every time, so
/// double-clicking a second project would otherwise open a second window.
/// This uses a lock file plus a small hand-off file — no sockets, no extra
/// dependencies.
///
/// If anything here fails the app still starts normally: a duplicate window is
/// a far smaller problem than refusing to launch.
class SingleInstance {
  SingleInstance._();

  static File? _lockFile;
  static File? _handoffFile;
  static Timer? _poller;

  /// True when another instance already owns the lock and has been asked to
  /// open [projectPath] instead. The caller should exit immediately.
  static Future<bool> handOffToExisting(String? projectPath) async {
    if (kIsWeb) return false;
    try {
      final dir = await _stateDir();
      final lock = File('${dir.path}/instance.lock');

      if (await lock.exists()) {
        // A stale lock (crashed process) must not block startup forever.
        final pid = int.tryParse((await lock.readAsString()).trim());
        if (pid != null && _isAlive(pid)) {
          if (projectPath != null) {
            await _writeHandoff(dir, projectPath);
            AppLogger.i('Handed $projectPath to the running instance');
          } else {
            AppLogger.i('Another instance is already running');
          }
          return true;
        }
        AppLogger.w('Removing stale instance lock (pid $pid)');
      }

      await lock.writeAsString('$pid');
      _lockFile = lock;
      _handoffFile = File('${dir.path}/handoff.txt');
      return false;
    } catch (e) {
      AppLogger.w('Single-instance check skipped: $e');
      return false;
    }
  }

  /// Watches for files handed over by later launches.
  ///
  /// [onProject] runs on the platform thread, so it must hop to the UI as
  /// needed — the callers here are Riverpod-driven and already do.
  static void startWatching(void Function(String path) onProject) {
    if (kIsWeb || _lockFile == null) return;
    _poller?.cancel();
    _poller = Timer.periodic(const Duration(milliseconds: 700), (_) async {
      final handoff = _handoffFile;
      if (handoff == null || !await handoff.exists()) return;
      try {
        final path = (await handoff.readAsString()).trim();
        await handoff.delete();
        if (path.isNotEmpty) onProject(path);
      } catch (e) {
        AppLogger.w('Handoff read failed: $e');
      }
    });
  }

  /// Releases the lock and any timers. Call on normal shutdown.
  static Future<void> release() async {
    _poller?.cancel();
    _poller = null;
    try {
      final lock = _lockFile;
      if (lock != null && await lock.exists()) await lock.delete();
    } catch (_) {}
    _lockFile = null;
    _handoffFile = null;
  }

  static Future<Directory> _stateDir() async {
    final support = await getApplicationSupportDirectory();
    final dir = Directory('${support.path}/instance');
    if (!await dir.exists()) await dir.create(recursive: true);
    return dir;
  }

  static Future<void> _writeHandoff(Directory dir, String path) async {
    // A short retry loop covers the moment the other instance is reading.
    for (var i = 0; i < 3; i++) {
      try {
        await File('${dir.path}/handoff.txt').writeAsString(path);
        return;
      } catch (_) {
        await Future<void>.delayed(const Duration(milliseconds: 120));
      }
    }
  }

  /// Whether a process with [pid] is currently running.
  static bool _isAlive(int pid) {
    try {
      if (Platform.isWindows) {
        final out = Process.runSync('tasklist', ['/FI', 'PID eq $pid', '/NH']);
        return out.stdout.toString().contains('$pid');
      }
      final out = Process.runSync('ps', ['-p', '$pid']);
      return out.exitCode == 0;
    } catch (_) {
      // Cannot tell — assume alive so we never double-open the same project.
      return true;
    }
  }
}

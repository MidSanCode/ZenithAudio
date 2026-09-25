import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// Verifies the exact command string written to the registry keeps its quotes,
/// because an unquoted `%1` breaks on paths containing spaces.
void main() {
  test('registry open command keeps quotes around exe and %1', () {
    const exePath = r'C:\Program Files\Zenith Audio\zenith_audio.exe';
    final command = '"$exePath" "%1"';

    expect(command, r'"C:\Program Files\Zenith Audio\zenith_audio.exe" "%1"');

    // Write it through reg.exe exactly as the service does, then read it back.
    const key = 'HKCU\\Software\\Classes\\ZenithAudio.QuoteTest\\shell\\open\\command';
    final add = Process.runSync('reg', ['add', key, '/ve', '/d', command, '/f'],
        runInShell: true);
    expect(add.exitCode, 0, reason: 'reg add failed: ${add.stderr}');

    try {
      final read = Process.runSync(
        'reg',
        ['query', key, '/ve'],
        runInShell: true,
      );
      final out = read.stdout.toString();
      // The stored value must still contain both quote pairs.
      expect(out, contains('"%1"'),
          reason: 'quotes around %1 were lost:\n$out');
      expect(out, contains('Program Files'),
          reason: 'quoted exe path was mangled:\n$out');
    } finally {
      Process.runSync('reg',
          ['delete', 'HKCU\\Software\\Classes\\ZenithAudio.QuoteTest', '/f'],
          runInShell: true);
    }
  });
}

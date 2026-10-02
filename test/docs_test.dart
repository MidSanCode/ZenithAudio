import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

/// S9: the documentation deliverables exist and carry their key sections.
///
/// Docs are easy to let rot; this pins the four S9 documents at their expected
/// paths and checks a load-bearing heading in each, so deleting one or gutting
/// it fails CI rather than going unnoticed.
void main() {
  final docs = <String, List<String>>{
    'docs/USER_MANUAL.md': ['用户手册', '快捷键'],
    'docs/SHORTCUTS.md': ['快捷键', 'Ctrl+S'],
    'docs/ARCHITECTURE.md': ['架构', 'Rust'],
    'docs/RUST_CORE.md': ['Rust 核心', '实时安全'],
  };

  docs.forEach((path, expected) {
    test('$path exists and has its key sections', () async {
      final file = File(path);
      expect(file.existsSync(), isTrue, reason: '$path must exist');
      final text = await file.readAsString();
      for (final needle in expected) {
        expect(text.contains(needle), isTrue,
            reason: '$path should mention "$needle"');
      }
    });
  });
}

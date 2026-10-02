import 'dart:io';

import 'package:flutter/foundation.dart' show TargetPlatform;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/core/shortcuts.dart';

/// S9: the canonical shortcut registry, and its guard against `SHORTCUTS.md`
/// drifting from the code.
void main() {
  group('rendering', () {
    test('the primary modifier renders as Ctrl on non-mac and ⌘ on mac', () {
      final save = shortcutById('saveProject')!;
      expect(save.labelOn(TargetPlatform.linux), 'Ctrl+S');
      expect(save.labelOn(TargetPlatform.macOS), '⌘S');
    });

    test('shift and alt render in the documented order', () {
      final redo = shortcutById('redo')!;
      expect(redo.labelOn(TargetPlatform.linux), 'Ctrl+Shift+Z');
      final exit = shortcutById('exit')!;
      expect(exit.labelOn(TargetPlatform.linux), 'Alt+F4');
      expect(exit.labelOn(TargetPlatform.macOS), '⌥F4');
    });

    test('named keys render as words', () {
      expect(shortcutById('playPause')!.labelOn(TargetPlatform.linux), 'Space');
      expect(shortcutById('abToggle')!.labelOn(TargetPlatform.linux), 'Shift+Tab');
    });
  });

  group('invariants', () {
    test('ids are unique', () {
      final ids = kShortcuts.map((s) => s.id).toList();
      expect(ids.toSet().length, ids.length);
    });

    test('no two shortcuts share an activator', () {
      // Two actions on one binding is a bug that only shows up as "the wrong
      // thing happened"; catch it here instead.
      final seen = <String>{};
      for (final s in kShortcuts) {
        final repr = '${s.key.debugName}|${s.primary}|${s.shift}|${s.alt}';
        expect(seen.add(repr), isTrue, reason: 'duplicate activator for ${s.id}');
      }
    });

    test('undo and redo use the same key but a different shift state', () {
      final undo = shortcutById('undo')!;
      final redo = shortcutById('redo')!;
      expect(undo.key, redo.key);
      expect(undo.shift, isFalse);
      expect(redo.shift, isTrue);
    });

    test('activators do not repeat', () {
      // A held undo key firing repeatedly loses work; every activator must
      // exclude key repeats.
      for (final s in kShortcuts) {
        expect(s.activator.includeRepeats, isFalse, reason: s.id);
      }
    });
  });

  group('documentation guard', () {
    test('SHORTCUTS.md documents every registry shortcut', () async {
      // The doc is the user-facing contract. Read it from disk so adding a
      // shortcut without a table row fails here rather than shipping an
      // undocumented binding. The test runner's CWD is the package root.
      final file = File('docs/SHORTCUTS.md');
      expect(file.existsSync(), isTrue,
          reason: 'docs/SHORTCUTS.md must exist at the package root');
      final doc = await file.readAsString();

      for (final shortcut in kShortcuts) {
        // The doc renders non-mac labels (`Ctrl+...`), so compare against that
        // form; the mac glyphs are covered by the rendering tests above.
        final label = shortcut.labelOn(TargetPlatform.linux);
        expect(
          doc.contains(label),
          isTrue,
          reason: 'docs/SHORTCUTS.md is missing a row for '
              '${shortcut.id} ($label)',
        );
      }
    });
  });
}

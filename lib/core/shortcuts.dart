/// The canonical keyboard-shortcut registry.
///
/// ## Why a single source
///
/// Shortcuts used to exist only as **translation strings** (`shortcut.undo` =
/// `"Ctrl+Z"`): a display label with nothing binding it, and two entries could
/// disagree with what the code actually handled. This registry is the one place
/// a shortcut is defined; the UI reads its label from here, and
/// `docs/SHORTCUTS.md` is kept in step by a test.
///
/// ## Platform display
///
/// The activator is logical (the primary modifier), but the *label* a user sees
/// must be `⌘` on macOS and `Ctrl` elsewhere. [Shortcut.label] renders for the
/// current platform; tests pin both renderings via [Shortcut.labelOn].
library;

import 'package:flutter/foundation.dart' show defaultTargetPlatform, TargetPlatform;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show SingleActivator;

/// A single application shortcut.
class Shortcut {
  /// Stable id, matching the `shortcut.*` translation key suffix.
  final String id;

  /// The key, as a logical key.
  final LogicalKeyboardKey key;

  /// Whether the primary platform modifier (Ctrl / ⌘) is required.
  final bool primary;

  /// Whether Shift is required.
  final bool shift;

  /// Whether Alt (⌥ on macOS) is required.
  final bool alt;

  /// The scope the shortcut is active in.
  final ShortcutScope scope;

  const Shortcut({
    required this.id,
    required this.key,
    this.primary = false,
    this.shift = false,
    this.alt = false,
    this.scope = ShortcutScope.global,
  });

  /// A Flutter activator for this shortcut.
  ///
  /// `includeRepeats` is false: a held key must not fire the action repeatedly
  /// (undo firing on key-repeat is a classic way to lose work).
  SingleActivator get activator => SingleActivator(
        key,
        control: primary && !_isMac,
        meta: primary && _isMac,
        shift: shift,
        alt: alt,
        includeRepeats: false,
      );

  /// The human-readable label for the current platform.
  String get label => labelOn(defaultTargetPlatform);

  /// The label as it would render on [platform].
  ///
  /// Public (and taking a platform) so a test can assert both the macOS and
  /// non-macOS forms without a widget.
  String labelOn(TargetPlatform platform) {
    final isMac = platform == TargetPlatform.macOS || platform == TargetPlatform.iOS;
    final parts = <String>[];
    if (primary) parts.add(isMac ? '⌘' : 'Ctrl');
    if (alt) parts.add(isMac ? '⌥' : 'Alt');
    if (shift) parts.add('Shift');
    parts.add(_keyName(key));
    return parts.join(isMac ? '' : '+');
  }

  static bool get _isMac =>
      defaultTargetPlatform == TargetPlatform.macOS ||
      defaultTargetPlatform == TargetPlatform.iOS;

  static String _keyName(LogicalKeyboardKey key) {
    if (key == LogicalKeyboardKey.space) return 'Space';
    if (key == LogicalKeyboardKey.tab) return 'Tab';
    if (key == LogicalKeyboardKey.escape) return 'Esc';
    if (key == LogicalKeyboardKey.enter) return 'Enter';
    if (key == LogicalKeyboardKey.f4) return 'F4';
    // `keyLabel` gives a stable uppercase name for letters/digits.
    return key.keyLabel.isEmpty ? key.debugName ?? '?' : key.keyLabel;
  }
}

/// Where a shortcut applies.
enum ShortcutScope {
  /// Available everywhere in the app.
  global,

  /// Only in the editor / piano roll.
  editor,
}

/// Every application shortcut, in documented order.
///
/// The order here is the order in `docs/SHORTCUTS.md`; the test compares the
/// two lists so neither can drift.
const List<Shortcut> kShortcuts = [
  Shortcut(id: 'newProject', key: LogicalKeyboardKey.keyN, primary: true),
  Shortcut(id: 'openProject', key: LogicalKeyboardKey.keyO, primary: true),
  Shortcut(id: 'saveProject', key: LogicalKeyboardKey.keyS, primary: true),
  Shortcut(id: 'exportProject', key: LogicalKeyboardKey.keyE, primary: true),
  Shortcut(
    id: 'exit',
    key: LogicalKeyboardKey.f4,
    alt: true,
    scope: ShortcutScope.global,
  ),
  Shortcut(
    id: 'undo',
    key: LogicalKeyboardKey.keyZ,
    primary: true,
    scope: ShortcutScope.editor,
  ),
  Shortcut(
    id: 'redo',
    key: LogicalKeyboardKey.keyZ,
    primary: true,
    shift: true,
    scope: ShortcutScope.editor,
  ),
  Shortcut(
    id: 'addTrack',
    key: LogicalKeyboardKey.keyT,
    primary: true,
    scope: ShortcutScope.editor,
  ),
  Shortcut(
    id: 'importAudio',
    key: LogicalKeyboardKey.keyI,
    primary: true,
    scope: ShortcutScope.editor,
  ),
  Shortcut(
    id: 'importMidi',
    key: LogicalKeyboardKey.keyI,
    primary: true,
    shift: true,
    scope: ShortcutScope.editor,
  ),
  Shortcut(
    id: 'playPause',
    key: LogicalKeyboardKey.space,
    scope: ShortcutScope.editor,
  ),
  Shortcut(
    id: 'abToggle',
    key: LogicalKeyboardKey.tab,
    shift: true,
    scope: ShortcutScope.editor,
  ),
];

/// Looks a shortcut up by id, or `null`.
Shortcut? shortcutById(String id) {
  for (final shortcut in kShortcuts) {
    if (shortcut.id == id) return shortcut;
  }
  return null;
}

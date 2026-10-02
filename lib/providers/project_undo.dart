part of 'project_provider.dart';

/// Undo / redo history, split out of the main provider in S0 (the provider
/// exceeded the 800-line rule).
///
/// A mixin rather than an extension: it needs access to the protected
/// [Notifier.state] setter and to the private history stacks.
mixin _ProjectHistoryMixin on Notifier<Project> {
  static const int _maxUndo = 50;

  final List<Project> _undoStack = [];
  final List<Project> _redoStack = [];

  /// Whether there are unsaved changes.
  ///
  /// Declared here rather than in the class body so sibling mixins bound with
  /// `on _ProjectHistoryMixin` can read and set it; Dart privacy is per-library
  /// but a mixin's `this` is its `on` type, so a class-body private would not
  /// be in scope for them.
  bool _isDirty = false;

  bool get canUndo => _undoStack.isNotEmpty;
  bool get canRedo => _redoStack.isNotEmpty;

  /// Marks the project dirty.
  void _markDirty() => _isDirty = true;

  /// A snapshot clone. Notes and step patterns are copied so later in-place
  /// mutation cannot corrupt a stored snapshot.
  Project _cloneForHistory(Project p) {
    return Project(
      id: p.id,
      name: p.name,
      tracks: p.tracks
          .map((t) => t.copyWith(
                notes: t.notes.map((n) => n.copyWith()).toList(),
                stepPattern: t.stepPattern.isEmpty
                    ? t.stepPattern
                    : List<bool>.from(t.stepPattern),
              ))
          .toList(),
      sampleRate: p.sampleRate,
      timeSignatureNumerator: p.timeSignatureNumerator,
      timeSignatureDenominator: p.timeSignatureDenominator,
      keySignature: p.keySignature,
      bpm: p.bpm,
      playbackSpeed: p.playbackSpeed,
      patterns: p.patterns,
      playlist: p.playlist,
    );
  }

  /// Records the current state on the undo stack and clears redo.
  void _pushUndo() {
    _undoStack.add(_cloneForHistory(state));
    if (_undoStack.length > _maxUndo) _undoStack.removeAt(0);
    _redoStack.clear();
  }

  void undo() {
    if (_undoStack.isEmpty) return;
    _redoStack.add(_cloneForHistory(state));
    state = _undoStack.removeLast();
    AppLogger.i('Undo');
  }

  void redo() {
    if (_redoStack.isEmpty) return;
    _undoStack.add(_cloneForHistory(state));
    state = _redoStack.removeLast();
    AppLogger.i('Redo');
  }

  void _clearHistory() {
    _undoStack.clear();
    _redoStack.clear();
  }
}

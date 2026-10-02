// Piano-roll display helpers (PLAN §3.S6b): scale highlighting, row shading
// and ghost notes.
//
// Why this is separate from the widget: these are all "what should be shown for
// this pitch/tick" questions with a single correct answer, and getting one
// wrong is visible to every user (a highlighted row that is not in the key, a
// ghost that points at the wrong pattern). They are pure functions of the key
// and the note lists, so they are unit-tested here and the painter only
// consumes the answers.

import '../../models/note.dart';
import '../../models/pattern.dart';


/// The pitch-class set of a musical key, for row highlighting.
class ScaleHighlight {
  /// Root pitch class, 0 = C.
  final int rootPitchClass;

  /// Intervals from the root, in semitones (a 7-note scale has 7 entries).
  final List<int> intervals;

  const ScaleHighlight({
    this.rootPitchClass = 0,
    this.intervals = majorIntervals,
  });

  /// The major scale.
  static const List<int> majorIntervals = [0, 2, 4, 5, 7, 9, 11];

  /// The natural minor scale.
  static const List<int> minorIntervals = [0, 2, 3, 5, 7, 8, 10];

  /// A highlight with nothing emphasized (all rows equal).
  static const ScaleHighlight none = ScaleHighlight(intervals: []);

  /// Whether [pitch] belongs to the scale.
  bool contains(int pitch) {
    if (intervals.isEmpty) return true;
    final pc = ((pitch % 12) + 12) % 12;
    final rel = ((pc - rootPitchClass) % 12 + 12) % 12;
    // A scale is a pitch-class set; compare against the interval set modulo 12
    // so the octave the pitch sits in does not matter.
    return intervals.any((i) => i % 12 == rel);
  }

  /// Whether the pitch is the root (shown stronger than other scale tones).
  bool isRoot(int pitch) {
    final pc = ((pitch % 12) + 12) % 12;
    return pc == rootPitchClass;
  }

  /// The MIDI pitch of the nearest scale tone at or above [pitch].
  ///
  /// Used by "snap to scale" and by a paint tool that constrains to the key.
  int snapUp(int pitch) {
    if (contains(pitch)) return pitch;
    for (var p = pitch + 1; p <= 127; p++) {
      if (contains(p)) return p;
    }
    return pitch;
  }

  /// The MIDI pitch of the nearest scale tone at or below [pitch].
  int snapDown(int pitch) {
    if (contains(pitch)) return pitch;
    for (var p = pitch - 1; p >= 0; p--) {
      if (contains(p)) return p;
    }
    return pitch;
  }
}

/// One ghost note: a note from another pattern shown faintly for reference.
class GhostNote {
  /// Source pitch.
  final int pitch;

  /// Start in ticks.
  final int startTicks;

  /// Length in ticks.
  final int lengthTicks;

  const GhostNote({
    required this.pitch,
    required this.startTicks,
    required this.lengthTicks,
  });

  /// End in ticks.
  int get endTicks => startTicks + lengthTicks;
}

/// Builds ghost notes for [pitchRange] from a reference pattern.
///
/// The reference is usually the pattern on another lane, or the previous
/// pattern in the arrangement. Only notes whose pitch falls in the visible
/// range are returned, so a painter never draws off-screen.
List<GhostNote> ghostNotes(
  Pattern reference, {
  required int minPitch,
  required int maxPitch,
}) {
  final out = <GhostNote>[];
  for (final note in reference.notes) {
    if (note.pitch < minPitch || note.pitch > maxPitch) continue;
    out.add(GhostNote(
      pitch: note.pitch,
      startTicks: note.startTicks,
      lengthTicks: note.lengthTicks,
    ));
  }
  out.sort((a, b) => a.startTicks == b.startTicks
      ? a.pitch.compareTo(b.pitch)
      : a.startTicks.compareTo(b.startTicks));
  return out;
}

/// A note's horizontal span as a fraction of a grid step, for velocity lanes
/// and minimap drawing.
///
/// Kept here so the widget and any future export agree on the mapping.
({double start, double length}) noteSpanFraction(
  Note note, {
  required int totalTicks,
}) {
  if (totalTicks <= 0) return (start: 0, length: 0);
  final start = (note.startTicks / totalTicks).clamp(0.0, 1.0);
  final end = (note.endTicks / totalTicks).clamp(0.0, 1.0);
  return (start: start, length: (end - start).clamp(0.0, 1.0));
}

/// Groups notes by pitch, for a piano-roll row index.
///
/// Returns a map from pitch to the notes on that row, sorted by start, so a
/// painter can draw one row without scanning the whole list.
Map<int, List<Note>> notesByPitch(Iterable<Note> notes) {
  final byPitch = <int, List<Note>>{};
  for (final note in notes) {
    (byPitch[note.pitch] ??= []).add(note);
  }
  for (final row in byPitch.values) {
    row.sort((a, b) => a.startTicks.compareTo(b.startTicks));
  }
  return byPitch;
}

/// The key shown for a project's key-signature string, when it is recognised.
///
/// A free function rather than a `ChordService` member so the piano roll does
/// not import the whole chord generator for one lookup.
ScaleHighlight scaleForKey(String keySignature, {bool minor = false}) {
  // Keys are written like "C", "F#", "Bb". A minor key is signalled by the
  // caller rather than by the string, because "C" alone is ambiguous.
  final root = _pitchClassOf(keySignature);
  return ScaleHighlight(
    rootPitchClass: root,
    intervals: minor ? ScaleHighlight.minorIntervals : ScaleHighlight.majorIntervals,
  );
}

int _pitchClassOf(String name) {
  const names = {
    'C': 0,
    'C#': 1,
    'Db': 1,
    'D': 2,
    'D#': 3,
    'Eb': 3,
    'E': 4,
    'F': 5,
    'F#': 6,
    'Gb': 6,
    'G': 7,
    'G#': 8,
    'Ab': 8,
    'A': 9,
    'A#': 10,
    'Bb': 10,
    'B': 11,
  };
  return names[name] ?? 0;
}

import '../models/note.dart';

/// Chord / progression generation service (FL-style chord tool).
class ChordService {
  ChordService._();

  /// Chord type → semitone offsets from the chord root.
  static const Map<String, List<int>> chordTypes = {
    'maj': [0, 4, 7],
    'min': [0, 3, 7],
    'dim': [0, 3, 6],
    'aug': [0, 4, 8],
    'sus2': [0, 2, 7],
    'sus4': [0, 5, 7],
    'maj6': [0, 4, 7, 9],
    'min6': [0, 3, 7, 9],
    'add9': [0, 4, 7, 14],
    '7': [0, 4, 7, 10],
    'maj7': [0, 4, 7, 11],
    'min7': [0, 3, 7, 10],
    'm7b5': [0, 3, 6, 10],
    'dom9': [0, 4, 7, 10, 14],
    'min9': [0, 3, 7, 10, 14],
  };

  static const List<String> progressionIds = [
    'pop', 'sensitive', '50s', 'canon', 'jazz', 'sad_lofi', 'minor_pop', 'blues',
  ];

  /// Progression library: scale degree (1-based) + chord type.
  static const Map<String, List<(int, String)>> progressions = {
    'pop': [(1, 'maj'), (5, 'maj'), (6, 'min'), (4, 'maj')],
    'sensitive': [(6, 'min'), (4, 'maj'), (1, 'maj'), (5, 'maj')],
    '50s': [(1, 'maj'), (6, 'min'), (4, 'maj'), (5, 'maj')],
    'canon': [
      (1, 'maj'), (5, 'maj'), (6, 'min'), (3, 'min'),
      (4, 'maj'), (1, 'maj'), (4, 'maj'), (5, 'maj'),
    ],
    'jazz': [(2, 'min7'), (5, '7'), (1, 'maj7'), (6, '7')],
    'sad_lofi': [(1, 'min7'), (4, 'min7'), (6, 'min7'), (5, '7')],
    'minor_pop': [(1, 'min'), (6, 'maj'), (3, 'maj'), (7, 'maj')],
    'blues': [
      (1, '7'), (1, '7'), (1, '7'), (1, '7'),
      (4, '7'), (4, '7'), (1, '7'), (1, '7'),
      (5, '7'), (4, '7'), (1, '7'), (5, '7'),
    ],
  };

  static const List<String> patternIds = ['block', 'arpUp', 'arpDown', 'alberti', 'strum', 'bassChord'];

  static const List<int> majorScale = [0, 2, 4, 5, 7, 9, 11];
  static const List<int> minorScale = [0, 2, 3, 5, 7, 8, 10];

  /// Semitone offset of a 1-based scale [degree] (can exceed 7 → octaves).
  static int degreeToSemitone(int degree, {String mode = 'major'}) {
    final scale = mode == 'minor' ? minorScale : majorScale;
    final i = degree - 1;
    final octave = i ~/ scale.length;
    return scale[i % scale.length] + 12 * octave;
  }

  static String romanName(int degree, String type, {String mode = 'major'}) {
    const romansMajor = ['I', 'II', 'III', 'IV', 'V', 'VI', 'VII'];
    const romansMinor = ['i', 'ii', 'iii', 'iv', 'v', 'vi', 'vii'];
    final idx = (degree - 1) % 7;
    var r = mode == 'minor' ? romansMinor[idx] : romansMajor[idx];
    if (type.contains('maj7')) r += 'maj7';
    else if (type == '7' || type == 'dom9') r += '7';
    else if (type.contains('min')) r = r.toLowerCase();
    else if (type == 'dim') r += '°';
    else if (type == 'aug') r += '+';
    return r;
  }

  /// Generate the notes of one chord.
  static List<Note> chordNotes({
    required int rootPc, // 0..11 pitch class of the chord root
    required String type,
    required int octave, // MIDI octave: C4 = 60 → octave 4
    required double startTime,
    required double duration,
    int velocity = 100,
    int inversion = 0,
    List<int>? shapeOverride,
  }) {
    final intervals = shapeOverride ?? chordTypes[type] ?? chordTypes['maj']!;
    final rootMidi = 12 * (octave + 1) + rootPc;
    final pitches = <int>[];
    for (final semi in intervals) {
      pitches.add(rootMidi + semi);
    }
    // Inversions: move bottom notes up an octave.
    for (int v = 0; v < inversion; v++) {
      if (pitches.isEmpty) break;
      final lowest = pitches.reduce((a, b) => a < b ? a : b);
      final idx = pitches.indexOf(lowest);
      pitches[idx] = lowest + 12;
    }
    return pitches
        .map((p) => Note(
              pitch: p.clamp(12, 108),
              startTime: startTime,
              duration: duration,
              velocity: velocity,
            ))
        .toList();
  }

  /// Generate a full chord progression as timeline notes.
  ///
  /// [keyRootPc] 0..11 (C=0), [mode] 'major'|'minor',
  /// [progressionId] one of [progressionIds],
  /// [startSec] where to insert, [secPerBeat] from project BPM,
  /// [beatsPerChord] chord length in beats, [repeat] how many times the
  /// progression repeats, [patternId] one of [patternIds].
  static List<Note> generateProgression({
    required int keyRootPc,
    required String mode,
    required String progressionId,
    required double startSec,
    required double secPerBeat,
    double beatsPerChord = 4,
    int repeat = 1,
    String patternId = 'block',
    int octave = 4,
    String? chordTypeOverride,
    int velocity = 96,
  }) {
    final steps = progressions[progressionId] ?? progressions['pop']!;
    final out = <Note>[];
    final chordDur = beatsPerChord * secPerBeat;

    for (int rep = 0; rep < repeat; rep++) {
      for (int s = 0; s < steps.length; s++) {
        final (degree, type) = steps[s];
        final chordType = chordTypeOverride ?? type;
        final rootSemi = degreeToSemitone(degree, mode: mode);
        final rootPc = (keyRootPc + rootSemi) % 12;
        final start = startSec + (rep * steps.length + s) * chordDur;
        out.addAll(_patternNotes(
          patternId: patternId,
          rootPc: rootPc,
          type: chordType,
          octave: octave,
          startTime: start,
          duration: chordDur,
          velocity: velocity,
        ));
      }
    }
    return out;
  }

  /// Generate a single chord at a position (e.g. from a selected root note).
  static List<Note> chordAt({
    required int rootMidi,
    required String type,
    required double startTime,
    required double duration,
    int velocity = 100,
    String patternId = 'block',
  }) {
    return _patternNotes(
      patternId: patternId,
      rootPc: rootMidi % 12,
      type: type,
      octave: (rootMidi ~/ 12) - 1,
      startTime: startTime,
      duration: duration,
      velocity: velocity,
    );
  }

  static List<Note> _patternNotes({
    required String patternId,
    required int rootPc,
    required String type,
    required int octave,
    required double startTime,
    required double duration,
    required int velocity,
  }) {
    final intervals = chordTypes[type] ?? chordTypes['maj']!;
    final rootMidi = 12 * (octave + 1) + rootPc;
    final pitches = intervals.map((s) => rootMidi + s).toList();
    final n = pitches.length;

    switch (patternId) {
      case 'arpUp':
      case 'arpDown':
        final seq = patternId == 'arpUp'
            ? pitches
            : pitches.reversed.toList();
        final stepDur = duration / seq.length;
        return [
          for (int i = 0; i < seq.length; i++)
            Note(
              pitch: seq[i].clamp(12, 108),
              startTime: startTime + i * stepDur,
              duration: stepDur * 0.95,
              velocity: velocity,
            ),
        ];
      case 'alberti':
        // low-high-mid-high classic pattern
        final sorted = [...pitches]..sort();
        final pattern = sorted.length >= 3
            ? [sorted.first, sorted.last, sorted[1], sorted.last]
            : sorted;
        final stepDur = duration / pattern.length;
        return [
          for (int i = 0; i < pattern.length; i++)
            Note(
              pitch: pattern[i].clamp(12, 108),
              startTime: startTime + i * stepDur,
              duration: stepDur * 0.9,
              velocity: velocity,
            ),
        ];
      case 'strum':
        const strumGap = 0.03;
        return [
          for (int i = 0; i < pitches.length; i++)
            Note(
              pitch: pitches[i].clamp(12, 108),
              startTime: startTime + i * strumGap,
              duration: duration - i * strumGap,
              velocity: velocity,
            ),
        ];
      case 'bassChord':
        final notes = <Note>[
          Note(
            pitch: rootMidi.clamp(12, 108),
            startTime: startTime,
            duration: duration,
            velocity: velocity + 10 > 127 ? 127 : velocity + 10,
          ),
        ];
        for (int i = 1; i < n; i++) {
          notes.add(Note(
            pitch: (rootMidi + intervals[i]).clamp(12, 108),
            startTime: startTime,
            duration: duration,
            velocity: velocity - 16 < 40 ? 40 : velocity - 16,
          ));
        }
        return notes;
      case 'block':
      default:
        return [
          for (final p in pitches)
            Note(
              pitch: p.clamp(12, 108),
              startTime: startTime,
              duration: duration,
              velocity: velocity,
            ),
        ];
    }
  }

  static const noteNames = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
}

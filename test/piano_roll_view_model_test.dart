import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/pattern.dart';
import 'package:zenith_audio/widgets/editor/piano_roll_view_model.dart';

/// S6b: piano-roll display helpers — scale highlighting, ghost notes, row
/// grouping.
void main() {
  Note n(int pitch, int start, {int len = 240}) =>
      Note(pitch: pitch, startTicks: start, lengthTicks: len);

  group('scale highlighting', () {
    test('the C major scale contains exactly the white keys', () {
      const scale = ScaleHighlight();
      for (final pitch in [60, 62, 64, 65, 67, 69, 71]) {
        expect(scale.contains(pitch), isTrue, reason: 'pitch $pitch is diatonic');
      }
      for (final pitch in [61, 63, 66, 68, 70]) {
        expect(scale.contains(pitch), isFalse, reason: 'pitch $pitch is chromatic');
      }
    });

    test('membership is pitch-class based, so every octave matches', () {
      const scale = ScaleHighlight();
      expect(scale.contains(60), isTrue);
      expect(scale.contains(72), isTrue);
      expect(scale.contains(0), isTrue);
      expect(scale.contains(61), isFalse);
      expect(scale.contains(73), isFalse);
    });

    test('the root is distinguished from other scale tones', () {
      const scale = ScaleHighlight();
      expect(scale.isRoot(60), isTrue);
      expect(scale.isRoot(72), isTrue, reason: 'octave-equivalent');
      expect(scale.isRoot(62), isFalse);
    });

    test('a transposed key shifts membership', () {
      const g = ScaleHighlight(rootPitchClass: 7); // G major
      expect(g.contains(66), isTrue, reason: 'F# is in G major');
      expect(g.contains(65), isFalse, reason: 'F natural is not');
    });

    test('an empty scale contains everything (no highlighting)', () {
      const none = ScaleHighlight.none;
      expect(none.contains(61), isTrue);
      expect(none.contains(60), isTrue);
    });

    test('snapUp / snapDown find the nearest scale tone', () {
      const scale = ScaleHighlight();
      // 61 (C#) is between C (60) and D (62).
      expect(scale.snapUp(61), 62);
      expect(scale.snapDown(61), 60);
      // A diatonic pitch snaps to itself.
      expect(scale.snapUp(64), 64);
      expect(scale.snapDown(64), 64);
    });

    test('snapping at the ends stays in range', () {
      const scale = ScaleHighlight();
      expect(scale.snapUp(127) <= 127, isTrue);
      final low = scale.snapDown(0);
      expect(low >= 0, isTrue);
    });
  });

  group('scaleForKey', () {
    test('parses a natural key', () {
      final c = scaleForKey('C');
      expect(c.rootPitchClass, 0);
      expect(c.contains(60), isTrue);
    });

    test('parses a sharp key', () {
      final d = scaleForKey('D');
      expect(d.rootPitchClass, 2);
      expect(d.contains(66), isTrue, reason: 'F# is in D major');
      expect(d.contains(65), isFalse);
    });

    test('parses a flat key', () {
      final bb = scaleForKey('Bb');
      expect(bb.rootPitchClass, 10);
    });

    test('minor uses the natural-minor intervals', () {
      final a = scaleForKey('A', minor: true);
      expect(a.rootPitchClass, 9);
      expect(a.contains(69), isTrue, reason: 'A is the root');
      expect(a.contains(67), isTrue, reason: 'G natural is in A minor');
      expect(a.contains(68), isFalse, reason: 'G# is not in A natural minor');
    });

    test('an unknown key falls back to C', () {
      final unknown = scaleForKey('H#weird');
      expect(unknown.rootPitchClass, 0);
    });
  });

  group('ghost notes', () {
    final reference = Pattern(
      id: 'ref',
      name: 'Ref',
      notes: [n(60, 0), n(72, 240), n(48, 480)],
    );

    test('only notes within the pitch range are returned', () {
      final ghosts = ghostNotes(reference, minPitch: 55, maxPitch: 80);
      expect(ghosts.map((g) => g.pitch).toList(), [60, 72]);
    });

    test('ghosts are sorted by start tick', () {
      final shuffled = Pattern(
        id: 'r',
        name: 'R',
        notes: [n(60, 480), n(62, 0), n(64, 240)],
      );
      final ghosts = ghostNotes(shuffled, minPitch: 0, maxPitch: 127);
      expect(ghosts.map((g) => g.startTicks).toList(), [0, 240, 480]);
    });

    test('an empty reference yields no ghosts', () {
      expect(
        ghostNotes(const Pattern(id: 'e', name: 'E'), minPitch: 0, maxPitch: 127),
        isEmpty,
      );
    });
  });

  group('note span fraction', () {
    test('maps a note to its fraction of the timeline', () {
      final span = noteSpanFraction(n(60, 0, len: 480), totalTicks: 960);
      expect(span.start, 0.0);
      expect(span.length, closeTo(0.5, 1e-9));
    });

    test('clamps to the timeline', () {
      final span = noteSpanFraction(n(60, 960, len: 960), totalTicks: 960);
      expect(span.start, 1.0);
      expect(span.length, 0.0);
    });

    test('a zero-length timeline is handled', () {
      final span = noteSpanFraction(n(60, 0), totalTicks: 0);
      expect(span.start, 0.0);
      expect(span.length, 0.0);
    });
  });

  group('notes by pitch', () {
    test('groups and sorts each row', () {
      final rows = notesByPitch([n(60, 480), n(60, 0), n(62, 240)]);
      expect(rows.keys.toSet(), {60, 62});
      expect(rows[60]!.map((e) => e.startTicks).toList(), [0, 480]);
      expect(rows[62]!.single.startTicks, 240);
    });

    test('an empty input yields an empty map', () {
      expect(notesByPitch(const []), isEmpty);
    });
  });
}

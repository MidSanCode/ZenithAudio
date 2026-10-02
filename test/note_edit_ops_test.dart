import 'dart:math' as math;

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/services/note_edit_ops.dart';

/// S6b: the pure note transforms behind quantise, swing and the velocity tools.
void main() {
  Note n(int start, {int len = 240, int vel = 100, int pitch = 60}) =>
      Note(pitch: pitch, startTicks: start, lengthTicks: len, velocity: vel);

  group('quantizeStarts', () {
    test('full strength snaps to the nearest grid line', () {
      final notes = [n(10), n(500), n(970)];
      final result = NoteEditOps.quantizeStarts(notes, gridTicks: 480);
      expect(result.notes.map((e) => e.startTicks).toList(), [0, 480, 960]);
      expect(result.changed, 3);
    });

    test('half strength moves halfway to the grid', () {
      final notes = [n(100)];
      final result = NoteEditOps.quantizeStarts(notes, gridTicks: 480, strength: 0.5);
      // Target 0, current 100, half way => 50.
      expect(result.notes.single.startTicks, 50);
    });

    test('zero strength is a no-op', () {
      final notes = [n(123)];
      final result = NoteEditOps.quantizeStarts(notes, gridTicks: 480, strength: 0);
      expect(result.notes.single.startTicks, 123);
      expect(result.isUnchanged, isTrue);
    });

    test('a bad grid is a no-op rather than a divide by zero', () {
      final notes = [n(123)];
      final result = NoteEditOps.quantizeStarts(notes, gridTicks: 0);
      expect(result.notes.single.startTicks, 123);
    });

    test('quantising does not change durations', () {
      final notes = [n(100, len: 333)];
      final result = NoteEditOps.quantizeStarts(notes, gridTicks: 480);
      expect(result.notes.single.lengthTicks, 333);
    });

    test('an already-grooved note keeps its offset under swing, not quantise', () {
      // (cross-check that the two operations do different things)
      final grooved = [n(500)]; // 20 ticks late of the 480 grid
      final swung = NoteEditOps.swing(grooved, gridTicks: 480, amount: 1.0);
      // 500 is nearest step 1 (odd) so it moves by 160 to 660.
      expect(swung.notes.single.startTicks, 660);
    });
  });

  group('swing', () {
    test('even steps do not move', () {
      final notes = [n(0), n(960), n(1920)];
      final result = NoteEditOps.swing(notes, gridTicks: 480, amount: 1.0);
      expect(result.notes.map((e) => e.startTicks).toList(), [0, 960, 1920]);
      expect(result.changed, 0);
    });

    test('odd steps move by a third at full amount', () {
      // 480 is step 1 (odd): full swing shifts by 480/3 = 160.
      final notes = [n(480)];
      final result = NoteEditOps.swing(notes, gridTicks: 480, amount: 1.0);
      expect(result.notes.single.startTicks, 640);
    });

    test('half amount shifts by half the triplet offset', () {
      final notes = [n(480)];
      final result = NoteEditOps.swing(notes, gridTicks: 480, amount: 0.5);
      expect(result.notes.single.startTicks, 560); // 480 + 80
    });

    test('zero amount is a no-op', () {
      final notes = [n(480)];
      final result = NoteEditOps.swing(notes, gridTicks: 480, amount: 0.0);
      expect(result.notes.single.startTicks, 480);
      expect(result.isUnchanged, isTrue);
    });

    test('an off-grid note keeps its offset from the swung grid line', () {
      // A note 20 ticks past the odd grid line at 480 should land 20 past the
      // swung position (640), i.e. 660.
      final notes = [n(500)];
      final result = NoteEditOps.swing(notes, gridTicks: 480, amount: 1.0);
      expect(result.notes.single.startTicks, 660);
    });
  });

  group('velocityRamp', () {
    test('interpolates across notes ordered by start time', () {
      final notes = [n(960, vel: 100), n(0, vel: 100), n(1920, vel: 100)];
      final result = NoteEditOps.velocityRamp(notes, from: 20, to: 120);
      // Sorted by start: index 1 (tick 0) => 20, index 0 (tick 960) => 70,
      // index 2 (tick 1920) => 120.
      expect(result.notes[1].velocity, 20);
      expect(result.notes[0].velocity, 70);
      expect(result.notes[2].velocity, 120);
    });

    test('a single note gets the end value', () {
      final result = NoteEditOps.velocityRamp([n(0)], from: 10, to: 90);
      expect(result.notes.single.velocity, 90);
    });

    test('velocities stay inside 1..127', () {
      final result = NoteEditOps.velocityRamp(
        [n(0), n(480)],
        from: 0,
        to: 200,
      );
      expect(result.notes.map((e) => e.velocity).toList(), [1, 127]);
    });
  });

  group('velocityRandomize', () {
    test('stays within the requested range with a pinned RNG', () {
      final notes = List.generate(20, (i) => n(i * 240, vel: 64));
      // A fixed seed makes the result reproducible.
      final result = NoteEditOps.velocityRandomize(
        notes,
        range: 10,
        random: math.Random(42),
      );
      for (final note in result.notes) {
        expect((note.velocity - 64).abs(), lessThanOrEqualTo(10));
      }
    });

    test('a zero range is a no-op', () {
      final notes = [n(0, vel: 64)];
      final result = NoteEditOps.velocityRandomize(notes, range: 0);
      expect(result.isUnchanged, isTrue);
    });
  });

  group('velocityScale', () {
    test('compressing pulls extreme values toward the centre', () {
      final notes = [n(0, vel: 127), n(240, vel: 0 + 1), n(480, vel: 64)];
      final result = NoteEditOps.velocityScale(notes, factor: 0.5, centre: 64);
      // 127 -> 64 + 63*0.5 = 95.5 -> 96; 1 -> 64 + (-63)*0.5 = 32.5 -> 33.
      expect(result.notes[0].velocity, 96);
      expect(result.notes[1].velocity, 33);
      expect(result.notes[2].velocity, 64);
    });

    test('expanding pushes values away from the centre and clamps', () {
      final notes = [n(0, vel: 127), n(240, vel: 1)];
      final result = NoteEditOps.velocityScale(notes, factor: 2.0, centre: 64);
      expect(result.notes[0].velocity, 127, reason: 'clamped at the top');
      expect(result.notes[1].velocity, 1, reason: 'clamped at the bottom');
    });

    test('factor 1 is a no-op', () {
      final result = NoteEditOps.velocityScale([n(0, vel: 42)], factor: 1.0);
      expect(result.isUnchanged, isTrue);
    });
  });

  group('transpose', () {
    test('shifts pitch and clamps at the ends', () {
      final notes = [n(0, pitch: 60), n(0, pitch: 0), n(0, pitch: 127)];
      final up = NoteEditOps.transpose(notes, 12);
      expect(up.notes.map((e) => e.pitch).toList(), [72, 12, 127]);
      final down = NoteEditOps.transpose(notes, -12);
      expect(down.notes.map((e) => e.pitch).toList(), [48, 0, 115]);
    });

    test('zero is a no-op', () {
      expect(NoteEditOps.transpose([n(0)], 0).isUnchanged, isTrue);
    });
  });

  group('notesInRange', () {
    test('includes notes that merely overlap the range', () {
      final notes = [n(0, len: 480), n(960), n(2000)];
      final inRange = NoteEditOps.notesInRange(notes, startTick: 100, endTick: 1000);
      expect(inRange.length, 2, reason: 'the first overlaps, the second starts in');
    });

    test('an inverted range selects nothing', () {
      expect(NoteEditOps.notesInRange([n(0)], startTick: 100, endTick: 0), isEmpty);
    });
  });
}

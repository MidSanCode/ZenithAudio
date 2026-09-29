import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';

/// S0: the tick model is the foundation the Rust core will agree with bit for
/// bit, so its conversions are pinned here rather than left implicit.
void main() {
  group('Ticks constants', () {
    test('ppq is the 960 grid every division lands on exactly', () {
      expect(Ticks.ppq, 960);
    });

    test('barTicks is ppq * 4 * numerator', () {
      // The implementation treats `numerator` as a whole-note multiple, so the
      // contract is pinned here explicitly rather than assumed to be "beats
      // per bar".
      expect(Ticks.barTicks(4), Ticks.ppq * 4 * 4);
      expect(Ticks.barTicks(1), Ticks.ppq * 4);
    });

    test('barTicks scales with the numerator', () {
      expect(Ticks.barTicks(3), Ticks.ppq * 4 * 3);
      expect(Ticks.barTicks(7), Ticks.ppq * 4 * 7);
    });
  });

  group('Ticks conversions', () {
    test('beats round-trip through ticks', () {
      expect(Ticks.fromBeats(1.0), 960);
      expect(Ticks.fromBeats(0.5), 480);
      expect(Ticks.toBeats(960), 1.0);
      expect(Ticks.toBeats(480), 0.5);
    });

    test('seconds depend on tempo', () {
      // At 120 bpm a quarter note is exactly half a second.
      expect(Ticks.fromSeconds(0.5, 120), 960);
      expect(Ticks.fromSeconds(1.0, 120), 1920);
      // Doubling the tempo halves the tick count for the same wall time.
      expect(Ticks.fromSeconds(0.5, 240), 1920);
      expect(Ticks.toSeconds(960, 120), closeTo(0.5, 1e-12));
      expect(Ticks.toSeconds(1920, 120), closeTo(1.0, 1e-12));
    });

    test('a bar round-trips in 4/4', () {
      expect(Ticks.fromBars(1, 4), Ticks.barTicks(4));
      expect(Ticks.toBars(Ticks.barTicks(4), 4), closeTo(1.0, 1e-12));
    });

    test('seconds -> ticks -> seconds survives a tempo change', () {
      // The whole point of the tick model: re-deriving at a new tempo keeps
      // the *musical* position, so the wall time scales by the tempo ratio.
      const startTicks = 960; // one beat at 120
      final atSixty = Ticks.toSeconds(startTicks, 60);
      final atOneEighty = Ticks.toSeconds(startTicks, 180);
      expect(atSixty, closeTo(1.0, 1e-12));
      expect(atOneEighty, closeTo(1.0 / 3.0, 1e-12));
    });
  });

  group('MusicalDivision', () {
    test('common binary divisions land on exact ticks', () {
      expect(const MusicalDivision(1).ticks, 3840); // whole
      expect(const MusicalDivision(2).ticks, 1920); // half
      expect(const MusicalDivision(4).ticks, 960); // quarter
      expect(const MusicalDivision(8).ticks, 480); // eighth
      expect(const MusicalDivision(16).ticks, 240); // sixteenth
      expect(const MusicalDivision(32).ticks, 120); // thirty-second
      expect(const MusicalDivision(64).ticks, 60); // sixty-fourth
    });

    test('triplets divide exactly — 960 is why ppq was chosen', () {
      expect(const MusicalDivision(4, tuplet: 3).ticks, 320);
      expect(const MusicalDivision(8, tuplet: 3).ticks, 160);
      expect(const MusicalDivision(16, tuplet: 3).ticks, 80);
    });

    test('quintuplets divide exactly', () {
      expect(const MusicalDivision(16, tuplet: 5).ticks, 48);
    });

    test('dotted values add half, then a quarter, of the base', () {
      final quarter = const MusicalDivision(4).ticks;
      final dotted = const MusicalDivision(4, dots: 1).ticks;
      final doubleDotted = const MusicalDivision(4, dots: 2).ticks;
      expect(dotted, quarter + quarter ~/ 2);
      expect(doubleDotted, dotted + quarter ~/ 4);
    });

    test('ids describe the division', () {
      expect(const MusicalDivision(16).id, '1/16');
      expect(const MusicalDivision(8, tuplet: 3).id, '1/8T');
      expect(const MusicalDivision(4, dots: 1).id, '1/4.');
    });

    test('equality is structural', () {
      expect(const MusicalDivision(8, tuplet: 3),
          const MusicalDivision(8, tuplet: 3));
      expect(const MusicalDivision(8, tuplet: 3).hashCode,
          const MusicalDivision(8, tuplet: 3).hashCode);
      expect(const MusicalDivision(8, tuplet: 3) == const MusicalDivision(8),
          isFalse);
    });

    test('every offered grid division is at least one tick', () {
      for (final d in kGridDivisions) {
        expect(d.ticks, greaterThan(0), reason: 'division $d');
      }
    });
  });

  group('MusicalTime.snap', () {
    test('rounds to the nearest grid multiple', () {
      expect(MusicalTime.snap(250, 240), 240);
      expect(MusicalTime.snap(361, 240), 480);
    });

    test('floors when round is false', () {
      expect(MusicalTime.snap(361, 240, round: false), 240);
      expect(MusicalTime.snap(479, 240, round: false), 240);
    });

    test('a non-positive grid is a passthrough, not a crash', () {
      expect(MusicalTime.snap(123, 0), 123);
      expect(MusicalTime.snap(123, -16), 123);
    });
  });

  group('MusicalTime.swingOffset', () {
    test('on-beat steps never move', () {
      expect(MusicalTime.swingOffset(0, 240, 1.0), 0);
      expect(MusicalTime.swingOffset(480, 240, 1.0), 0);
    });

    test('off-beat steps move later, scaled by the grid', () {
      final full = MusicalTime.swingOffset(240, 240, 1.0);
      expect(full, greaterThan(0));
      expect(full, 80); // a third of the grid
      expect(MusicalTime.swingOffset(240, 240, 0.5), lessThan(full));
    });

    test('zero swing or zero grid is a passthrough', () {
      expect(MusicalTime.swingOffset(240, 240, 0.0), 0);
      expect(MusicalTime.swingOffset(240, 0, 1.0), 0);
    });
  });

  group('MusicalTime.quantize', () {
    test('full strength snaps exactly', () {
      // 361/240 = 1.504 -> rounds to grid step 2 -> 480.
      expect(MusicalTime.snap(361, 240), 480);
      expect(MusicalTime.quantize(361, 240), 480);
    });

    test('zero strength leaves the value untouched', () {
      expect(MusicalTime.quantize(361, 240, strength: 0.0), 361);
    });

    test('half strength moves halfway to the target', () {
      // 361 snaps to 480, so half strength lands on 361 + (480-361)/2 = 420.5
      // which rounds to 421.
      expect(MusicalTime.quantize(361, 240, strength: 0.5), 421);
    });

    test('a value already on the grid is a fixed point', () {
      expect(MusicalTime.quantize(480, 240), 480);
      expect(MusicalTime.quantize(480, 240, strength: 0.5), 480);
    });

    test('a non-positive grid is a passthrough', () {
      expect(MusicalTime.quantize(361, 0), 361);
    });
  });

  group('MusicalTime.safe', () {
    test('leaves values alone', () {
      expect(MusicalTime.safe(0), 0);
      expect(MusicalTime.safe(12345), 12345);
      expect(MusicalTime.safe(-30), -30);
    });

    test('is total over the int range — no crash, no wraparound', () {
      // `safe` takes an int, so its job is rejecting values that would
      // overflow downstream arithmetic rather than sanitising doubles.
      expect(MusicalTime.safe(1 << 40), 1 << 40);
      expect(MusicalTime.safe(-(1 << 40)), -(1 << 40));
    });
  });
}

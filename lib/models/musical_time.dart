import 'dart:math' as math;

/// Musical time base used by the whole editor.
///
/// The engine stores notes on a **pulse-per-quarter-note** grid so that
/// tempo changes and time-signature changes never re-quantise the music.
/// Seconds remain a derived, presentation-layer value.
class Ticks {
  Ticks._();

  /// Pulses per quarter note. Chosen so that every common subdivision —
  /// binary (1/32 … 1/1), ternary (triplets), quintuplets and 1/64 — lands on
  /// an exact integer tick.
  static const int ppq = 960;

  /// One bar in [timeSignatureNumerator]/4.
  static int barTicks(int numerator) => ppq * 4 * numerator;

  /// Conversions to ticks.
  static int fromBeats(double beats) => (beats * ppq).round();
  static int fromSeconds(double seconds, double bpm) =>
      fromBeats(seconds * bpm / 60.0);
  static int fromBars(double bars, int numerator) =>
      (bars * barTicks(numerator)).round();

  /// Conversions from ticks.
  static double toBeats(int ticks) => ticks / ppq;
  static double toSeconds(int ticks, double bpm) =>
      ticks / ppq * 60.0 / bpm;
  static double toBars(int ticks, int numerator) =>
      ticks / barTicks(numerator);
}

/// A grid / note-length value expressed as ticks.
///
/// [denominator] is the note value (4 = quarter, 8 = eighth …) and
/// [dots] the number of augmentation dots; [tuplet] > 1 marks a tuplet
/// division (3 = triplet, 5 = quintuplet).
class MusicalDivision {
  final int denominator;
  final int dots;
  final int tuplet;

  const MusicalDivision(this.denominator, {this.dots = 0, this.tuplet = 1});

  /// Ticks covered by one step of this division.
  int get ticks {
    var value = Ticks.ppq * 4 / denominator; // whole = 4 quarters
    var add = value / 2;
    for (int i = 0; i < dots; i++) {
      value += add;
      add /= 2;
    }
    return (value / tuplet).round();
  }

  /// Localisation id, e.g. `1/16`, `1/8T`, `1/4.`.
  String get id {
    final suffix = tuplet == 3
        ? 'T'
        : tuplet == 5
            ? 'Q'
            : '.' * dots;
    return '1/$denominator$suffix';
  }

  @override
  bool operator ==(Object other) =>
      other is MusicalDivision &&
      other.denominator == denominator &&
      other.dots == dots &&
      other.tuplet == tuplet;

  @override
  int get hashCode => Object.hash(denominator, dots, tuplet);

  @override
  String toString() => id;
}

/// The default set offered by the piano-roll grid selector.
const List<MusicalDivision> kGridDivisions = [
  MusicalDivision(1),
  MusicalDivision(2),
  MusicalDivision(4),
  MusicalDivision(4, dots: 1),
  MusicalDivision(8),
  MusicalDivision(8, tuplet: 3),
  MusicalDivision(16),
  MusicalDivision(16, tuplet: 3),
  MusicalDivision(32),
];

/// Musical-position arithmetic shared by the piano roll and the playlist.
class MusicalTime {
  MusicalTime._();

  /// Snaps [ticks] to the nearest multiple of [grid], or to the floor when
  /// [round] is false.
  static int snap(int ticks, int grid, {bool round = true}) {
    if (grid <= 0) return ticks;
    if (round) return ((ticks / grid).round()) * grid;
    return (ticks / grid).floor() * grid;
  }

  /// Applies a swing feel to a straight grid position.
  ///
  /// [swing] is 0 (straight) … 1 (full triplet feel). Only off-beat grid
  /// steps move; the amount is scaled by the grid length.
  static int swingOffset(int ticks, int grid, double swing) {
    if (grid <= 0 || swing <= 0) return 0;
    final step = (ticks / grid).round();
    if (step.isEven) return 0;
    // Full swing pushes the off-beat to the 2/3 point of the pair.
    final full = (grid / 3).round();
    return (full * swing.clamp(0.0, 2.0)).round();
  }

  /// Quantises [ticks] toward [grid] with a strength of 0…1.
  static int quantize(int ticks, int grid, {double strength = 1.0}) {
    if (grid <= 0) return ticks;
    final target = snap(ticks, grid);
    final s = strength.clamp(0.0, 1.0);
    return (ticks + (target - ticks) * s).round();
  }

  /// Rounds [ticks] to the nearest whole tick, guarding against NaN inputs.
  static int safe(int ticks) {
    if (ticks.isNaN) return 0;
    if (ticks.isInfinite) return math.max(0, ticks > 0 ? 1 << 40 : 0);
    return ticks;
  }
}

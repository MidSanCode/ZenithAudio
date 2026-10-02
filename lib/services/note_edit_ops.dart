/// Pure, tick-based note editing operations for the piano roll (PLAN §3.S6b).
///
/// ## Why a separate service
///
/// These operations are the *musical* transforms — quantise, swing, velocity
/// shaping — and they are independent of any widget, provider or audio engine.
/// Keeping them here means they can be reasoned about and tested as pure
/// functions of a note list, which is where the bugs actually live: a swing
/// that moves the wrong grid step, or a velocity ramp that clips to zero, is a
/// correctness problem, not a UI problem.
///
/// ## Ticks, not seconds
///
/// Everything operates on [Note.startTicks] / [Note.lengthTicks] — the
/// authoritative values (see `note.dart`). The seconds view is re-derived at
/// the end so callers that still read `startTime` see consistent numbers.
///
/// ## The swing subtlety
///
/// Swing displaces **off-beat** grid steps later, by `amount` of the way from
/// the straight position toward the 2/3 (triplet) position. Even steps do not
/// move. This is applied as a *delta from the nearest grid line* so a note that
/// was slightly off the grid keeps its offset rather than being snapped — swing
/// is a feel, not a quantiser.
library;

import 'dart:math' as math;

import '../models/musical_time.dart';
import '../models/note.dart';

/// The result of a velocity operation, kept as a value so tests can assert on
/// the distribution rather than on individual notes.
class NoteEditResult {
  /// The transformed notes, same length and order as the input.
  final List<Note> notes;

  /// How many notes the operation actually changed.
  final int changed;

  const NoteEditResult({required this.notes, required this.changed});

  /// Whether the operation was a no-op.
  bool get isUnchanged => changed == 0;
}

/// A collection of stateless note transforms.
abstract final class NoteEditOps {
  /// Quantises note starts toward the nearest `gridTicks` line.
  ///
  /// [strength] is `0..1`: `1` snaps fully to the grid, `0` leaves the note
  /// untouched, and `0.5` moves it halfway. Strength is what makes quantisation
  /// usable on a performance — full snap destroys the groove.
  ///
  /// Note **durations are not quantised**: quantising length is a separate,
  /// much more destructive operation and is not what a "quantise" button is
  /// expected to do.
  static NoteEditResult quantizeStarts(
    List<Note> notes, {
    required int gridTicks,
    double strength = 1.0,
  }) {
    if (gridTicks <= 0) {
      return NoteEditResult(notes: notes, changed: 0);
    }
    final s = strength.clamp(0.0, 1.0);
    var changed = 0;
    final out = <Note>[];
    for (final note in notes) {
      final target = MusicalTime.quantize(note.startTicks, gridTicks, strength: s);
      if (target == note.startTicks) {
        out.add(note);
      } else {
        out.add(note.copyWith(startTicks: target, bpm: note.bpm));
        changed++;
      }
    }
    return NoteEditResult(notes: out, changed: changed);
  }

  /// Applies a swing feel by pushing off-beat notes later.
  ///
  /// [amount] is `0` (straight) to `1` (full triplet feel, the off-beat landing
  /// on the 2/3 point). Only notes whose nearest grid step is odd move, by the
  /// extra `amount * gridTicks / 3`; the note's own offset from that grid line
  /// is preserved, so a swung take does not get flattened onto the grid.
  static NoteEditResult swing(
    List<Note> notes, {
    required int gridTicks,
    required double amount,
  }) {
    if (gridTicks <= 0 || amount == 0) {
      return NoteEditResult(notes: notes, changed: 0);
    }
    final a = amount.clamp(0.0, 1.0);
    final shift = (gridTicks / 3.0 * a).round();
    if (shift == 0) {
      return NoteEditResult(notes: notes, changed: 0);
    }
    var changed = 0;
    final out = <Note>[];
    for (final note in notes) {
      // Which grid step is the note nearest? Even steps are downbeats.
      final step = (note.startTicks / gridTicks).round();
      final isOffBeat = step.isOdd;
      if (!isOffBeat) {
        out.add(note);
        continue;
      }
      // Push the off-beat by the swing shift, never below its own start.
      final target = math.max(0, note.startTicks + shift);
      if (target == note.startTicks) {
        out.add(note);
      } else {
        out.add(note.copyWith(startTicks: target, bpm: note.bpm));
        changed++;
      }
    }
    return NoteEditResult(notes: out, changed: changed);
  }

  /// Applies a velocity ramp across the notes, ordered by start time.
  ///
  /// [from] and [to] are MIDI velocities (`1..127`); notes are ordered by
  /// [Note.startTicks] and interpolated between them. A single note gets [to]
  /// rather than a division by zero.
  static NoteEditResult velocityRamp(
    List<Note> notes, {
    required int from,
    required int to,
  }) {
    if (notes.isEmpty) return NoteEditResult(notes: notes, changed: 0);
    final lo = from.clamp(1, 127);
    final hi = to.clamp(1, 127);
    // Order by start so the ramp follows the music, not the list order.
    final order = List<int>.generate(notes.length, (i) => i)
      ..sort((a, b) => notes[a].startTicks.compareTo(notes[b].startTicks));

    var changed = 0;
    final out = List<Note>.from(notes);
    for (var rank = 0; rank < order.length; rank++) {
      final index = order[rank];
      final t = order.length == 1 ? 1.0 : rank / (order.length - 1);
      final value = (lo + (hi - lo) * t).round().clamp(1, 127);
      if (value != notes[index].velocity) {
        out[index] = notes[index].copyWith(velocity: value);
        changed++;
      }
    }
    return NoteEditResult(notes: out, changed: changed);
  }

  /// Randomises velocities by up to `±range` around each note's current value.
  ///
  /// [random] is injected so a test can pin the result; it defaults to
  /// [math.Random].
  static NoteEditResult velocityRandomize(
    List<Note> notes, {
    required int range,
    math.Random? random,
  }) {
    if (range <= 0 || notes.isEmpty) {
      return NoteEditResult(notes: notes, changed: 0);
    }
    final rng = random ?? math.Random();
    var changed = 0;
    final out = <Note>[];
    for (final note in notes) {
      final delta = rng.nextInt(range * 2 + 1) - range;
      final value = (note.velocity + delta).clamp(1, 127);
      if (value == note.velocity) {
        out.add(note);
      } else {
        out.add(note.copyWith(velocity: value));
        changed++;
      }
    }
    return NoteEditResult(notes: out, changed: changed);
  }

  /// Compresses or expands the velocity range around a centre.
  ///
  /// [factor] `> 1` expands (quieter notes get quieter, louder get louder);
  /// `0 < factor < 1` compresses toward [centre] (default `64`). This is the
  /// "tighten up the dynamics" / "open them out" control.
  static NoteEditResult velocityScale(
    List<Note> notes, {
    required double factor,
    int centre = 64,
  }) {
    if (!factor.isFinite || factor <= 0 || factor == 1.0 || notes.isEmpty) {
      return NoteEditResult(notes: notes, changed: 0);
    }
    var changed = 0;
    final out = <Note>[];
    for (final note in notes) {
      final scaled = (centre + (note.velocity - centre) * factor).round();
      final value = scaled.clamp(1, 127);
      if (value == note.velocity) {
        out.add(note);
      } else {
        out.add(note.copyWith(velocity: value));
        changed++;
      }
    }
    return NoteEditResult(notes: out, changed: changed);
  }

  /// Transposes every note by `semitones`, clamping to `[0, 127]`.
  static NoteEditResult transpose(List<Note> notes, int semitones) {
    if (semitones == 0 || notes.isEmpty) {
      return NoteEditResult(notes: notes, changed: 0);
    }
    var changed = 0;
    final out = <Note>[];
    for (final note in notes) {
      final pitch = (note.pitch + semitones).clamp(0, 127);
      if (pitch == note.pitch) {
        out.add(note);
      } else {
        out.add(note.copyWith(pitch: pitch));
        changed++;
      }
    }
    return NoteEditResult(notes: out, changed: changed);
  }

  /// Restricts notes to those overlapping `[startTick, endTick)`.
  ///
  /// Used by "select range" before applying a destructive transform.
  static List<Note> notesInRange(
    List<Note> notes, {
    required int startTick,
    required int endTick,
  }) {
    if (endTick <= startTick) return const [];
    return notes
        .where((n) => n.startTicks < endTick && n.endTicks > startTick)
        .toList();
  }
}

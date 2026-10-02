import 'dart:typed_data';

import '../../models/musical_time.dart';
import '../../models/pattern.dart';
import 'smf_types.dart';

/// Writes the project's tick-based [Pattern] model to a Standard MIDI File.
///
/// Format 1 is emitted when there is more than one pattern and format 0 when
/// there is exactly one, which is what most readers expect. Tempo and
/// time-signature meta events are written into the first track so the file
/// plays at the right speed.
class SmfWriter {
  /// MIDI note number for middle C (C4 = 60 in the scientific convention used
  /// by most DAWs and by this project's editor).
  static const int middleC = 60;

  /// Encodes [patterns] as a format 1 (or 0) Standard MIDI File.
  ///
  /// [bpm] is written as the initial tempo; [timeSignature] defaults to 4/4.
  /// [ppq] defaults to the project grid ([Ticks.ppq]).
  static Uint8List write({
    required List<Pattern> patterns,
    double bpm = 120.0,
    (int, int) timeSignature = (4, 4),
    int ppq = Ticks.ppq,
  }) {
    final format = patterns.length <= 1 ? 0 : 1;
    final output = BytesBuilder(copy: false);

    // Collect the track chunks first, then write the header, so the track count
    // in the header is exact.
    final tracks = <List<int>>[];
    if (format == 0) {
      // Format 0: one track. Tempo/time-signature go in that same track.
      final pattern = patterns.isEmpty ? null : patterns.first;
      if (pattern == null) {
        tracks.add(_trackChunk([..._metaEvents(bpm, timeSignature, ppq)]));
      } else {
        tracks.add(_trackChunk(_eventsForPattern(
          pattern,
          includeMeta: true,
          bpm: bpm,
          timeSignature: timeSignature,
          ppq: ppq,
          name: pattern.name,
        )));
      }
    } else {
      // Format 1: a conductor track (tempo + time signature) followed by one
      // track per pattern. A conductor track is the conventional layout and is
      // what most readers expect for tempo.
      tracks.add(_trackChunk(_metaEvents(bpm, timeSignature, ppq)));
      for (final pattern in patterns) {
        tracks.add(_trackChunk(_eventsForPattern(
          pattern,
          includeMeta: false,
          bpm: bpm,
          timeSignature: timeSignature,
          ppq: ppq,
          name: pattern.name,
        )));
      }
    }

    // ── MThd ──
    output.add(_ascii('MThd'));
    output.add(_uint32(6));
    output.add(_uint16(format));
    output.add(_uint16(tracks.length));
    output.add(_uint16(ppq));

    for (final track in tracks) {
      output.add(track);
    }
    return output.toBytes();
  }

  /// Builds the event stream for one pattern.
  static List<int> _eventsForPattern(
    Pattern pattern, {
    required bool includeMeta,
    required double bpm,
    required (int, int) timeSignature,
    required int ppq,
    required String? name,
  }) {
    final events = <_Event>[];
    for (final note in pattern.notes) {
      final pitch = note.pitch.clamp(0, 127);
      final velocity = note.velocity.clamp(1, 127);
      events.add(_Event(
        note.startTicks,
        order: 1,
        // Channel 0 for every pattern: the project has no per-track MIDI
        // channel concept yet, and a reader splitting by track does not need one.
        bytes: [0x90, pitch, velocity],
      ));
      final end = note.startTicks + (note.lengthTicks < 1 ? 1 : note.lengthTicks);
      events.add(_Event(end, order: 0, bytes: [0x80, pitch, 0]));
    }

    final out = <int>[];
    if (includeMeta) {
      out.addAll(_metaEvents(bpm, timeSignature, ppq));
    } else if (name != null && name.isNotEmpty) {
      // A track-name meta event helps a reader label the imported track.
      final nameBytes = _ascii(name, allowTruncate: true);
      out.addAll([0x00, 0xFF, 0x03, nameBytes.length, ...nameBytes]);
    }

    // Sort by tick; note-offs (order 0) before note-ons (order 1) at the same
    // tick, so a repeated note is not lost.
    events.sort((a, b) {
      final byTick = a.tick.compareTo(b.tick);
      return byTick != 0 ? byTick : a.order.compareTo(b.order);
    });

    var previousTick = 0;
    for (final event in events) {
      final delta = event.tick - previousTick;
      out.addAll(encodeVarLen(delta < 0 ? 0 : delta));
      out.addAll(event.bytes);
      previousTick = event.tick;
    }
    out.addAll(_endOfTrack());
    return out;
  }

  /// Tempo and time-signature meta events at tick 0, plus end-of-track.
  static List<int> _metaEvents(double bpm, (int, int) timeSignature, int ppq) {
    final out = <int>[];
    // Tempo: microseconds per quarter note.
    final upq = bpm > 0 ? (60000000 / bpm).round() : 500000;
    out.addAll(encodeVarLen(0));
    out.addAll([0xFF, 0x51, 0x03, (upq >> 16) & 0xFF, (upq >> 8) & 0xFF, upq & 0xFF]);
    // Time signature: numerator, log2(denominator), 24 MIDI clocks/click,
    // 8 32nd-notes per quarter.
    final (numerator, denominator) = timeSignature;
    var denPower = 0;
    var den = denominator;
    while (den > 1) {
      den >>= 1;
      denPower++;
    }
    out.addAll(encodeVarLen(0));
    out.addAll([0xFF, 0x58, 0x04, numerator & 0xFF, denPower & 0xFF, 0x18, 0x08]);
    out.addAll(_endOfTrack());
    return out;
  }

  /// Wraps `events` in an MTrk chunk.
  static List<int> _trackChunk(List<int> events) {
    final header = <int>[..._ascii('MTrk'), ..._uint32(events.length)];
    return [...header, ...events];
  }

  static List<int> _endOfTrack() => [0x00, 0xFF, 0x2F, 0x00];

  static List<int> _ascii(String text, {bool allowTruncate = false}) {
    final bytes = <int>[];
    for (final rune in text.runes) {
      if (rune < 0x80) {
        bytes.add(rune);
      } else if (allowTruncate) {
        // Meta text such as a track name: keep it ASCII, dropping non-ASCII
        // runes rather than emitting malformed bytes.
        bytes.add(0x3F); // '?'
      } else {
        throw ArgumentError.value(text, 'text', 'must be ASCII');
      }
    }
    return bytes;
  }

  static List<int> _uint16(int value) => [(value >> 8) & 0xFF, value & 0xFF];

  static List<int> _uint32(int value) => [
        (value >> 24) & 0xFF,
        (value >> 16) & 0xFF,
        (value >> 8) & 0xFF,
        value & 0xFF,
      ];
}

/// One scheduled event during writing, with a tie-break order.
class _Event {
  final int tick;
  final int order;
  final List<int> bytes;

  const _Event(this.tick, {required this.order, required this.bytes});
}

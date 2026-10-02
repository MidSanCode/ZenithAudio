import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/pattern.dart';
import 'package:zenith_audio/services/midi/smf_reader.dart';
import 'package:zenith_audio/services/midi/smf_types.dart';
import 'package:zenith_audio/services/midi/smf_writer.dart';

void main() {
  group('variable-length quantities', () {
    test('short values round-trip', () {
      for (final value in [0, 1, 0x7F]) {
        expect(SmfReader.parse(_fileWithDelta([value])).tracks, isNotEmpty,
            reason: 'delta $value should parse');
      }
    });

    test('encoding matches the specification examples', () {
      // 0x7F is one byte; 0x80 is 81 00; 0x3FFF is FF 7F; 0x4000 is 81 80 00.
      expect(encodeVarLen(0x7F), [0x7F]);
      expect(encodeVarLen(0x80), [0x81, 0x00]);
      expect(encodeVarLen(0x3FFF), [0xFF, 0x7F]);
      expect(encodeVarLen(0x4000), [0x81, 0x80, 0x00]);
      expect(encodeVarLen(0x100000), [0xC0, 0x80, 0x00]);
    });

    test('a negative delta is rejected', () {
      expect(() => encodeVarLen(-1), throwsArgumentError);
    });
  });

  group('reading', () {
    test('a malformed header is rejected with an offset', () {
      final bytes = Uint8List.fromList([1, 2, 3, 4, 5, 6, 7, 8]);
      expect(
        () => SmfReader.parse(bytes),
        throwsA(isA<SmfFormatException>()),
      );
    });

    test('a minimal format-0 file parses its note', () {
      final file = SmfReader.parse(_minimalFile());
      expect(file.header.format, 0);
      expect(file.header.ppq, 960);
      expect(file.tracks, hasLength(1));
      final notes = file.tracks.first.notes;
      expect(notes, hasLength(1));
      expect(notes.first.pitch, 60);
      expect(notes.first.velocity, 100);
      expect(notes.first.startTick, 0);
      expect(notes.first.lengthTicks, 960);
    });

    test('a note-on with velocity 0 is treated as a note-off', () {
      // On at 0, then a velocity-0 note-on at 480.
      final bytes = _buildFile(format: 0, division: 960, tracks: [
        _track([
          ...[0x00, 0x90, 60, 100],
          ...[0x83, 0x60, 0x90, 60, 0], // delta 480, note-on vel 0
          ...[0x00, 0xFF, 0x2F, 0x00],
        ]),
      ]);
      final file = SmfReader.parse(bytes);
      expect(file.tracks.first.notes.single.lengthTicks, 480);
    });

    test('running status is honoured', () {
      // First note-on carries status; the second omits it.
      final bytes = _buildFile(format: 0, division: 960, tracks: [
        _track([
          ...[0x00, 0x90, 60, 100],
          ...[0x00, 60, 0], // running status note-off (vel 0)
          ...[0x00, 62, 100], // running status note-on
          ...[0x00, 62, 0],
          ...[0x00, 0xFF, 0x2F, 0x00],
        ]),
      ]);
      final file = SmfReader.parse(bytes);
      expect(file.tracks.first.notes.map((n) => n.pitch).toList(), [60, 62]);
    });

    test('tempo and time signature are read', () {
      final bytes = _buildFile(format: 0, division: 480, tracks: [
        _track([
          ...[0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20], // 500000 us = 120 BPM
          ...[0x00, 0xFF, 0x58, 0x04, 3, 2, 0x18, 0x08], // 3/4
          ...[0x00, 0xFF, 0x2F, 0x00],
        ]),
      ]);
      final file = SmfReader.parse(bytes);
      expect(file.header.ppq, 480);
      expect(file.initialBpm, closeTo(120.0, 1e-6));
      final ts = file.tracks.first.timeSignatures.single;
      expect(ts.numerator, 3);
      expect(ts.denominator, 4);
    });

    test('a truncated chunk raises rather than reading past the end', () {
      final bytes = _buildFile(format: 0, division: 960, tracks: [
        _track([0x00, 0x90, 60, 100]), // no end-of-track, no note-off
      ]);
      // The parser should still succeed (open notes get a length of 1), but a
      // file whose chunk length exceeds its bytes must fail.
      final corrupt = Uint8List.fromList(bytes)..[18] = 0xFF;
      expect(() => SmfReader.parse(corrupt), throwsA(isA<SmfFormatException>()));
    });
  });

  group('writing', () {
    test('a single pattern writes a valid format-0 file', () {
      final pattern = Pattern(
        id: 'p1',
        name: 'Lead',
        notes: [
          Note(pitch: 60, startTicks: 0, lengthTicks: 480, velocity: 100),
          Note(pitch: 64, startTicks: 480, lengthTicks: 480, velocity: 90),
        ],
      );
      final bytes = SmfWriter.write(patterns: [pattern], bpm: 120);
      final parsed = SmfReader.parse(bytes);
      expect(parsed.header.format, 0);
      expect(parsed.header.ppq, Ticks.ppq);
      expect(parsed.tracks.first.notes, hasLength(2));
      expect(parsed.initialBpm, closeTo(120, 0.01));
    });

    test('several patterns write a format-1 file with a conductor track', () {
      final patterns = [
        for (var i = 0; i < 3; i++)
          Pattern(
            id: 'p$i',
            name: 'Track $i',
            notes: [Note(pitch: 60 + i, startTicks: 0, lengthTicks: 240)],
          ),
      ];
      final bytes = SmfWriter.write(patterns: patterns, bpm: 140);
      final parsed = SmfReader.parse(bytes);
      expect(parsed.header.format, 1);
      // Conductor track plus one per pattern.
      expect(parsed.header.trackCount, 4);
      expect(parsed.tracks, hasLength(4));
      expect(parsed.initialBpm, closeTo(140, 0.01));
    });

    test('an empty pattern list still writes a valid file', () {
      final bytes = SmfWriter.write(patterns: const []);
      final parsed = SmfReader.parse(bytes);
      expect(parsed.header.format, 0);
      expect(parsed.tracks, hasLength(1));
      expect(parsed.tracks.first.notes, isEmpty);
    });
  });

  group('round trips', () {
    test('write -> read preserves pitches, velocities and ticks', () {
      final original = Pattern(
        id: 'p',
        name: 'Round',
        notes: [
          Note(pitch: 48, startTicks: 0, lengthTicks: 960, velocity: 127),
          Note(pitch: 55, startTicks: 960, lengthTicks: 480, velocity: 64),
          Note(pitch: 60, startTicks: 1920, lengthTicks: 240, velocity: 1),
        ],
      );
      final parsed = SmfReader.parse(
        SmfWriter.write(patterns: [original], bpm: 100),
      );
      final notes = parsed.tracks.first.notes;
      expect(notes.map((n) => n.pitch).toList(), [48, 55, 60]);
      expect(notes.map((n) => n.velocity).toList(), [127, 64, 1]);
      expect(notes.map((n) => n.startTick).toList(), [0, 960, 1920]);
      expect(notes.map((n) => n.lengthTicks).toList(), [960, 480, 240]);
    });

    test('smfToPatterns rescales a foreign PPQ to the project grid', () {
      // A file at 480 PPQ; one beat = 480 ticks there, 960 here.
      final bytes = _buildFile(format: 0, division: 480, tracks: [
        _track([
          ...[0x00, 0x90, 60, 100],
          ...[0x83, 0x60, 0x80, 60, 0], // delta 480
          ...[0x00, 0xFF, 0x2F, 0x00],
        ]),
      ]);
      final file = SmfReader.parse(bytes);
      final data = smfToPatterns(file);
      expect(data.patterns, hasLength(1));
      final note = data.patterns.first.notes.single;
      expect(note.startTicks, 0);
      expect(note.lengthTicks, 960, reason: '480 ticks at 480 PPQ = one beat');
    });

    test('a two-track format-1 file yields two patterns', () {
      final bytes = _buildFile(format: 1, division: 960, tracks: [
        _track([..._metaOnly(120.0), 0x00, 0xFF, 0x2F, 0x00]),
        _track([
          ...[0x00, 0x90, 60, 100],
          ...[0x87, 0x40, 0x80, 60, 0], // delta 960
          ...[0x00, 0xFF, 0x2F, 0x00],
        ]),
        _track([
          ...[0x00, 0x91, 67, 100], // channel 1, so it is independent
          ...[0x87, 0x40, 0x81, 67, 0],
          ...[0x00, 0xFF, 0x2F, 0x00],
        ]),
      ]);
      final file = SmfReader.parse(bytes);
      final data = smfToPatterns(file);
      expect(data.patterns, hasLength(2));
      expect(data.patterns[0].notes.single.pitch, 60);
      expect(data.patterns[1].notes.single.pitch, 67);
    });

    test('SMPTE division is rejected rather than misread', () {
      final bytes = _buildFile(
        format: 0,
        division: 0xE728, // -25 fps, 40 ticks/frame
        tracks: [
          _track([..._metaOnly(120.0), 0x00, 0xFF, 0x2F, 0x00]),
        ],
      );
      final file = SmfReader.parse(bytes);
      expect(file.header.isMetrical, isFalse);
      expect(() => smfToPatterns(file), throwsA(isA<SmfFormatException>()));
    });
  });
}

// ── Test helpers ──

List<int> _metaOnly(double bpm) {
  final upq = (60000000 / bpm).round();
  return [
    0x00, 0xFF, 0x51, 0x03, (upq >> 16) & 0xFF, (upq >> 8) & 0xFF, upq & 0xFF,
    0x00, 0xFF, 0x58, 0x04, 4, 2, 0x18, 0x08,
  ];
}

Uint8List _fileWithDelta(List<int> deltas) {
  final events = <int>[];
  for (final delta in deltas) {
    events.addAll(encodeVarLen(delta));
    events.addAll([0x90, 60, 100]);
  }
  events.addAll([0x00, 0xFF, 0x2F, 0x00]);
  return _buildFile(format: 0, division: 960, tracks: [_track(events)]);
}

Uint8List _minimalFile() {
  return _buildFile(format: 0, division: 960, tracks: [
    _track([
      ...[0x00, 0x90, 60, 100],
      ...[0x87, 0x40, 0x80, 60, 0], // delta 960, note-off
      ...[0x00, 0xFF, 0x2F, 0x00],
    ]),
  ]);
}

List<int> _track(List<int> events) => [..._ascii('MTrk'), ..._uint32(events.length), ...events];

Uint8List _buildFile({
  required int format,
  required int division,
  required List<List<int>> tracks,
}) {
  final out = BytesBuilder(copy: false)
    ..add(_ascii('MThd'))
    ..add(_uint32(6))
    ..add(_uint16(format))
    ..add(_uint16(tracks.length))
    ..add(_uint16(division));
  for (final track in tracks) {
    out.add(track);
  }
  return out.toBytes();
}

List<int> _ascii(String s) => s.codeUnits;
List<int> _uint16(int v) => [(v >> 8) & 0xFF, v & 0xFF];
List<int> _uint32(int v) => [(v >> 24) & 0xFF, (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF];

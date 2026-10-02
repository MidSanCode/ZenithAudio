import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/pattern.dart';
import 'package:zenith_audio/services/midi/smf_reader.dart';
import 'package:zenith_audio/services/midi/smf_types.dart';
import 'package:zenith_audio/services/midi/smf_writer.dart';

/// S6c: the pure MIDI conversion path — write patterns to bytes, read them
/// back, and convert a parsed file to project patterns. Deliberately free of
/// the Riverpod provider and the audio engine, so it runs with no device and no
/// media backend.
///
/// The provider's `importMidiBytes` / `exportMidiBytes` are thin wrappers over
/// exactly these functions plus `state` assignment; the conversion is where the
/// behaviour lives, so that is what is pinned here.
void main() {
  test('write -> parse -> smfToPatterns preserves notes and tempo', () {
    final pattern = Pattern(
      id: 'p',
      name: 'Lead',
      notes: [
        Note(pitch: 60, startTicks: 0, lengthTicks: 480, velocity: 100),
        Note(pitch: 64, startTicks: 480, lengthTicks: 480, velocity: 80),
      ],
    );
    final bytes = SmfWriter.write(patterns: [pattern], bpm: 96);
    final file = SmfReader.parse(bytes);
    final data = smfToPatterns(file);

    expect(data.patterns, hasLength(1));
    expect(data.bpm, closeTo(96, 0.5));
    final notes = data.patterns.first.notes;
    expect(notes.map((n) => n.pitch).toList(), [60, 64]);
    expect(notes.map((n) => n.startTicks).toList(), [0, 480]);
    expect(notes.map((n) => n.lengthTicks).toList(), [480, 480]);
  });

  test('a format-1 export yields one pattern per source pattern', () {
    final patterns = [
      for (var i = 0; i < 3; i++)
        Pattern(
          id: 'p$i',
          name: 'T$i',
          notes: [Note(pitch: 60 + i, startTicks: 0, lengthTicks: 240)],
        ),
    ];
    final bytes = SmfWriter.write(patterns: patterns, bpm: 100);
    final data = smfToPatterns(SmfReader.parse(bytes));
    expect(data.patterns, hasLength(3));
    expect(data.patterns.map((p) => p.notes.single.pitch).toList(), [60, 61, 62]);
  });

  test('the imported pattern loops to whole bars past its content', () {
    // A note ending at 1000 ticks (well inside one 4/4 bar of 15 360 ticks)
    // should give a one-bar pattern.
    final pattern = Pattern(
      id: 'p',
      name: 'P',
      notes: [Note(pitch: 60, startTicks: 0, lengthTicks: 1000)],
    );
    final data = smfToPatterns(
      SmfReader.parse(SmfWriter.write(patterns: [pattern])),
    );
    expect(data.patterns.single.lengthTicks, Ticks.barTicks(4));
  });

  test('malformed bytes raise SmfFormatException, not RangeError', () {
    expect(
      () => SmfReader.parse(Uint8List.fromList(List.filled(8, 0xAB))),
      throwsA(isA<SmfFormatException>()),
    );
  });
}

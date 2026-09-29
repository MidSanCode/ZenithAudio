import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/playlist.dart';
// `Pattern` is shadowed by `dart:core`'s regex `Pattern`, so the model is
// imported under a prefix here.
import 'package:zenith_audio/models/pattern.dart' as arrangement;
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/playlist_engine.dart';

/// S0 guards two promises that are easy to break silently:
///
/// 1. Older projects — notes stored as seconds, with no tick fields — still
///    load and play identically.
/// 2. The track-per-pattern arrangement migrates into Patterns + a Playlist
///    without moving a single note on the timeline.
void main() {
  group('legacy seconds-based notes', () {
    test('a note with no tick fields gains ticks from its seconds', () {
      final note = Note.fromJson({'pitch': 60, 'startTime': 1.0, 'duration': 0.5, 'velocity': 90});

      expect(note.pitch, 60);
      expect(note.velocity, 90);
      // At the default 120 bpm, one second is two beats.
      expect(note.startTicks, Ticks.fromSeconds(1.0, 120));
      expect(note.lengthTicks, Ticks.fromSeconds(0.5, 120));
    });

    test('a legacy note honours the project tempo it was saved under', () {
      final note = Note.fromJson({
        'pitch': 64,
        'startTime': 0.5,
        'duration': 0.5,
        'bpm': 60,
      });

      // At 60 bpm half a second is half a beat.
      expect(note.startTicks, Ticks.ppq ~/ 2);
      expect(note.lengthTicks, Ticks.ppq ~/ 2);
    });

    test('missing optional fields fall back instead of throwing', () {
      final note = Note.fromJson(<String, dynamic>{});

      expect(note.pitch, 60);
      expect(note.velocity, 100);
      expect(note.startTicks, 0);
      expect(note.lengthTicks, greaterThan(0));
    });

    test('explicit tick fields win over the seconds view', () {
      // The new format writes both; ticks are authoritative so a rounding
      // difference in the seconds view must not shift the note.
      final note = Note.fromJson({
        'pitch': 72,
        'startTicks': 1234,
        'lengthTicks': 567,
        'startTime': 999.0,
        'duration': 999.0,
      });

      expect(note.startTicks, 1234);
      expect(note.lengthTicks, 567);
    });

    test('a pathological zero-length note is clamped to one tick', () {
      final note = Note.fromSeconds(pitch: 60, startTime: 1.0, duration: 0.0);

      expect(note.lengthTicks, greaterThanOrEqualTo(1));
    });

    test('withTempo keeps the musical position and re-derives seconds', () {
      final at120 = Note.fromSeconds(pitch: 60, startTime: 1.0, duration: 1.0, bpm: 120);
      final at60 = at120.withTempo(60);

      // Ticks are untouched: the note did not move musically.
      expect(at60.startTicks, at120.startTicks);
      expect(at60.lengthTicks, at120.lengthTicks);
      // The wall-clock duration doubles when the tempo halves.
      expect(at60.duration, closeTo(at120.duration * 2, 1e-9));
      expect(at60.bpm, 60);
    });

    test('withTempo is a no-op when the tempo is unchanged', () {
      final note = Note.fromSeconds(pitch: 60, startTime: 1.0, duration: 1.0);
      expect(note.withTempo(120), same(note));
    });

    test('copyWith converts seconds overrides through the tempo', () {
      final note = Note(pitch: 60, startTicks: 0, lengthTicks: 960, bpm: 120);
      final moved = note.copyWith(startTime: 0.5);

      expect(moved.startTicks, Ticks.fromSeconds(0.5, 120));
      expect(moved.pitch, 60);
      expect(moved.lengthTicks, note.lengthTicks);
    });

    test('equality ignores the derived seconds view', () {
      final a = Note(pitch: 60, startTicks: 0, lengthTicks: 960, bpm: 120);
      final b = Note(pitch: 60, startTicks: 0, lengthTicks: 960, bpm: 240);

      expect(a, b);
      expect(a.hashCode, b.hashCode);
    });
  });

  group('playlist migration from tracks', () {
    Track trackWith(List<Note> notes, {String name = 'Lead'}) => Track(
          id: 'track-1',
          name: name,
          type: TrackType.instrument,
          notes: notes,
          color: const Color(0xFF40C4FF),
        );

    test('one instrument track becomes one pattern and one clip', () {
      final track = trackWith([
        Note(pitch: 60, startTicks: 0, lengthTicks: 480),
      ]);

      final result = PlaylistEngine.fromTracks([track], numerator: 4);

      expect(result.patterns, hasLength(1));
      expect(result.playlist.items, hasLength(1));
      expect(result.patterns.single.notes, hasLength(1));
      expect(result.playlist.items.single.patternId, result.patterns.single.id);
      expect(result.playlist.items.single.startTicks, 0);
    });

    test('the pattern spans whole bars, never less than one', () {
      final barTicks = Ticks.barTicks(4);

      // A single short note still yields a full bar so the block can loop.
      final short = PlaylistEngine.fromTracks(
        [trackWith([Note(pitch: 60, startTicks: 0, lengthTicks: 10)])],
        numerator: 4,
      );
      expect(short.patterns.single.lengthTicks, barTicks);

      // Content beyond one bar rounds up to the next whole bar.
      final long = PlaylistEngine.fromTracks(
        [trackWith([Note(pitch: 60, startTicks: 0, lengthTicks: barTicks + 1)])],
        numerator: 4,
      );
      expect(long.patterns.single.lengthTicks, barTicks * 2);
    });

    test('empty and audio tracks are skipped, lanes stay contiguous', () {
      final empty = Track(id: 'e', name: 'Empty', type: TrackType.instrument);
      final audio = Track(
        id: 'a',
        name: 'Audio',
        type: TrackType.audio,
        audioFilePath: 'x.wav',
      );
      final lead = trackWith([Note(pitch: 60, startTicks: 0, lengthTicks: 480)]);
      final bass = trackWith(
        [Note(pitch: 40, startTicks: 0, lengthTicks: 480)],
        name: 'Bass',
      );

      final result = PlaylistEngine.fromTracks(
        [empty, audio, lead, bass],
        numerator: 4,
      );

      expect(result.patterns, hasLength(2));
      expect(result.playlist.items, hasLength(2));
      // Lanes are assigned 0,1 — the skipped tracks must not leave a gap.
      expect(result.playlist.items.map((i) => i.trackIndex), [0, 1]);
    });

    test('flattening the migrated arrangement reproduces the original notes', () {
      final notes = [
        Note(pitch: 60, startTicks: 0, lengthTicks: 240),
        Note(pitch: 64, startTicks: 240, lengthTicks: 240),
      ];
      final result = PlaylistEngine.fromTracks(
        [trackWith(notes)],
        numerator: 4,
      );

      final flat = PlaylistEngine.flatten(result.playlist, result.patterns);

      expect(flat, hasLength(notes.length));
      // Every note keeps its exact tick position and length.
      expect(flat.map((n) => n.startTicks).toList(), notes.map((n) => n.startTicks).toList());
      expect(flat.map((n) => n.lengthTicks).toList(), notes.map((n) => n.lengthTicks).toList());
      expect(flat.map((n) => n.pitch).toList(), notes.map((n) => n.pitch).toList());
    });

    test('a project with no note tracks migrates to an empty arrangement', () {
      final result = PlaylistEngine.fromTracks([], numerator: 4);

      expect(result.patterns, isEmpty);
      expect(result.playlist.items, isEmpty);
    });

    test('migration ids are stable and derived from the track', () {
      final track = trackWith([Note(pitch: 60, startTicks: 0, lengthTicks: 480)]);

      final first = PlaylistEngine.fromTracks([track], numerator: 4);
      final second = PlaylistEngine.fromTracks([track], numerator: 4);

      expect(first.patterns.single.id, second.patterns.single.id);
      expect(first.playlist.items.single.id, second.playlist.items.single.id);
      // The pattern keeps the track's name so the timeline stays readable.
      expect(first.patterns.single.name, 'Lead');
    });

    test('callbacks can override the generated pattern identity', () {
      final track = trackWith([Note(pitch: 60, startTicks: 0, lengthTicks: 480)]);

      final result = PlaylistEngine.fromTracks(
        [track],
        numerator: 4,
        patternId: (i) => 'custom-$i',
        patternName: (i) => 'Custom $i',
      );

      expect(result.patterns.single.id, 'custom-0');
      expect(result.patterns.single.name, 'Custom 0');
      expect(result.playlist.items.single.patternId, 'custom-0');
    });
  });

  group('playlist round-trip through JSON', () {
    test('a migrated arrangement survives serialization', () {
      final track = Track(
        id: 't1',
        name: 'Lead',
        type: TrackType.instrument,
        notes: [Note(pitch: 60, startTicks: 0, lengthTicks: 480)],
        color: const Color(0xFF40C4FF),
      );
      final migrated = PlaylistEngine.fromTracks([track], numerator: 4);

      final patternBack =
          arrangement.Pattern.fromJson(migrated.patterns.single.toJson());
      final playlistBack =
          Playlist.fromJson(migrated.playlist.toJson());

      expect(patternBack.id, migrated.patterns.single.id);
      expect(patternBack.lengthTicks, migrated.patterns.single.lengthTicks);
      expect(patternBack.notes.map((n) => n.startTicks).toList(), [0]);
      expect(playlistBack.items.length, migrated.playlist.items.length);
      expect(playlistBack.items.single.patternId, patternBack.id);
    });
  });
}

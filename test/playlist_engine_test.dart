import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/pattern.dart';
import 'package:zenith_audio/models/playlist.dart';
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/playlist_engine.dart';

/// S6a: the Pattern + Playlist arrangement algebra.
///
/// Pure functions, no provider — this is where the arrangement bugs live (a
/// clone that shares notes by accident, a move off the timeline, a migration
/// that loses notes), so this is what is pinned.
void main() {
  Note n(int start, {int len = 480, int pitch = 60}) =>
      Note(pitch: pitch, startTicks: start, lengthTicks: len);

  group('cloning', () {
    test('a unique clone copies notes; edits do not affect the source', () {
      final source = Pattern(id: 'a', name: 'A', notes: [n(0), n(480)]);
      final clone = PlaylistEngine.cloneUnique(source, 'b');
      expect(clone.id, 'b');
      expect(clone.notes, source.notes);
      // The note objects are distinct, so mutating one list cannot touch the
      // other. (`Note` is immutable, so identity is the observable difference.)
      expect(identical(clone.notes.first, source.notes.first), isFalse);
    });

    test('a linked clone shares the source note list', () {
      final source = Pattern(id: 'a', name: 'A', notes: [n(0)]);
      final clone = PlaylistEngine.cloneLinked(source, 'b');
      expect(identical(clone.notes, source.notes), isTrue);
      expect(PlaylistEngine.isLinked(source, clone), isTrue);
    });
  });

  group('placement', () {
    test('addItem appends a placement', () {
      final playlist = PlaylistEngine.addItem(
        const Playlist(),
        itemId: 'i1',
        patternId: 'p1',
        startTicks: 0,
        lengthTicks: Ticks.barTicks(4),
      );
      expect(playlist.items, hasLength(1));
      expect(playlist.items.first.patternId, 'p1');
    });

    test('moveItem clamps a negative start to zero and snaps to the grid', () {
      final playlist = PlaylistEngine.addItem(
        const Playlist(),
        itemId: 'i1',
        patternId: 'p1',
        startTicks: 0,
        lengthTicks: 1000,
      );
      final moved = PlaylistEngine.moveItem(
        playlist,
        'i1',
        startTicks: 490,
        grid: 480,
      );
      expect(moved.items.first.startTicks, 480, reason: 'snapped down');
      final negative =
          PlaylistEngine.moveItem(playlist, 'i1', startTicks: -10);
      expect(negative.items.first.startTicks, 0, reason: 'clamped');
    });

    test('removeItem drops only the named placement', () {
      var playlist = PlaylistEngine.addItem(const Playlist(),
          itemId: 'i1', patternId: 'p1', startTicks: 0, lengthTicks: 100);
      playlist = PlaylistEngine.addItem(playlist,
          itemId: 'i2', patternId: 'p1', startTicks: 200, lengthTicks: 100);
      final result = PlaylistEngine.removeItem(playlist, 'i1');
      expect(result.items.map((i) => i.id).toList(), ['i2']);
    });
  });

  group('expansion', () {
    test('a block longer than the pattern repeats it', () {
      final pattern = Pattern(
        id: 'p',
        name: 'P',
        lengthTicks: 480,
        notes: [n(0, len: 480)],
      );
      final item = const PlaylistItem(
        id: 'i',
        patternId: 'p',
        startTicks: 0,
        lengthTicks: 960, // two pattern lengths
      );
      final notes = PlaylistEngine.expandItem(item, pattern);
      expect(notes, hasLength(2));
      expect(notes[0].startTicks, 0);
      expect(notes[1].startTicks, 480);
    });

    test('a block shorter than the pattern is cut', () {
      final pattern = Pattern(
        id: 'p',
        name: 'P',
        lengthTicks: 960,
        notes: [n(0, len: 480), n(480, len: 480)],
      );
      const item = PlaylistItem(
        id: 'i',
        patternId: 'p',
        startTicks: 0,
        lengthTicks: 480,
      );
      final notes = PlaylistEngine.expandItem(item, pattern);
      expect(notes, hasLength(1), reason: 'the second note starts at the cut');
    });

    test('transpose shifts pitches and clamps', () {
      final pattern = Pattern(id: 'p', name: 'P', lengthTicks: 480, notes: [
        Note(pitch: 120, startTicks: 0, lengthTicks: 480),
      ]);
      const item = PlaylistItem(
        id: 'i',
        patternId: 'p',
        startTicks: 0,
        lengthTicks: 480,
        transpose: 24,
      );
      expect(PlaylistEngine.expandItem(item, pattern).single.pitch, 127);
    });
  });

  group('legacy migration', () {
    test('fromTracks makes one pattern per note track and flattens back', () {
      final tracks = [
        Track(
          id: 't1',
          name: 'Keys',
          type: TrackType.instrument,
          notes: [n(0), n(480)],
        ),
        const Track(id: 't2', name: 'Empty', type: TrackType.instrument),
      ];
      final migration = PlaylistEngine.fromTracks(tracks, numerator: 4);
      // Only the note-bearing track becomes a pattern.
      expect(migration.patterns, hasLength(1));
      expect(migration.playlist.items, hasLength(1));

      // Flattening reproduces the original note positions exactly.
      final flat = PlaylistEngine.flattenTrack(
        migration.playlist,
        migration.patterns,
        0,
      );
      expect(flat.map((e) => e.startTicks).toList(), [0, 480]);
      expect(flat.map((e) => e.pitch).toList(), [60, 60]);
    });

    test('prunes patterns no placement references', () {
      final patterns = [
        const Pattern(id: 'used', name: 'U'),
        const Pattern(id: 'unused', name: 'X'),
      ];
      final playlist = PlaylistEngine.addItem(
        const Playlist(),
        itemId: 'i',
        patternId: 'used',
        startTicks: 0,
        lengthTicks: 100,
      );
      // With keepId, the unused pattern is retained.
      final pruned =
          PlaylistEngine.pruneUnused(patterns, playlist, keepId: 'unused');
      expect(pruned.map((p) => p.id).toSet(), {'used', 'unused'});
      // Without keepId, the unused pattern is dropped.
      final pruned2 = PlaylistEngine.pruneUnused(patterns, playlist);
      expect(pruned2.map((p) => p.id).toList(), ['used']);
    });

    test('uniqueId avoids collisions', () {
      expect(PlaylistEngine.uniqueId('pat', []), 'pat');
      expect(PlaylistEngine.uniqueId('pat', ['pat']), 'pat-1');
      expect(PlaylistEngine.uniqueId('pat', ['pat', 'pat-1']), 'pat-2');
    });
  });
}

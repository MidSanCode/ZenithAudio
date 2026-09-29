import 'musical_time.dart';
import 'note.dart';
import 'pattern.dart';
import 'playlist.dart';
import 'track.dart';

/// Thrown when an operation would create an invalid arrangement.
class PlaylistException implements Exception {
  final String message;
  const PlaylistException(this.message);
  @override
  String toString() => 'PlaylistException: $message';
}

/// Pure arrangement algebra for the Pattern + Playlist layer.
///
/// Everything here is a free function over immutable data so it can be unit
/// tested without a `ProviderContainer`, and so the same code can later drive
/// the Rust-side sequencer.
class PlaylistEngine {
  PlaylistEngine._();

  // ─────────────────────────── patterns ───────────────────────────

  /// A unique pattern id derived from [base].
  static String uniqueId(String base, Iterable<String> taken) {
    final used = taken.toSet();
    if (!used.contains(base)) return base;
    var n = 1;
    while (used.contains('$base-$n')) {
      n++;
    }
    return '$base-$n';
  }

  /// Clones [source] as a **linked** instance: the copy shares the source's
  /// notes by reference, so editing either id's notes is meaningful only once
  /// the caller decides which id the playlist should point at.
  ///
  /// Use [cloneUnique] to obtain an independent copy instead.
  static Pattern cloneLinked(Pattern source, String newId, {String? name}) {
    return Pattern(
      id: newId,
      name: name ?? source.name,
      notes: source.notes,
      lengthTicks: source.lengthTicks,
      color: source.color,
    );
  }

  /// Clones [source] as an **independent** pattern (deep-copied notes).
  static Pattern cloneUnique(Pattern source, String newId, {String? name}) {
    return Pattern(
      id: newId,
      name: name ?? '${source.name} *',
      notes: source.notes.map((n) => n.copyWith()).toList(),
      lengthTicks: source.lengthTicks,
      color: source.color,
    );
  }

  /// True when two patterns share their note list (a linked pair).
  static bool isLinked(Pattern a, Pattern b) => identical(a.notes, b.notes);

  /// Replaces one pattern in a list, preserving order.
  static List<Pattern> replace(
    List<Pattern> patterns,
    Pattern updated,
  ) =>
      patterns
          .map((p) => p.id == updated.id ? updated : p)
          .toList(growable: false);

  /// Removes patterns that no playlist item references any more.
  ///
  /// [keepId] is always retained (the pattern currently open in the editor).
  static List<Pattern> pruneUnused(
    List<Pattern> patterns,
    Playlist playlist, {
    String? keepId,
  }) {
    final used = playlist.items.map((i) => i.patternId).toSet();
    return patterns
        .where((p) => used.contains(p.id) || p.id == keepId)
        .toList(growable: false);
  }

  // ─────────────────────────── playlist ───────────────────────────

  /// Adds a placement of [patternId] at [startTicks] on [trackIndex].
  static Playlist addItem(
    Playlist playlist, {
    required String itemId,
    required String patternId,
    required int startTicks,
    required int lengthTicks,
    int trackIndex = 0,
  }) {
    final item = PlaylistItem(
      id: itemId,
      patternId: patternId,
      startTicks: startTicks,
      lengthTicks: lengthTicks,
      trackIndex: trackIndex,
    );
    return playlist.copyWith(items: [...playlist.items, item]);
  }

  /// Removes the item with [itemId].
  static Playlist removeItem(Playlist playlist, String itemId) =>
      playlist.copyWith(
        items: playlist.items.where((i) => i.id != itemId).toList(),
      );

  /// Moves the item with [itemId] to a new start / lane, snapping to [grid].
  static Playlist moveItem(
    Playlist playlist,
    String itemId, {
    required int startTicks,
    int? trackIndex,
    int grid = 0,
  }) {
    return playlist.copyWith(
      items: playlist.items.map((i) {
        if (i.id != itemId) return i;
        final snapped =
            grid > 0 ? MusicalTime.snap(startTicks, grid, round: false) : startTicks;
        return i.copyWith(
          startTicks: snapped < 0 ? 0 : snapped,
          trackIndex: trackIndex ?? i.trackIndex,
        );
      }).toList(),
    );
  }

  /// Resizes the item with [itemId] so it spans [lengthTicks] from its start.
  static Playlist resizeItem(Playlist playlist, String itemId, int lengthTicks) {
    if (lengthTicks <= 0) {
      throw const PlaylistException('item length must be positive');
    }
    return playlist.copyWith(
      items: playlist.items
          .map((i) => i.id == itemId ? i.copyWith(lengthTicks: lengthTicks) : i)
          .toList(),
    );
  }

  /// Sets the loop region. Throws when the region is empty or inverted.
  static Playlist updateLoop(
    Playlist playlist, {
    required int startTicks,
    required int endTicks,
  }) {
    if (endTicks <= startTicks) {
      throw const PlaylistException('loop end must be after loop start');
    }
    return playlist.copyWith(
      loopStartTicks: startTicks,
      loopEndTicks: endTicks,
    );
  }

  static Playlist clearLoop(Playlist playlist) =>
      playlist.copyWith(clearLoop: true);

  /// Appends a marker, keeping markers sorted by position.
  static Playlist addMarker(Playlist playlist, TimeMarker marker) {
    final markers = [...playlist.markers, marker]
      ..sort((a, b) => a.ticks.compareTo(b.ticks));
    return playlist.copyWith(markers: markers);
  }

  static Playlist removeMarker(Playlist playlist, String markerId) =>
      playlist.copyWith(
        markers: playlist.markers.where((m) => m.id != markerId).toList(),
      );

  /// The block covering [tick] on [trackIndex], if any.
  static PlaylistItem? itemAt(
    Playlist playlist,
    int tick, {
    int? trackIndex,
  }) {
    for (final i in playlist.items) {
      if (trackIndex != null && i.trackIndex != trackIndex) continue;
      if (i.containsTick(tick) || i.endTicks == tick) return i;
    }
    return null;
  }

  /// Two blocks may not overlap on the same lane.
  static bool overlaps(PlaylistItem a, PlaylistItem b) =>
      a.trackIndex == b.trackIndex &&
      a.startTicks < b.endTicks &&
      b.startTicks < a.endTicks;

  /// Finds a free start position on [trackIndex] of [lengthTicks] starting
  /// from [preferredStart], by nudging forward past any overlap.
  static int findFreeSlot(
    Playlist playlist, {
    required int trackIndex,
    required int lengthTicks,
    required int preferredStart,
  }) {
    var start = preferredStart < 0 ? 0 : preferredStart;
    final lane = playlist.itemsOnTrack(trackIndex);
    var moved = true;
    var guard = 0;
    while (moved && guard < 512) {
      moved = false;
      guard++;
      for (final i in lane) {
        final candidate = PlaylistItem(
          id: '__probe',
          patternId: i.patternId,
          startTicks: start,
          lengthTicks: lengthTicks,
          trackIndex: trackIndex,
        );
        if (overlaps(candidate, i)) {
          start = i.endTicks;
          moved = true;
        }
      }
    }
    return start;
  }

  // ─────────────────────── arrangement flattening ───────────────────────

  /// Expands one playlist item into concrete notes at absolute tick positions.
  ///
  /// A block longer than its pattern repeats the pattern to fill the block; a
  /// shorter block is cut. [item.transpose] shifts every pitch.
  static List<Note> expandItem(PlaylistItem item, Pattern pattern) {
    if (pattern.notes.isEmpty || item.lengthTicks <= 0) return const [];
    final patternLength =
        pattern.lengthTicks > 0 ? pattern.lengthTicks : pattern.contentEndTicks;
    if (patternLength <= 0) return const [];

    final out = <Note>[];
    var offset = 0;
    while (offset < item.lengthTicks) {
      for (final n in pattern.notes) {
        final localStart = offset + n.startTicks;
        if (localStart >= item.lengthTicks) continue;
        final clipped = (localStart + n.lengthTicks).clamp(0, item.lengthTicks);
        final length = clipped - localStart;
        if (length <= 0) continue;
        out.add(Note(
          pitch: (n.pitch + item.transpose).clamp(0, 127),
          startTicks: item.startTicks + localStart,
          lengthTicks: length,
          velocity: n.velocity,
        ));
      }
      offset += patternLength;
    }
    out.sort((a, b) => a.startTicks.compareTo(b.startTicks));
    return out;
  }

  /// Flattens the whole playlist for one track index.
  static List<Note> flattenTrack(
    Playlist playlist,
    List<Pattern> patterns,
    int trackIndex,
  ) {
    final byId = {for (final p in patterns) p.id: p};
    final out = <Note>[];
    for (final item in playlist.itemsOnTrack(trackIndex)) {
      final pattern = byId[item.patternId];
      if (pattern == null) continue;
      out.addAll(expandItem(item, pattern));
    }
    out.sort((a, b) => a.startTicks.compareTo(b.startTicks));
    return out;
  }

  /// Flattens the whole playlist into one note list (used for playback and
  /// for the "render arrangement" export path).
  static List<Note> flatten(Playlist playlist, List<Pattern> patterns) {
    final byId = {for (final p in patterns) p.id: p};
    final out = <Note>[];
    for (final item in playlist.items) {
      final pattern = byId[item.patternId];
      if (pattern == null) continue;
      out.addAll(expandItem(item, pattern));
    }
    out.sort((a, b) => a.startTicks.compareTo(b.startTicks));
    return out;
  }

  // ─────────────────────── legacy migration ───────────────────────

  /// Builds an equivalent Pattern + Playlist arrangement from a legacy
  /// project whose notes live directly on its tracks.
  ///
  /// Each note track becomes exactly one pattern (spanning one bar, or the
  /// content length rounded up to a whole bar) plus one playlist block that
  /// starts at tick 0, preserving the original timeline exactly.
  static ({List<Pattern> patterns, Playlist playlist}) fromTracks(
    List<Track> tracks, {
    required int numerator,
    String Function(int index)? patternId,
    String Function(int index)? patternName,
  }) {
    final patterns = <Pattern>[];
    final items = <PlaylistItem>[];
    final barTicks = Ticks.barTicks(numerator);
    var lane = 0;

    for (var i = 0; i < tracks.length; i++) {
      final t = tracks[i];
      if (!t.isInstrument || t.notes.isEmpty) {
        continue;
      }
      final id = patternId?.call(i) ?? 'pat-${t.id}';
      final contentEnd = t.notes
          .map((n) => n.endTicks)
          .reduce((a, b) => a > b ? a : b);
      // Round up to whole bars so the block loops musically.
      final bars = (contentEnd / barTicks).ceil();
      final length = (bars < 1 ? 1 : bars) * barTicks;
      patterns.add(Pattern(
        id: id,
        name: patternName?.call(i) ?? t.name,
        notes: List<Note>.from(t.notes),
        lengthTicks: length,
        color: t.color.toARGB32(),
      ));
      items.add(PlaylistItem(
        id: 'clip-${t.id}',
        patternId: id,
        startTicks: 0,
        lengthTicks: length,
        trackIndex: lane,
        name: t.name,
      ));
      lane++;
    }
    return (
      patterns: patterns,
      playlist: Playlist(items: items),
    );
  }
}

part of 'project_provider.dart';

/// Pattern + Playlist arrangement editing for [ProjectNotifier] (PLAN §3.S6a).
///
/// All the *algebra* lives in `services/playlist_engine.dart` as pure functions;
/// this file only bridges it to project state and pushes one undo entry per
/// user-visible action. Keeping the two apart is deliberate: the algebra is
/// where the bugs live (a move that loses a block, a clone that accidentally
/// shares notes), and it is fully tested without a provider.
///
/// ## Empty-arrangement laziness
///
/// A legacy project has notes directly on its tracks and an empty
/// `patterns`/`playlist`. The first arrangement edit seeds the playlist from
/// those tracks via `PlaylistEngine.fromTracks`, so the user does not have to
/// "convert" a project before arranging it.
mixin _ProjectArrangementMixin on _ProjectHistoryMixin {
  /// The bar length used when seeding or sizing patterns.
  int get _barTicks => Ticks.barTicks(state.timeSignatureNumerator);

  /// Ensures `patterns`/`playlist` exist, migrating from tracks on first use.
  ///
  /// Idempotent: a project that already has an arrangement is returned as-is.
  Project _ensureArrangement() {
    if (state.patterns.isNotEmpty || state.playlist != null) return state;
    if (state.tracks.isEmpty) {
      return state.copyWith(playlist: const Playlist());
    }
    final migrated = PlaylistEngine.fromTracks(
      state.tracks,
      numerator: state.timeSignatureNumerator,
    );
    return state.copyWith(
      patterns: migrated.patterns,
      playlist: migrated.playlist,
    );
  }

  /// Adds an empty pattern, returning its id.
  String addPattern({String? name, int? lengthTicks, int? color}) {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final id = PlaylistEngine.uniqueId(
      'pat',
      seeded.patterns.map((p) => p.id),
    );
    final pattern = Pattern(
      id: id,
      name: name ?? 'Pattern ${seeded.patterns.length + 1}',
      lengthTicks: lengthTicks ?? _barTicks,
      color: color ?? 0xFF40C4FF,
    );
    state = seeded.copyWith(patterns: [...seeded.patterns, pattern]);
    return id;
  }

  /// Removes a pattern and any playlist items that referenced it.
  void removePattern(String patternId) {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final playlist = seeded.playlist ?? const Playlist();
    state = seeded.copyWith(
      patterns: seeded.patterns
          .where((p) => p.id != patternId)
          .toList(growable: false),
      playlist: playlist.copyWith(
        items: playlist.items
            .where((i) => i.patternId != patternId)
            .toList(growable: false),
      ),
    );
  }

  /// Renames a pattern.
  void renamePattern(String patternId, String name) {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final existing =
        seeded.patterns.where((p) => p.id == patternId).firstOrNull;
    if (existing == null) return;
    state = seeded.copyWith(
      patterns:
          PlaylistEngine.replace(seeded.patterns, existing.copyWith(name: name)),
    );
  }

  /// Replaces a pattern's notes, growing its length if the notes now extend
  /// past the old end so nothing is silently cut off.
  ///
  /// This is the hook the piano roll uses once it edits a pattern-backed track.
  void updatePatternNotes(String patternId, List<Note> notes) {
    _markDirty();
    final seeded = _ensureArrangement();
    final existing =
        seeded.patterns.where((p) => p.id == patternId).firstOrNull;
    if (existing == null) return;
    final contentEnd = notes.isEmpty
        ? 0
        : notes.map((n) => n.endTicks).reduce((a, b) => a > b ? a : b);
    final length =
        contentEnd > existing.lengthTicks ? contentEnd : existing.lengthTicks;
    state = seeded.copyWith(
      patterns: PlaylistEngine.replace(
        seeded.patterns,
        existing.copyWith(notes: notes, lengthTicks: length),
      ),
    );
  }

  /// Places [patternId] on the playlist at [startTicks], returning the item id.
  String placePattern(
    String patternId, {
    required int startTicks,
    int? lengthTicks,
    int trackIndex = 0,
  }) {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final pattern =
        seeded.patterns.where((p) => p.id == patternId).firstOrNull;
    if (pattern == null) return '';
    final playlist = seeded.playlist ?? const Playlist();
    final itemId = PlaylistEngine.uniqueId(
      'item',
      playlist.items.map((i) => i.id),
    );
    state = seeded.copyWith(
      playlist: PlaylistEngine.addItem(
        playlist,
        itemId: itemId,
        patternId: patternId,
        startTicks: startTicks,
        lengthTicks: lengthTicks ?? pattern.lengthTicks,
        trackIndex: trackIndex,
      ),
    );
    return itemId;
  }

  /// Moves a playlist block, optionally snapping to [grid].
  void movePlaylistItem(
    String itemId, {
    required int startTicks,
    int? trackIndex,
    int grid = 0,
  }) {
    _markDirty();
    final seeded = _ensureArrangement();
    final playlist = seeded.playlist;
    if (playlist == null) return;
    state = seeded.copyWith(
      playlist: PlaylistEngine.moveItem(
        playlist,
        itemId,
        startTicks: startTicks,
        trackIndex: trackIndex,
        grid: grid,
      ),
    );
  }

  /// Removes a playlist block.
  void removePlaylistItem(String itemId) {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final playlist = seeded.playlist;
    if (playlist == null) return;
    state = seeded.copyWith(playlist: PlaylistEngine.removeItem(playlist, itemId));
  }

  /// Clones a pattern, **linked** (shared notes) or **unique** (independent).
  ///
  /// A link keeps every placement in sync, which is what a user wants for a
  /// repeated loop; a unique clone breaks that link to make a variant.
  String clonePattern(String patternId, {required bool linked}) {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final source =
        seeded.patterns.where((p) => p.id == patternId).firstOrNull;
    if (source == null) return '';
    final newId = PlaylistEngine.uniqueId(
      '$patternId-copy',
      seeded.patterns.map((p) => p.id),
    );
    final clone = linked
        ? PlaylistEngine.cloneLinked(source, newId)
        : PlaylistEngine.cloneUnique(source, newId);
    state = seeded.copyWith(patterns: [...seeded.patterns, clone]);
    return newId;
  }

  /// Flattens the arrangement back onto the tracks (one track lane at a time).
  ///
  /// Used when a caller only understands the legacy note-on-track shape.
  void flattenArrangement() {
    _pushUndo();
    _markDirty();
    final seeded = _ensureArrangement();
    final playlist = seeded.playlist;
    if (playlist == null) return;
    state = seeded.copyWith(
      tracks: [
        for (var i = 0; i < seeded.tracks.length; i++)
          seeded.tracks[i].copyWith(
            notes: PlaylistEngine.flattenTrack(playlist, seeded.patterns, i),
          ),
      ],
    );
  }
}

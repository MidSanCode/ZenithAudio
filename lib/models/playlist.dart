import 'musical_time.dart';

/// One placement of a [Pattern] on the playlist timeline (the second layer of
/// the arrangement).
///
/// The **same** [patternId] may appear many times: every placement plays the
/// same notes, so editing the pattern updates all of them at once (a "linked"
/// instance). Use `unique` cloning to break that link.
class PlaylistItem {
  final String id;

  /// Referenced pattern id.
  final String patternId;

  /// Placement start on the timeline, in PPQ ticks.
  final int startTicks;

  /// How much of the pattern plays (in ticks). When shorter than the pattern
  /// the tail is cut; when longer the pattern repeats to fill the block.
  final int lengthTicks;

  /// Index of the track lane this block lives on.
  final int trackIndex;

  /// Optional per-placement transpose in semitones.
  final int transpose;

  final String name;

  const PlaylistItem({
    required this.id,
    required this.patternId,
    required this.startTicks,
    required this.lengthTicks,
    this.trackIndex = 0,
    this.transpose = 0,
    this.name = '',
  });

  int get endTicks => startTicks + lengthTicks;

  bool containsTick(int tick) => tick >= startTicks && tick < endTicks;

  PlaylistItem copyWith({
    String? id,
    String? patternId,
    int? startTicks,
    int? lengthTicks,
    int? trackIndex,
    int? transpose,
    String? name,
  }) {
    return PlaylistItem(
      id: id ?? this.id,
      patternId: patternId ?? this.patternId,
      startTicks: startTicks ?? this.startTicks,
      lengthTicks: lengthTicks ?? this.lengthTicks,
      trackIndex: trackIndex ?? this.trackIndex,
      transpose: transpose ?? this.transpose,
      name: name ?? this.name,
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'pattern_id': patternId,
        'start_ticks': startTicks,
        'length_ticks': lengthTicks,
        'track_index': trackIndex,
        if (transpose != 0) 'transpose': transpose,
        if (name.isNotEmpty) 'name': name,
      };

  factory PlaylistItem.fromJson(Map<String, dynamic> json) => PlaylistItem(
        id: json['id'] as String? ?? '',
        patternId: json['pattern_id'] as String? ?? '',
        startTicks: (json['start_ticks'] as num?)?.toInt() ?? 0,
        lengthTicks: (json['length_ticks'] as num?)?.toInt() ?? Ticks.ppq * 4,
        trackIndex: (json['track_index'] as num?)?.toInt() ?? 0,
        transpose: (json['transpose'] as num?)?.toInt() ?? 0,
        name: json['name'] as String? ?? '',
      );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is PlaylistItem &&
          id == other.id &&
          patternId == other.patternId &&
          startTicks == other.startTicks &&
          lengthTicks == other.lengthTicks &&
          trackIndex == other.trackIndex &&
          transpose == other.transpose;

  @override
  int get hashCode => Object.hash(
      id, patternId, startTicks, lengthTicks, trackIndex, transpose);
}

/// The arrangement section: a list of [PlaylistItem]s, a loop region and
/// named time markers.
class Playlist {
  final List<PlaylistItem> items;

  /// Loop region in ticks. Null start = no active loop.
  final int? loopStartTicks;
  final int? loopEndTicks;

  /// Named markers shown on the ruler.
  final List<TimeMarker> markers;

  const Playlist({
    this.items = const [],
    this.loopStartTicks,
    this.loopEndTicks,
    this.markers = const [],
  });

  bool get hasLoop =>
      loopStartTicks != null &&
      loopEndTicks != null &&
      loopEndTicks! > loopStartTicks!;

  Playlist copyWith({
    List<PlaylistItem>? items,
    int? loopStartTicks,
    int? loopEndTicks,
    bool clearLoop = false,
    List<TimeMarker>? markers,
  }) {
    return Playlist(
      items: items ?? this.items,
      loopStartTicks: clearLoop ? null : (loopStartTicks ?? this.loopStartTicks),
      loopEndTicks: clearLoop ? null : (loopEndTicks ?? this.loopEndTicks),
      markers: markers ?? this.markers,
    );
  }

  /// Items placed on [trackIndex], ordered by start.
  List<PlaylistItem> itemsOnTrack(int trackIndex) {
    final list = items.where((i) => i.trackIndex == trackIndex).toList()
      ..sort((a, b) => a.startTicks.compareTo(b.startTicks));
    return list;
  }

  /// Highest track index in use (0 when empty).
  int get maxTrackIndex =>
      items.isEmpty ? 0 : items.map((i) => i.trackIndex).reduce((a, b) => a > b ? a : b);

  /// Last end position of any block.
  int get contentEndTicks => items.isEmpty
      ? 0
      : items.map((i) => i.endTicks).reduce((a, b) => a > b ? a : b);

  Map<String, dynamic> toJson() => {
        'items': items.map((i) => i.toJson()).toList(),
        if (loopStartTicks != null) 'loop_start_ticks': loopStartTicks,
        if (loopEndTicks != null) 'loop_end_ticks': loopEndTicks,
        if (markers.isNotEmpty)
          'markers': markers.map((m) => m.toJson()).toList(),
      };

  factory Playlist.fromJson(Map<String, dynamic> json) => Playlist(
        items: ((json['items'] as List<dynamic>?) ?? const [])
            .map((i) => PlaylistItem.fromJson(i as Map<String, dynamic>))
            .toList(),
        loopStartTicks: (json['loop_start_ticks'] as num?)?.toInt(),
        loopEndTicks: (json['loop_end_ticks'] as num?)?.toInt(),
        markers: ((json['markers'] as List<dynamic>?) ?? const [])
            .map((m) => TimeMarker.fromJson(m as Map<String, dynamic>))
            .toList(),
      );
}

/// A named position on the timeline.
class TimeMarker {
  final String id;
  final String name;
  final int ticks;
  final int color;

  const TimeMarker({
    required this.id,
    required this.name,
    required this.ticks,
    this.color = 0xFFFFD740,
  });

  TimeMarker copyWith({String? id, String? name, int? ticks, int? color}) =>
      TimeMarker(
        id: id ?? this.id,
        name: name ?? this.name,
        ticks: ticks ?? this.ticks,
        color: color ?? this.color,
      );

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'ticks': ticks,
        'color': '#${color.toRadixString(16).padLeft(8, '0')}',
      };

  factory TimeMarker.fromJson(Map<String, dynamic> json) => TimeMarker(
        id: json['id'] as String? ?? '',
        name: json['name'] as String? ?? '',
        ticks: (json['ticks'] as num?)?.toInt() ?? 0,
        color: _parse(json['color'] as String?),
      );

  static int _parse(String? hex) {
    if (hex == null) return 0xFFFFD740;
    try {
      return int.parse(hex.replaceFirst('#', ''), radix: 16);
    } catch (_) {
      return 0xFFFFD740;
    }
  }
}

/// A tempo / time-signature change on the conductor track.
///
/// Position 0 always carries the project's base tempo and signature; later
/// entries override them from [ticks] onward.
class TempoChange {
  final int ticks;
  final double bpm;

  /// Numerator/denominator of the time signature in force at [ticks].
  final int numerator;
  final int denominator;

  const TempoChange({
    required this.ticks,
    required this.bpm,
    this.numerator = 4,
    this.denominator = 4,
  });

  TempoChange copyWith({
    int? ticks,
    double? bpm,
    int? numerator,
    int? denominator,
  }) =>
      TempoChange(
        ticks: ticks ?? this.ticks,
        bpm: bpm ?? this.bpm,
        numerator: numerator ?? this.numerator,
        denominator: denominator ?? this.denominator,
      );

  Map<String, dynamic> toJson() => {
        'ticks': ticks,
        'bpm': bpm,
        'numerator': numerator,
        'denominator': denominator,
      };

  factory TempoChange.fromJson(Map<String, dynamic> json) => TempoChange(
        ticks: (json['ticks'] as num?)?.toInt() ?? 0,
        bpm: (json['bpm'] as num?)?.toDouble() ?? 120,
        numerator: (json['numerator'] as num?)?.toInt() ?? 4,
        denominator: (json['denominator'] as num?)?.toInt() ?? 4,
      );
}

/// A project template offered on the "new project" flow.
///
/// Templates are pure data so they can be listed without instantiating any
/// provider.
class ProjectTemplate {
  final String id;

  /// Translation key of the display name.
  final String nameKey;
  final double bpm;
  final int numerator;
  final int denominator;
  final String keySignature;

  /// Track blueprints: instrument preset id + display name + pattern bars.
  final List<ProjectTemplateTrack> tracks;

  const ProjectTemplate({
    required this.id,
    required this.nameKey,
    this.bpm = 120,
    this.numerator = 4,
    this.denominator = 4,
    this.keySignature = 'C',
    this.tracks = const [],
  });

  /// The built-in templates.
  static const List<ProjectTemplate> builtIn = [
    ProjectTemplate(
      id: 'empty',
      nameKey: 'template.empty',
      tracks: [],
    ),
    ProjectTemplate(
      id: 'pop_band',
      nameKey: 'template.popBand',
      bpm: 120,
      tracks: [
        ProjectTemplateTrack(instrumentId: 'piano', nameKey: 'template.track.keys', bars: 4),
        ProjectTemplateTrack(instrumentId: 'electric_bass', nameKey: 'template.track.bass', bars: 4),
        ProjectTemplateTrack(instrumentId: 'synth', nameKey: 'template.track.lead', bars: 4),
      ],
    ),
    ProjectTemplate(
      id: 'lofi_beat',
      nameKey: 'template.lofiBeat',
      bpm: 82,
      keySignature: 'Am',
      tracks: [
        ProjectTemplateTrack(instrumentId: 'piano', nameKey: 'template.track.chords', bars: 4),
        ProjectTemplateTrack(instrumentId: 'electric_bass', nameKey: 'template.track.bass', bars: 4),
      ],
    ),
    ProjectTemplate(
      id: 'orchestral',
      nameKey: 'template.orchestral',
      bpm: 96,
      tracks: [
        ProjectTemplateTrack(instrumentId: 'strings', nameKey: 'template.track.strings', bars: 8),
        ProjectTemplateTrack(instrumentId: 'brass', nameKey: 'template.track.brass', bars: 8),
        ProjectTemplateTrack(instrumentId: 'pad', nameKey: 'template.track.pad', bars: 8),
      ],
    ),
  ];

  static ProjectTemplate? byId(String id) {
    for (final t in builtIn) {
      if (t.id == id) return t;
    }
    return null;
  }
}

/// One track inside a [ProjectTemplate].
class ProjectTemplateTrack {
  final String instrumentId;
  final String nameKey;
  final int bars;

  const ProjectTemplateTrack({
    required this.instrumentId,
    required this.nameKey,
    this.bars = 4,
  });

  int get lengthTicks => Ticks.ppq * 4 * bars;
}

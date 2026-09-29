import 'dart:math';
import 'pattern.dart';
import 'playlist.dart';
import 'track.dart';

class Project {
  final String id;
  final String name;
  final List<Track> tracks;
  final double sampleRate;

  final int timeSignatureNumerator;
  final int timeSignatureDenominator;
  final String keySignature;
  final double bpm;
  final double playbackSpeed;

  // ── S0 optional extension slots ──
  // Populated by S6; null/empty in older projects, which keeps them readable.

  /// Patterns referenced by [playlist]. Empty for legacy projects whose notes
  /// still live directly on their tracks.
  final List<Pattern> patterns;

  /// Arrangement layer. null for legacy projects.
  final Playlist? playlist;

  const Project({
    required this.id,
    required this.name,
    this.tracks = const [],
    this.sampleRate = 44100,
    this.timeSignatureNumerator = 4,
    this.timeSignatureDenominator = 4,
    this.keySignature = 'C',
    this.bpm = 120,
    this.playbackSpeed = 1.0,
    this.patterns = const [],
    this.playlist,
  });

  /// True when the project carries the Pattern + Playlist arrangement layer.
  bool get hasArrangement => patterns.isNotEmpty || playlist != null;

  double get duration =>
      tracks.fold<double>(0, (m, t) => max(t.computedDuration, m));

  double get secondsPerBeat => 60.0 / bpm;

  double get beatDuration => secondsPerBeat;

  double get barDuration => secondsPerBeat * timeSignatureNumerator;

  Project copyWith({
    String? id,
    String? name,
    List<Track>? tracks,
    double? sampleRate,
    int? timeSignatureNumerator,
    int? timeSignatureDenominator,
    String? keySignature,
    double? bpm,
    double? playbackSpeed,
    List<Pattern>? patterns,
    Playlist? playlist,
  }) {
    return Project(
      id: id ?? this.id,
      name: name ?? this.name,
      tracks: tracks ?? this.tracks,
      sampleRate: sampleRate ?? this.sampleRate,
      timeSignatureNumerator:
          timeSignatureNumerator ?? this.timeSignatureNumerator,
      timeSignatureDenominator:
          timeSignatureDenominator ?? this.timeSignatureDenominator,
      keySignature: keySignature ?? this.keySignature,
      bpm: bpm ?? this.bpm,
      playbackSpeed: playbackSpeed ?? this.playbackSpeed,
      patterns: patterns ?? this.patterns,
      playlist: playlist ?? this.playlist,
    );
  }

  bool get hasSoloTrack => tracks.any((t) => t.isSolo);

  bool shouldTrackPlay(Track track) {
    if (hasSoloTrack) return track.isSolo;
    return !track.isMuted;
  }
}

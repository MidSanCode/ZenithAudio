import 'musical_time.dart';

/// A single musical note.
///
/// Two time representations coexist during the tick migration:
///
/// * [startTicks] / [lengthTicks] — the authoritative PPQ-tick values used by
///   the editor, the playlist and (later) the Rust sequencer.
/// * [startTime] / [duration] — seconds, kept as a **derived**, backward
///   compatible view so every existing caller keeps working unchanged.
///
/// When only seconds are supplied (legacy code, older project files) the tick
/// values are derived from the project tempo at construction time. When only
/// ticks are supplied the seconds are derived the same way.
class Note {
  final int pitch;

  /// Authoritative start position in PPQ ticks (see [Ticks.ppq]).
  final int startTicks;

  /// Authoritative length in PPQ ticks.
  final int lengthTicks;

  final int velocity;

  /// Seconds view of [startTicks]; derived from the tempo at construction.
  final double startTime;

  /// Seconds view of [lengthTicks]; derived from the tempo at construction.
  final double duration;

  /// Tempo the seconds view was derived with, so a pure `copyWith` can keep
  /// ticks and seconds consistent without knowing the project.
  final double bpm;

  Note({
    required this.pitch,
    required this.startTicks,
    required this.lengthTicks,
    this.velocity = 100,
    double? bpm,
    double? startTime,
    double? duration,
  })  : bpm = bpm ?? 120.0,
        startTime = startTime ??
            Ticks.toSeconds(startTicks, bpm ?? 120.0),
        duration = duration ??
            Ticks.toSeconds(lengthTicks, bpm ?? 120.0);

  /// Seconds-based factory kept for legacy call sites. [bpm] is required to
  /// derive the tick values; it defaults to the project reference tempo.
  factory Note.fromSeconds({
    required int pitch,
    required double startTime,
    required double duration,
    int velocity = 100,
    double bpm = 120.0,
  }) {
    final startTicks = Ticks.fromSeconds(startTime, bpm);
    final endTicks = Ticks.fromSeconds(startTime + duration, bpm);
    return Note(
      pitch: pitch,
      startTicks: startTicks,
      lengthTicks: (endTicks - startTicks).clamp(1, 1 << 30),
      velocity: velocity,
      bpm: bpm,
      startTime: startTime,
      duration: duration,
    );
  }

  /// Re-derives the seconds view for a new tempo, keeping the tick position.
  Note withTempo(double newBpm) {
    if (newBpm == bpm) return this;
    return Note(
      pitch: pitch,
      startTicks: startTicks,
      lengthTicks: lengthTicks,
      velocity: velocity,
      bpm: newBpm,
    );
  }

  int get endTicks => startTicks + lengthTicks;

  Note copyWith({
    int? pitch,
    int? startTicks,
    int? lengthTicks,
    int? velocity,
    double? bpm,
    // Seconds-based overrides are honoured for legacy call sites: passing
    // them converts through the (possibly new) tempo.
    double? startTime,
    double? duration,
  }) {
    final tempo = bpm ?? this.bpm;
    int? ticksFromSeconds;
    if (startTime != null) {
      ticksFromSeconds = Ticks.fromSeconds(startTime, tempo);
    }
    int? lenFromSeconds;
    if (duration != null) {
      final base = startTime != null ? startTime : this.startTime;
      final st = Ticks.fromSeconds(base, tempo);
      lenFromSeconds =
          (Ticks.fromSeconds(base + duration, tempo) - st).clamp(1, 1 << 30);
    }
    final newStart = startTicks ?? ticksFromSeconds ?? this.startTicks;
    final newLen = lengthTicks ?? lenFromSeconds ?? this.lengthTicks;
    return Note(
      pitch: pitch ?? this.pitch,
      startTicks: newStart,
      lengthTicks: newLen,
      velocity: velocity ?? this.velocity,
      bpm: tempo,
    );
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is Note &&
          pitch == other.pitch &&
          startTicks == other.startTicks &&
          lengthTicks == other.lengthTicks &&
          velocity == other.velocity;

  @override
  int get hashCode => Object.hash(pitch, startTicks, lengthTicks, velocity);

  /// Legacy (camelCase) shape — kept for the plugin/preview code paths.
  Map<String, dynamic> toJson() => {
        'pitch': pitch,
        'startTime': startTime,
        'duration': duration,
        'velocity': velocity,
      };

  factory Note.fromJson(Map<String, dynamic> json) {
    final bpm = (json['bpm'] as num?)?.toDouble() ?? 120.0;
    final startTicks = (json['startTicks'] as num?)?.toInt();
    final lengthTicks = (json['lengthTicks'] as num?)?.toInt();
    if (startTicks != null && lengthTicks != null) {
      return Note(
        pitch: (json['pitch'] as num?)?.toInt() ?? 60,
        startTicks: startTicks,
        lengthTicks: lengthTicks,
        velocity: (json['velocity'] as num?)?.toInt() ?? 100,
        bpm: bpm,
      );
    }
    return Note.fromSeconds(
      pitch: (json['pitch'] as num?)?.toInt() ?? 60,
      startTime: (json['startTime'] as num?)?.toDouble() ?? 0,
      duration: (json['duration'] as num?)?.toDouble() ?? 1,
      velocity: (json['velocity'] as num?)?.toInt() ?? 100,
      bpm: bpm,
    );
  }
}

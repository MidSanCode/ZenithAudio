import '../models/note.dart';
import 'musical_time.dart';

/// Where a pattern's notes and length come from.
///
/// Patterns are the **first** layer of the arrangement: a reusable block of
/// notes. The playlist references patterns by id.
class Pattern {
  final String id;
  final String name;

  /// Notes owned by this pattern. Stored on the PPQ tick grid.
  final List<Note> notes;

  /// Pattern length in ticks (the loop length used by the playlist).
  final int lengthTicks;

  /// ARGB colour used by the playlist block.
  final int color;

  const Pattern({
    required this.id,
    required this.name,
    this.notes = const [],
    this.lengthTicks = Ticks.ppq * 4,
    this.color = 0xFF40C4FF,
  });

  /// Length in bars for the current time signature.
  double bars(int numerator) => Ticks.toBars(lengthTicks, numerator);

  /// The last note end, or 0 when empty.
  int get contentEndTicks => notes.isEmpty
      ? 0
      : notes.map((n) => n.endTicks).reduce((a, b) => a > b ? a : b);

  /// A placeholder used when a playlist item points at a missing pattern.
  static const Pattern empty = Pattern(id: '', name: '');

  Pattern copyWith({
    String? id,
    String? name,
    List<Note>? notes,
    int? lengthTicks,
    int? color,
  }) {
    return Pattern(
      id: id ?? this.id,
      name: name ?? this.name,
      notes: notes ?? this.notes,
      lengthTicks: lengthTicks ?? this.lengthTicks,
      color: color ?? this.color,
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'length_ticks': lengthTicks,
        'color': '#${color.toRadixString(16).padLeft(8, '0')}',
        'notes': notes.map(_noteToJson).toList(),
      };

  static Map<String, dynamic> _noteToJson(Note n) => {
        'pitch': n.pitch,
        'start_ticks': n.startTicks,
        'length_ticks': n.lengthTicks,
        'velocity': n.velocity,
      };

  factory Pattern.fromJson(Map<String, dynamic> json) {
    return Pattern(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? 'Pattern',
      lengthTicks: (json['length_ticks'] as num?)?.toInt() ?? Ticks.ppq * 4,
      color: _parseColor(json['color'] as String?),
      notes: ((json['notes'] as List<dynamic>?) ?? const [])
          .map((n) => _noteFromJson(n as Map<String, dynamic>))
          .toList(),
    );
  }

  static Note _noteFromJson(Map<String, dynamic> n) => Note(
        pitch: (n['pitch'] as num?)?.toInt() ?? 60,
        startTicks: (n['start_ticks'] as num?)?.toInt() ?? 0,
        lengthTicks: (n['length_ticks'] as num?)?.toInt() ?? Ticks.ppq,
        velocity: (n['velocity'] as num?)?.toInt() ?? 100,
      );

  static int _parseColor(String? hex) {
    if (hex == null) return 0xFF40C4FF;
    try {
      return int.parse(hex.replaceFirst('#', ''), radix: 16);
    } catch (_) {
      return 0xFF40C4FF;
    }
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is Pattern &&
          id == other.id &&
          name == other.name &&
          lengthTicks == other.lengthTicks &&
          color == other.color;

  @override
  int get hashCode => Object.hash(id, name, lengthTicks, color);
}

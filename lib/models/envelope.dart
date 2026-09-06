import 'dart:math' as math;

/// FL-style envelope curve: a list of control points joined by shaped
/// segments. Each point carries a `curve` value (-1..1) that bends the
/// segment leaving it: < 0 rises fast then flattens (concave), > 0 rises
/// slowly then jumps (convex), 0 = linear.
class EnvelopePoint {
  /// Horizontal position, normalized 0..1 across the note duration.
  final double x;

  /// Vertical value 0..1.
  final double y;

  /// Segment curvature -1..1 (applies to the segment starting at this point).
  final double curve;

  const EnvelopePoint({required this.x, required this.y, this.curve = 0.0});

  EnvelopePoint copyWith({double? x, double? y, double? curve}) =>
      EnvelopePoint(x: x ?? this.x, y: y ?? this.y, curve: curve ?? this.curve);

  Map<String, dynamic> toJson() => {'x': x, 'y': y, 'c': curve};

  factory EnvelopePoint.fromJson(Map<String, dynamic> json) => EnvelopePoint(
        x: (json['x'] as num?)?.toDouble() ?? 0,
        y: (json['y'] as num?)?.toDouble() ?? 0,
        curve: (json['c'] as num?)?.toDouble() ?? 0,
      );
}

class EnvelopeCurve {
  /// Points sorted by x. The shape holds the last value after the final point.
  final List<EnvelopePoint> points;

  const EnvelopeCurve({required this.points});

  /// Evaluate at normalized position [u] (0..1). Out-of-range values clamp.
  double evaluate(double u) {
    if (points.isEmpty) return 1.0;
    if (u <= points.first.x) return points.first.y;
    for (int i = 0; i < points.length - 1; i++) {
      final p = points[i];
      final q = points[i + 1];
      if (u <= q.x) {
        final span = q.x - p.x;
        if (span <= 1e-9) return q.y;
        var f = (u - p.x) / span;
        final k = math.pow(4.0, p.curve).toDouble();
        f = math.pow(f, k).toDouble();
        return p.y + (q.y - p.y) * f;
      }
    }
    return points.last.y;
  }

  EnvelopeCurve copyWith({List<EnvelopePoint>? points}) =>
      EnvelopeCurve(points: points ?? this.points);

  EnvelopeCurve clone() =>
      EnvelopeCurve(points: points.map((p) => p.copyWith()).toList());

  Map<String, dynamic> toJson() =>
      {'points': points.map((p) => p.toJson()).toList()};

  factory EnvelopeCurve.fromJson(Map<String, dynamic> json) => EnvelopeCurve(
        points: ((json['points'] as List?) ?? const [])
            .map((p) => EnvelopePoint.fromJson(p as Map<String, dynamic>))
            .toList(),
      );

  // ── Presets ──

  /// Percussive exponential decay (pluck, bell).
  static EnvelopeCurve pluck({double decay = 0.7}) => EnvelopeCurve(points: [
        const EnvelopePoint(x: 0, y: 1, curve: -0.6),
        EnvelopePoint(x: decay, y: 0.02, curve: -0.4),
        const EnvelopePoint(x: 1, y: 0),
      ]);

  /// Classic pad: soft attack, full sustain, release tail.
  static EnvelopeCurve pad() => const EnvelopeCurve(points: [
        EnvelopePoint(x: 0, y: 0, curve: 0.5),
        EnvelopePoint(x: 0.25, y: 1, curve: -0.3),
        EnvelopePoint(x: 0.8, y: 0.8),
        EnvelopePoint(x: 1, y: 0, curve: 0.3),
      ]);

  /// Organ-like: instant attack, constant, hard release.
  static EnvelopeCurve organ() => const EnvelopeCurve(points: [
        EnvelopePoint(x: 0, y: 1),
        EnvelopePoint(x: 0.97, y: 1),
        EnvelopePoint(x: 1, y: 0),
      ]);

  /// Simple fade-out.
  static EnvelopeCurve fadeOut() => EnvelopeCurve(points: [
        const EnvelopePoint(x: 0, y: 1, curve: 0.2),
        const EnvelopePoint(x: 1, y: 0),
      ]);
}

import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../../models/envelope.dart';

/// Envelope editor: draggable control points on a 2D canvas.
/// Each point bends the segment that FOLLOWS it (curve wheel semantics).
class EnvelopeEditor extends StatefulWidget {
  final EnvelopeCurve curve;
  final ValueChanged<EnvelopeCurve> onChanged;
  final double height;
  final Color color;

  const EnvelopeEditor({
    super.key,
    required this.curve,
    required this.onChanged,
    this.height = 120,
    this.color = const Color(0xFF00AAFF),
  });

  @override
  State<EnvelopeEditor> createState() => _EnvelopeEditorState();
}

class _EnvelopeEditorState extends State<EnvelopeEditor> {
  int _dragIndex = -1;
  bool _draggingCurve = false;
  Offset? _lastPos;

  static const double _pointRadius = 5;

  void _emit(List<EnvelopePoint> pts) {
    pts.sort((a, b) => a.x.compareTo(b.x));
    widget.onChanged(EnvelopeCurve(points: pts));
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTapDown: _onTapDown,
      onPanStart: _onPanStart,
      onPanUpdate: _onPanUpdate,
      onPanEnd: (_) {
        _dragIndex = -1;
        _draggingCurve = false;
      },
      onDoubleTapDown: _onDoubleTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.precise,
        child: CustomPaint(
          size: Size(double.infinity, widget.height),
          painter: _EnvelopePainter(
            curve: widget.curve,
            color: widget.color,
            dragIndex: _dragIndex,
            pointRadius: _pointRadius,
          ),
        ),
      ),
    );
  }

  Offset _toLocal(EnvelopePoint p, Size size) => Offset(
        p.x * size.width,
        (1 - p.y) * size.height,
      );

  EnvelopePoint _fromLocal(Offset o, Size size, EnvelopePoint old) =>
      EnvelopePoint(
        x: (o.dx / size.width).clamp(0.0, 1.0),
        y: (1 - o.dy / size.height).clamp(0.0, 1.0),
        curve: old.curve,
      );

  int? _hitPoint(Offset local, Size size) {
    for (int i = 0; i < widget.curve.points.length; i++) {
      final p = _toLocal(widget.curve.points[i], size);
      if ((p - local).distance <= _pointRadius + 6) return i;
    }
    return null;
  }

  void _onTapDown(TapDownDetails d) {
    final size = context.size ?? Size.zero;
    final idx = _hitPoint(d.localPosition, size);
    if (idx != null) {
      setState(() => _dragIndex = idx);
    }
  }

  void _onPanStart(DragStartDetails d) {
    final size = context.size ?? Size.zero;
    final idx = _hitPoint(d.localPosition, size);
    if (idx != null) {
      setState(() {
        _dragIndex = idx;
        _draggingCurve = false;
        _lastPos = d.localPosition;
      });
    }
  }

  void _onPanUpdate(DragUpdateDetails d) {
    final size = context.size ?? Size.zero;
    if (_dragIndex >= 0 && _dragIndex < widget.curve.points.length) {
      final pts = widget.curve.points.map((p) => p.copyWith()).toList();
      var np = _fromLocal(d.localPosition, size, pts[_dragIndex]);
      // Keep first/last x pinned; clamp interior points between neighbours.
      if (_dragIndex == 0) np = EnvelopePoint(x: 0, y: np.y, curve: np.curve);
      if (_dragIndex == pts.length - 1) {
        np = EnvelopePoint(x: 1, y: np.y, curve: np.curve);
      }
      if (_dragIndex > 0) {
        np = EnvelopePoint(x: max(np.x, pts[_dragIndex - 1].x + 0.01), y: np.y, curve: np.curve);
      }
      if (_dragIndex < pts.length - 1) {
        np = EnvelopePoint(x: min(np.x, pts[_dragIndex + 1].x - 0.01), y: np.y, curve: np.curve);
      }
      pts[_dragIndex] = np;
      _emit(pts);
    }
  }

  void _onDoubleTap(TapDownDetails d) {
    final size = context.size ?? Size.zero;
    final idx = _hitPoint(d.localPosition, size);
    final pts = widget.curve.points.map((p) => p.copyWith()).toList();
    if (idx != null) {
      if (pts.length > 2) {
        pts.removeAt(idx);
        _emit(pts);
      }
      return;
    }
    // Add a new point at the tapped x, y taken from the current curve.
    final x = (d.localPosition.dx / size.width).clamp(0.0, 1.0);
    final y = widget.curve.evaluate(x);
    pts.add(EnvelopePoint(x: x, y: y));
    _emit(pts);
  }
}

class _EnvelopePainter extends CustomPainter {
  final EnvelopeCurve curve;
  final Color color;
  final int dragIndex;
  final double pointRadius;

  _EnvelopePainter({
    required this.curve,
    required this.color,
    required this.dragIndex,
    required this.pointRadius,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final bg = Paint()..color = const Color(0x14141414);
    final gridPaint = Paint()
      ..color = const Color(0x22FFFFFF)
      ..strokeWidth = 0.5;
    // grid
    for (int i = 1; i < 4; i++) {
      canvas.drawLine(Offset(size.width * i / 4, 0),
          Offset(size.width * i / 4, size.height), gridPaint);
      canvas.drawLine(Offset(0, size.height * i / 4),
          Offset(size.width, size.height * i / 4), gridPaint);
    }
    canvas.drawRect(Offset.zero & size, bg);

    final fillPath = Path()..moveTo(0, size.height);
    final linePaint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2;

    if (curve.points.isEmpty) {
      canvas.drawLine(Offset(0, size.height / 2),
          Offset(size.width, size.height / 2), linePaint);
      return;
    }

    // Sample the curve densely for smooth bent segments.
    const steps = 240;
    var first = true;
    for (int i = 0; i <= steps; i++) {
      final u = i / steps;
      final y = (1 - curve.evaluate(u)) * size.height;
      final x = u * size.width;
      if (first) {
        fillPath.moveTo(x, y);
        first = false;
      } else {
        fillPath.lineTo(x, y);
      }
    }
    fillPath.lineTo(size.width, size.height);
    fillPath.close();
    canvas.drawPath(
      fillPath,
      Paint()
        ..color = color.withAlpha(40)
        ..style = PaintingStyle.fill,
    );

    // Re-draw the outline on top of the fill.
    var outline = Path();
    for (int i = 0; i <= steps; i++) {
      final u = i / steps;
      final y = (1 - curve.evaluate(u)) * size.height;
      final x = u * size.width;
      if (i == 0) {
        outline.moveTo(x, y);
      } else {
        outline.lineTo(x, y);
      }
    }
    canvas.drawPath(outline, linePaint);

    // Control points
    for (int i = 0; i < curve.points.length; i++) {
      final p = curve.points[i];
      final c = Offset(p.x * size.width, (1 - p.y) * size.height);
      final isDrag = i == dragIndex;
      canvas.drawCircle(
        c,
        pointRadius + (isDrag ? 2 : 0),
        Paint()
          ..color = isDrag ? Colors.white : color
          ..style = PaintingStyle.fill,
      );
      canvas.drawCircle(
        c,
        pointRadius + (isDrag ? 2 : 0),
        Paint()
          ..color = Colors.black54
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1,
      );
      // curve handle indicator: small tick showing bend direction
      if (i < curve.points.length - 1 && p.curve.abs() > 0.05) {
        final q = curve.points[i + 1];
        final mid = Offset(
          (p.x + q.x) / 2 * size.width,
          (1 - curve.evaluate((p.x + q.x) / 2)) * size.height,
        );
        canvas.drawCircle(
          mid,
          2.5,
          Paint()..color = Colors.white70,
        );
      }
    }
  }

  @override
  bool shouldRepaint(covariant _EnvelopePainter old) =>
      old.curve != curve ||
      old.dragIndex != dragIndex ||
      old.color != color;
}

double min(double a, double b) => math.min(a, b);
double max(double a, double b) => math.max(a, b);

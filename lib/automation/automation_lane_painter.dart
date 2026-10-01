/// Painter and hit-testing for one automation lane's curve.
///
/// ## Why painting is separate from the widget
///
/// The editor needs to answer two questions on every frame: "draw this curve"
/// and "which point is under the cursor". Both are pure functions of the
/// points, the viewport and the value range, so they live here as plain classes
/// with no `BuildContext`. That means they are testable without pumping a
/// widget, which matters because the hit-test arithmetic is where the bugs are
/// — a four-pixel grab radius that is wrong makes dragging feel broken in a way
/// no screenshot reveals.
///
/// ## Coordinate conventions
///
/// * `frame` → x, increasing rightwards.
/// * `value` → y, **increasing upwards** (a higher gain is drawn higher), so
///   the transform negates against the canvas's downward y.
library;

import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../automation/parameter_address.dart';
import '../automation/native_types.dart';

/// The mapping between the lane's data space and its on-screen space.
@immutable
class LaneViewport {
  /// Creates a viewport.
  const LaneViewport({
    required this.startFrame,
    required this.framesPerPixel,
    required this.height,
    required this.minValue,
    required this.maxValue,
  });

  /// Frame at the left edge of the lane.
  final int startFrame;

  /// Horizontal zoom. Larger means more compressed.
  ///
  /// Stored as frames-per-pixel rather than a scale factor because every
  /// conversion is a division either way, and this form makes the zoom limit
  /// (one frame per pixel) a single lower bound instead of a bound on a
  /// reciprocal — a form that invites a divide-by-zero at the limit.
  final double framesPerPixel;

  /// Height of the lane in logical pixels.
  final double height;

  /// Value drawn at the bottom edge.
  final double minValue;

  /// Value drawn at the top edge.
  final double maxValue;

  /// Converts a frame to an x offset.
  double xForFrame(int frame) => (frame - startFrame) / framesPerPixel;

  /// Converts an x offset to the nearest frame.
  int frameForX(double x) => startFrame + (x * framesPerPixel).round();

  /// Converts a value to a y offset (measured from the top).
  double yForValue(double value) {
    final span = maxValue - minValue;
    if (span <= 0) return height / 2;
    // Negated because the canvas y axis points down while values increase up.
    final normalised = (value - minValue) / span;
    return height - normalised * height;
  }

  /// Converts a y offset (from the top) to a value.
  double valueForY(double y) {
    if (height <= 0) return minValue;
    final normalised = 1.0 - (y / height);
    return minValue + normalised * (maxValue - minValue);
  }

  /// How many frames the lane shows at its current width.
  int framesForWidth(double width) => (width * framesPerPixel).round();

  /// The frame range visible in a lane of [width] pixels.
  (int, int) visibleFrameRange(double width) =>
      (startFrame, startFrame + framesForWidth(width));

  /// Returns a copy with the given fields replaced.
  LaneViewport copyWith({
    int? startFrame,
    double? framesPerPixel,
    double? height,
    double? minValue,
    double? maxValue,
  }) =>
      LaneViewport(
        startFrame: startFrame ?? this.startFrame,
        framesPerPixel: framesPerPixel ?? this.framesPerPixel,
        height: height ?? this.height,
        minValue: minValue ?? this.minValue,
        maxValue: maxValue ?? this.maxValue,
      );
}

/// Draws automation curves and points.
class AutomationLanePainter extends CustomPainter {
  /// Creates a painter.
  AutomationLanePainter({
    required this.points,
    required this.viewport,
    required this.lineColor,
    required this.fillColor,
    required this.gridColor,
    required this.selectedIndices,
    this.valueAt,
    this.playheadFrame,
    this.playheadColor,
    this.tensionHandles = true,
  });

  /// The lane's points, in frame order.
  final List<AutomationPointV2> points;

  /// The data-to-screen mapping.
  final LaneViewport viewport;

  /// Colour of the curve stroke.
  final Color lineColor;

  /// Colour of the gradient beneath the curve.
  final Color fillColor;

  /// Colour of the value gridlines.
  final Color gridColor;

  /// Indices of points drawn as selected.
  final Set<int> selectedIndices;

  /// Optional override for evaluating the curve, used while dragging.
  ///
  /// When `null` the points' own interpolation is used. The drag preview passes
  /// a function so the curve can follow the cursor before the change is
  /// committed to the core — committing on every pointer move would mean an FFI
  /// call per frame and a re-sort per pixel.
  final double Function(int frame)? valueAt;

  /// Transport position, drawn as a vertical line when non-null.
  final int? playheadFrame;

  /// Colour of the playhead line.
  final Color? playheadColor;

  /// Whether to draw the small tension handle on shaped segments.
  final bool tensionHandles;

  /// Radius of a point's grab handle, in logical pixels.
  ///
  /// Shared with [hitTestPoint] so the drawn target and the touchable target
  /// cannot disagree — a mismatch is the classic "I can see it but I can't grab
  /// it" bug.
  static const double pointRadius = 4.0;

  @override
  void paint(Canvas canvas, Size size) {
    _paintGrid(canvas, size);
    if (points.isEmpty) return;

    final path = _buildCurvePath(size);
    _paintFill(canvas, size, path);
    _paintStroke(canvas, path);
    _paintPlayhead(canvas, size);
    _paintPoints(canvas);
  }

  /// Draws horizontal value gridlines.
  void _paintGrid(Canvas canvas, Size size) {
    final grid = Paint()
      ..color = gridColor
      ..strokeWidth = 1;
    // Five bands is enough to read the scale without competing with the curve
    // for attention.
    for (var i = 0; i <= 4; i++) {
      final y = size.height * (i / 4);
      canvas.drawLine(Offset(0, y), Offset(size.width, y), grid);
    }
  }

  /// Builds the curve path, honouring each segment's interpolation mode.
  ///
  /// Curved and exponential segments are sampled rather than drawn as beziers:
  /// the played value is what the engine computes, and sampling the same
  /// function is the only way the drawing is guaranteed to match the sound. A
  /// bezier approximation would look close and be audibly different.
  Path _buildCurvePath(Size size) {
    final path = Path();
    final first = points.first;
    path.moveTo(
      viewport.xForFrame(first.frame),
      viewport.yForValue(_valueAt(first.frame)),
    );

    for (var i = 0; i + 1 < points.length; i++) {
      final from = points[i];
      final to = points[i + 1];
      final x0 = viewport.xForFrame(from.frame);
      final x1 = viewport.xForFrame(to.frame);

      // Skip segments entirely off-screen to the left; they cannot affect
      // the visible pixels.
      if (x1 < 0) continue;

      if (from.curve == AutomationCurve.linear) {
        path.lineTo(x1, viewport.yForValue(to.value));
        continue;
      }
      if (from.curve == AutomationCurve.hold) {
        // Step: flat, then vertical.
        path.lineTo(x1, viewport.yForValue(from.value));
        path.lineTo(x1, viewport.yForValue(to.value));
        continue;
      }

      // Shaped: sample once per pixel so the drawn line has no visible facets
      // at any zoom level.
      final steps = math.max(2, (x1 - x0).abs().ceil());
      for (var step = 1; step <= steps; step++) {
        final t = step / steps;
        final frame = (from.frame + (to.frame - from.frame) * t).round();
        path.lineTo(
          viewport.xForFrame(frame),
          viewport.yForValue(_valueAt(frame)),
        );
      }
    }
    return path;
  }

  /// Evaluates the lane at [frame], preferring the drag override.
  double _valueAt(int frame) {
    final override = valueAt;
    if (override != null) return override(frame);
    return evaluatePoints(points, frame);
  }

  /// Fills the area under the curve with a gradient.
  void _paintFill(Canvas canvas, Size size, Path curve) {
    final fill = Path.from(curve)
      ..lineTo(viewport.xForFrame(points.last.frame), size.height)
      ..lineTo(viewport.xForFrame(points.first.frame), size.height)
      ..close();
    final shader = ui.Gradient.linear(
      Offset(0, 0),
      Offset(0, size.height),
      [fillColor, fillColor.withAlpha(0)],
    );
    canvas.drawPath(fill, Paint()..shader = shader);
  }

  /// Strokes the curve.
  void _paintStroke(Canvas canvas, Path path) {
    canvas.drawPath(
      path,
      Paint()
        ..color = lineColor
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.6
        ..strokeJoin = StrokeJoin.round
        ..strokeCap = StrokeCap.round,
    );
  }

  /// Draws the transport position.
  void _paintPlayhead(Canvas canvas, Size size) {
    final frame = playheadFrame;
    final color = playheadColor;
    if (frame == null || color == null) return;
    final x = viewport.xForFrame(frame);
    if (x < 0 || x > size.width) return;
    canvas.drawLine(
      Offset(x, 0),
      Offset(x, size.height),
      Paint()
        ..color = color
        ..strokeWidth = 1,
    );
  }

  /// Draws each point, enlarged when selected.
  void _paintPoints(Canvas canvas) {
    for (var i = 0; i < points.length; i++) {
      final point = points[i];
      final centre = Offset(
        viewport.xForFrame(point.frame),
        viewport.yForValue(point.value),
      );
      final selected = selectedIndices.contains(i);
      canvas.drawCircle(
        centre,
        selected ? pointRadius + 2 : pointRadius,
        Paint()..color = selected ? Colors.white : lineColor,
      );
      canvas.drawCircle(
        centre,
        selected ? pointRadius + 2 : pointRadius,
        Paint()
          ..color = lineColor
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.5,
      );

      // The tension handle sits at the segment midpoint, which is where the
      // bend is most visible, so dragging it feels like pulling the curve.
      if (tensionHandles &&
          selected &&
          i + 1 < points.length &&
          point.curve == AutomationCurve.curve) {
        final next = points[i + 1];
        final midFrame = (point.frame + next.frame) ~/ 2;
        final mid = Offset(
          viewport.xForFrame(midFrame),
          viewport.yForValue(_valueAt(midFrame)),
        );
        canvas.drawCircle(mid, 3, Paint()..color = lineColor.withAlpha(160));
      }
    }
  }

  @override
  bool shouldRepaint(covariant AutomationLanePainter old) =>
      old.points != points ||
      old.viewport != viewport ||
      old.selectedIndices != selectedIndices ||
      old.playheadFrame != playheadFrame ||
      old.lineColor != lineColor ||
      old.valueAt != valueAt;
}

/// Evaluates a sorted point list at [frame].
///
/// Mirrors `AutomationClip::value_at`: before the first point the first value
/// holds, after the last the last value holds, and `hold` segments step rather
/// than ramp. This is the Dart-side reference used for drawing and hit-testing;
/// it is deliberately a plain function over a list so it can be tested without
/// a store, a handle or a widget.
double evaluatePoints(List<AutomationPointV2> points, int frame) {
  if (points.isEmpty) return 0;
  if (points.length == 1) return points.first.value;

  final first = points.first;
  if (frame <= first.frame) return first.value;
  final last = points.last;
  if (frame >= last.frame) return last.value;

  // Binary search for the segment containing `frame`. The editor calls this per
  // pixel while painting, so a linear scan would make a long lane repaint in
  // O(points × pixels).
  var low = 0;
  var high = points.length - 1;
  while (high - low > 1) {
    final mid = (low + high) ~/ 2;
    if (points[mid].frame <= frame) {
      low = mid;
    } else {
      high = mid;
    }
  }
  return points[low].valueAt(frame, points[high]);
}

/// The result of a hit test against a lane's points.
@immutable
class LaneHit {
  /// Creates a hit.
  const LaneHit({required this.index, required this.point, this.onTensionHandle = false});

  /// Index of the hit point in the lane's list.
  final int index;

  /// The point itself.
  final AutomationPointV2 point;

  /// Whether the grab landed on the tension handle rather than the point.
  ///
  /// A single gesture has to mean two things — move the point, or bend the
  /// segment after it — so the decision is made once, at grab time, instead of
  /// being re-derived during the drag where the cursor has already moved away
  /// from where the user pressed.
  final bool onTensionHandle;

  /// Whether a point was hit at all.
  bool get isHit => index >= 0;
}

/// What is under a pointer in an automation lane.
///
/// A pure value so the widget layer can decide between "start dragging a point"
/// and "start drawing a new one" without duplicating the geometry.
abstract final class LaneHitTester {
  /// Radius within which a press grabs a point, in logical pixels.
  ///
  /// Larger than the drawn radius because a 4-pixel target is unreasonable to
  /// hit with a trackpad; 10 px matches the comfortable minimum for a
  /// touch-or-mouse control.
  static const double grabRadius = 10.0;

  /// Finds the point nearest [position], or a miss.
  ///
  /// Returns the **closest** point within the radius rather than the first,
  /// because points can overlap when zoomed out and the first match would make
  /// the wrong one move.
  static LaneHit hitTest({
    required List<AutomationPointV2> points,
    required Offset position,
    required LaneViewport viewport,
  }) {
    var bestIndex = -1;
    var bestDistance = double.infinity;
    var bestOnHandle = false;

    for (var i = 0; i < points.length; i++) {
      final point = points[i];
      final centre = Offset(
        viewport.xForFrame(point.frame),
        viewport.yForValue(point.value),
      );
      final distance = (centre - position).distance;
      if (distance <= grabRadius && distance < bestDistance) {
        bestDistance = distance;
        bestIndex = i;
        bestOnHandle = false;
      }

      // The tension handle takes priority when it is genuinely closer: it sits
      // on the curve, so a naive nearest-point test would always pick the point
      // and the handle would be unusable.
      if (i + 1 < points.length && point.curve == AutomationCurve.curve) {
        final next = points[i + 1];
        final midFrame = (point.frame + next.frame) ~/ 2;
        final handle = Offset(
          viewport.xForFrame(midFrame),
          viewport.yForValue(evaluatePoints(points, midFrame)),
        );
        final handleDistance = (handle - position).distance;
        if (handleDistance <= grabRadius && handleDistance < bestDistance) {
          bestDistance = handleDistance;
          bestIndex = i;
          bestOnHandle = true;
        }
      }
    }

    if (bestIndex < 0) {
      return const LaneHit(index: -1, point: AutomationPointV2(frame: 0, value: 0));
    }
    return LaneHit(
      index: bestIndex,
      point: points[bestIndex],
      onTensionHandle: bestOnHandle,
    );
  }

  /// Finds the index of the segment starting at or before [frame].
  ///
  /// Used when drawing a point into an existing curve: the new point inherits
  /// the curve mode of the segment it splits, so inserting into a shaped
  /// segment does not silently straighten it.
  static int segmentIndexAt(List<AutomationPointV2> points, int frame) {
    if (points.isEmpty) return -1;
    var low = 0;
    var high = points.length - 1;
    if (frame <= points.first.frame) return -1;
    if (frame >= points.last.frame) return points.length - 1;
    while (high - low > 1) {
      final mid = (low + high) ~/ 2;
      if (points[mid].frame <= frame) {
        low = mid;
      } else {
        high = mid;
      }
    }
    return low;
  }
}

/// Clamps a tension value into the legal range.
double clampTension(double tension) {
  if (tension.isNaN) return 0;
  return tension.clamp(-1.0, 1.0);
}

/// The lane metadata the editor needs to draw a header.
@immutable
class LaneHeaderInfo {
  /// Creates header info.
  const LaneHeaderInfo({
    required this.address,
    required this.label,
    required this.enabled,
    required this.armed,
    required this.pointCount,
    this.color,
  });

  /// The lane's parameter.
  final ParameterAddress address;

  /// Display name.
  final String label;

  /// Whether playback is enabled.
  final bool enabled;

  /// Whether recording is armed.
  final bool armed;

  /// Number of points.
  final int pointCount;

  /// Overriding colour, or `null` for the theme default.
  final Color? color;

  /// Builds header info from a descriptor and a lane state, or `null` when the
  /// lane does not exist yet.
  static LaneHeaderInfo? from({
    required NativeParameterDescriptor descriptor,
    required NativeLaneState? state,
  }) {
    if (state == null) return null;
    return LaneHeaderInfo(
      address: state.address,
      label: descriptor.label.isEmpty ? descriptor.key : descriptor.label,
      enabled: state.enabled,
      armed: state.armed,
      pointCount: state.pointCount,
      color: state.color == 0 ? null : Color(0xFF000000 | state.color),
    );
  }
}

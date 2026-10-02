import 'dart:math' as math;

import '../../models/musical_time.dart';
import '../../models/playlist.dart';

/// Pure geometry for the arrangement (playlist) view (PLAN §3.S6a).
///
/// ## Why the math is separate
///
/// Hit-testing a playlist block and mapping pixels to ticks are the parts that
/// actually break — a block that is one pixel too narrow is unclickable, and a
/// tick mapping that rounds the wrong way drops a drop one grid step off. None
/// of that needs a widget, so it lives here where it is unit tested, and the
/// canvas is only a thin painter over these functions.
///
/// ## Coordinate system
///
/// The view scrolls in both axes: horizontally in ticks, vertically in lanes.
/// [ArrangementGeometry] holds the two scale factors and the scroll offsets, so
/// a frame computes `offset = tickToX(tick) - scrollX` in one call.
class ArrangementGeometry {
  /// Horizontal pixels per tick.
  final double pixelsPerTick;

  /// Vertical pixels per lane (track row).
  final double laneHeight;

  /// Left offset of lane 0 in pixels.
  final double laneHeaderWidth;

  /// Horizontal scroll, in ticks.
  final double scrollTicks;

  /// Vertical scroll, in lanes.
  final double scrollLane;

  const ArrangementGeometry({
    this.pixelsPerTick = 0.05,
    this.laneHeight = 40,
    this.laneHeaderWidth = 120,
    this.scrollTicks = 0,
    this.scrollLane = 0,
  });

  ArrangementGeometry copyWith({
    double? pixelsPerTick,
    double? laneHeight,
    double? laneHeaderWidth,
    double? scrollTicks,
    double? scrollLane,
  }) =>
      ArrangementGeometry(
        pixelsPerTick: pixelsPerTick ?? this.pixelsPerTick,
        laneHeight: laneHeight ?? this.laneHeight,
        laneHeaderWidth: laneHeaderWidth ?? this.laneHeaderWidth,
        scrollTicks: scrollTicks ?? this.scrollTicks,
        scrollLane: scrollLane ?? this.scrollLane,
      );

  /// The x pixel (before scroll) of a tick.
  ///
  /// `scrollTicks` is a tick count, so it is scaled by [pixelsPerTick] here;
  /// subtracting it raw would scroll at the wrong rate whenever the horizontal
  /// zoom is not 1 px/tick.
  double tickToX(int tick) =>
      laneHeaderWidth + (tick - scrollTicks) * pixelsPerTick;

  /// The tick a viewport x pixel maps to, clamped at zero.
  int xToTick(double x) {
    final local = (x - laneHeaderWidth) / pixelsPerTick + scrollTicks;
    if (local <= 0) return 0;
    return local.round();
  }

  /// The y pixel (before scroll) of a lane index.
  double laneToY(int lane) => lane * laneHeight - scrollLane * laneHeight;

  /// The lane index a viewport y pixel maps to, clamped at zero.
  int yToLane(double y) {
    final local = y + scrollLane * laneHeight;
    if (local <= 0) return 0;
    return (local / laneHeight).floor();
  }

  /// The rectangle of a playlist item, in view coordinates.
  ({double left, double top, double width, double height}) itemRect(
    PlaylistItem item,
  ) {
    final left = tickToX(item.startTicks);
    final right = tickToX(item.endTicks);
    return (
      left: left,
      top: laneToY(item.trackIndex),
      width: math.max(1.0, right - left),
      height: laneHeight,
    );
  }

  /// The topmost item under a viewport point, or `null`.
  ///
  /// Later items win when they overlap, which matches painting order (later
  /// items are drawn on top), so a click selects what the user sees.
  PlaylistItem? hitTest(Playlist playlist, double x, double y) {
    for (final item in playlist.items.reversed) {
      final rect = itemRect(item);
      if (x >= rect.left &&
          x < rect.left + rect.width &&
          y >= rect.top &&
          y < rect.top + rect.height) {
        return item;
      }
    }
    return null;
  }
}

/// Snaps a tick to the nearest grid line, keeping the value non-negative.
int snapArrangementTick(int tick, int grid) {
  if (grid <= 0) return math.max(0, tick);
  final snapped = MusicalTime.snap(tick, grid);
  return math.max(0, snapped);
}

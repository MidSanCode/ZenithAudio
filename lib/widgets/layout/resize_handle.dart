import 'package:flutter/material.dart';

/// A thin draggable divider between workspace panels.
///
/// [axis] is the divider's long axis: [Axis.vertical] sits between
/// left|right panels (drag horizontally), [Axis.horizontal] between
/// top/bottom panels (drag vertically).
class ResizeHandle extends StatelessWidget {
  static const double thickness = 6;

  final Axis axis;
  final ValueChanged<double> onDrag;

  const ResizeHandle({super.key, required this.axis, required this.onDrag});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final lineColor = cs.outlineVariant.withAlpha(110);
    final isVertical = axis == Axis.vertical;

    return MouseRegion(
      cursor: isVertical
          ? SystemMouseCursors.resizeLeftRight
          : SystemMouseCursors.resizeUpDown,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onHorizontalDragUpdate:
            isVertical ? (d) => onDrag(d.delta.dx) : null,
        onVerticalDragUpdate:
            !isVertical ? (d) => onDrag(d.delta.dy) : null,
        child: Container(
          width: isVertical ? thickness : double.infinity,
          height: isVertical ? double.infinity : thickness,
          color: Colors.transparent,
          alignment: Alignment.center,
          child: isVertical
              ? Container(width: 2, color: lineColor)
              : Container(height: 2, color: lineColor),
        ),
      ),
    );
  }
}

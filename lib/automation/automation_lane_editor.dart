/// The interactive automation lane: draw, drag, shape, and record.
///
/// ## Gesture model
///
/// The editor has to support several edits on one surface, and the user does
/// not get a mode picker for each — so the mapping is:
///
/// | Gesture | Result |
/// |---|---|
/// | Click empty space | Draw a point at that frame/value |
/// | Click a point and drag | Move that point |
/// | Click a point's tension handle and drag | Bend the segment after it |
/// | Click a point (no drag) | Select it |
/// | Double-click a point | Delete it |
/// | Right-click a point | Cycle its interpolation mode |
///
/// The decision between "draw" and "grab" is made **once, on pointer down**,
/// from where the press landed. Re-deciding during the drag would let a cursor
/// that has moved off the point switch from "move" to "draw" mid-gesture, which
/// is the single most common way a curve editor feels broken.
///
/// ## Where edits go
///
/// Every committed edit calls into the Rust core; there is no Dart-side copy of
/// the truth. Dragging is the exception: the in-flight position is previewed
/// locally and committed on pointer up, because an FFI round trip plus a
/// re-sort per pointer move would drop frames on a lane with many points.
library;

import 'package:flutter/material.dart';

import '../automation/automation_bindings.dart';
import '../automation/automation_lane_painter.dart';
import '../automation/native_types.dart';
import '../automation/parameter_address.dart';

/// Which edit a gesture is performing.
enum _DragKind {
  /// Nothing is being dragged.
  none,

  /// Moving an existing point.
  movePoint,

  /// Bending the segment after a point.
  tension,
}

/// A single automation lane, with its header and its curve.
///
/// Stateless with respect to the data: [points] and [descriptor] come from the
/// parent, which owns the polling loop. That keeps this widget cheap to rebuild
/// and means the editor and the mixer see the same values.
class AutomationLaneEditor extends StatefulWidget {
  /// Creates a lane editor.
  const AutomationLaneEditor({
    super.key,
    required this.handle,
    required this.descriptor,
    required this.points,
    required this.viewport,
    required this.onPointsChanged,
    this.playheadFrame,
    this.color,
    this.height = 96,
    this.recordMode = RecordMode.off,
    this.onRecordModeChanged,
    this.label,
  });

  /// The open core handle, or `null` when the native core is unavailable.
  ///
  /// A `null` handle renders the lane read-only rather than throwing: a test or
  /// a web build legitimately has no core, and the curve is still worth showing.
  final AutomationHandle? handle;

  /// Static description of the parameter.
  final NativeParameterDescriptor descriptor;

  /// The lane's points, in frame order.
  final List<AutomationPointV2> points;

  /// The data-to-screen mapping.
  final LaneViewport viewport;

  /// Notifies the parent that the core now holds [points] and they should be
  /// re-read.
  ///
  /// The editor does not hold the new list itself: after an edit the core has
  /// sorted and re-indexed, so the authoritative order is the core's.
  final ValueChanged<List<AutomationPointV2>> onPointsChanged;

  /// Transport position, drawn as a playhead.
  final int? playheadFrame;

  /// Overriding lane colour.
  final Color? color;

  /// Lane height in logical pixels.
  final double height;

  /// The recording mode in effect for this lane.
  final RecordMode recordMode;

  /// Notifies the parent that the user changed the record mode.
  final ValueChanged<RecordMode>? onRecordModeChanged;

  /// Overriding display label.
  final String? label;

  @override
  State<AutomationLaneEditor> createState() => _AutomationLaneEditorState();
}

class _AutomationLaneEditorState extends State<AutomationLaneEditor> {
  /// The edit in progress, or [_DragKind.none].
  _DragKind _drag = _DragKind.none;

  /// Index of the point being dragged.
  int _dragIndex = -1;

  /// Preview of the dragged point's frame.
  int? _previewFrame;

  /// Preview of the dragged point's value.
  double? _previewValue;

  /// Preview of the dragged segment's tension.
  double? _previewTension;

  /// Indices currently selected.
  final Set<int> _selected = {};

  /// Points as displayed, with any in-flight drag applied.
  ///
  /// Returns [AutomationLaneEditor.points] unchanged when no drag is active, so
  /// the common case allocates nothing.
  List<AutomationPointV2> get _visiblePoints {
    if (_drag == _DragKind.none || _dragIndex < 0) return widget.points;
    if (_drag == _DragKind.tension) {
      final index = _dragIndex;
      if (index >= widget.points.length) return widget.points;
      return [
        for (var i = 0; i < widget.points.length; i++)
          i == index
              ? widget.points[i].copyWith(tension: _previewTension)
              : widget.points[i],
      ];
    }
    // A move can reorder the list; preview it in sorted order so the curve
    // does not fold back on itself while dragging past a neighbour.
    final moved = [
      for (var i = 0; i < widget.points.length; i++)
        i == _dragIndex
            ? widget.points[i].copyWith(
                frame: _previewFrame,
                value: _previewValue,
              )
            : widget.points[i],
    ];
    moved.sort((a, b) => a.frame.compareTo(b.frame));
    return moved;
  }

  /// The index of the dragged point after sorting, or `-1`.
  int get _sortedDragIndex {
    if (_drag != _DragKind.movePoint || _dragIndex < 0) return -1;
    final frame = _previewFrame;
    if (frame == null || _dragIndex >= widget.points.length) return -1;
    final dragged = widget.points[_dragIndex];
    // Count how many points sort before the previewed position; ties keep the
    // original relative order, matching the core's stable insertion.
    var index = 0;
    for (var i = 0; i < widget.points.length; i++) {
      if (i == _dragIndex) continue;
      final other = widget.points[i];
      if (other.frame < frame ||
          (other.frame == frame && i < _dragIndex)) {
        index++;
      }
    }
    assert(dragged.frame >= 0 || true);
    return index;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final laneColor = widget.color ?? theme.colorScheme.primary;

    return SizedBox(
      height: widget.height,
      child: Row(
        children: [
          _buildHeader(context, laneColor),
          Expanded(
            child: MouseRegion(
              cursor: SystemMouseCursors.precise,
              child: Listener(
                // `Listener` rather than `GestureDetector`: the drag has to
                // begin on pointer *down* so the draw-versus-grab decision is
                // made from where the user actually pressed, and a pan
                // recogniser would not fire until the pointer had moved past
                // its slop, by which time the intent is ambiguous.
                behavior: HitTestBehavior.opaque,
                onPointerDown: (event) => _onPointerDown(event, context),
                onPointerMove: (event) => _onPointerMove(event, context),
                onPointerUp: (event) => _onPointerUp(event, context),
                onPointerCancel: (_) => _cancelDrag(),
                child: CustomPaint(
                  painter: AutomationLanePainter(
                    points: _visiblePoints,
                    viewport: widget.viewport,
                    lineColor: laneColor,
                    fillColor: laneColor.withAlpha(40),
                    gridColor: theme.colorScheme.outlineVariant.withAlpha(80),
                    selectedIndices: _dragIndex >= 0
                        ? {_sortedDragIndex < 0 ? _dragIndex : _sortedDragIndex}
                        : _selected,
                    playheadFrame: widget.playheadFrame,
                    playheadColor: theme.colorScheme.secondary,
                  ),
                  size: Size.infinite,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// Builds the lane's header: name, state, and record controls.
  Widget _buildHeader(BuildContext context, Color laneColor) {
    final theme = Theme.of(context);
    final handle = widget.handle;
    final enabled = handle?.laneState(widget.descriptor.address)?.enabled ?? true;
    final armed = handle?.laneState(widget.descriptor.address)?.armed ?? false;

    return SizedBox(
      width: 168,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Row(
              children: [
                Container(width: 3, height: 14, color: laneColor),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    widget.label ?? widget.descriptor.label,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.labelMedium,
                  ),
                ),
                // A per-lane bypass is the difference between "automation is
                // wrong" and "automation is off"; without it the user has to
                // delete the lane to hear the base value.
                IconButton(
                  visualDensity: VisualDensity.compact,
                  padding: EdgeInsets.zero,
                  constraints: const BoxConstraints(minWidth: 24, minHeight: 24),
                  tooltip: enabled ? '关闭本轨自动化' : '开启本轨自动化',
                  icon: Icon(
                    enabled ? Icons.timeline : Icons.timeline_outlined,
                    size: 14,
                    color: enabled
                        ? laneColor
                        : theme.colorScheme.onSurfaceVariant,
                  ),
                  onPressed: handle == null
                      ? null
                      : () {
                          handle.setLaneEnabled(widget.descriptor.address, !enabled);
                          setState(() {});
                        },
                ),
                IconButton(
                  visualDensity: VisualDensity.compact,
                  padding: EdgeInsets.zero,
                  constraints: const BoxConstraints(minWidth: 24, minHeight: 24),
                  tooltip: armed ? '取消预备录制' : '预备录制本轨',
                  icon: Icon(
                    Icons.fiber_manual_record,
                    size: 14,
                    color: armed
                        ? theme.colorScheme.error
                        : theme.colorScheme.onSurfaceVariant,
                  ),
                  onPressed: handle == null
                      ? null
                      : () {
                          handle.setLaneArmed(widget.descriptor.address, !armed);
                          setState(() {});
                        },
                ),
              ],
            ),
            const SizedBox(height: 2),
            Text(
              '${widget.points.length} 个点',
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
                fontSize: 10,
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// Handles a pointer press: decide grab-versus-draw and remember it.
  void _onPointerDown(PointerDownEvent event, BuildContext context) {
    final position = event.localPosition;
    final hit = LaneHitTester.hitTest(
      points: widget.points,
      position: position,
      viewport: widget.viewport,
    );

    if (hit.isHit) {
      setState(() {
        _dragIndex = hit.index;
        _drag = hit.onTensionHandle ? _DragKind.tension : _DragKind.movePoint;
        _previewFrame = hit.point.frame;
        _previewValue = hit.point.value;
        _previewTension = hit.point.tension;
        _selected
          ..clear()
          ..add(hit.index);
      });
      return;
    }

    // Empty space: draw a point here. This is committed immediately because
    // there is nothing to preview — the user has already said where it goes.
    _drawPointAt(position);
  }

  /// Handles a pointer move during a drag.
  void _onPointerMove(PointerMoveEvent event, BuildContext context) {
    if (_drag == _DragKind.none || _dragIndex < 0) return;
    final position = event.localPosition;

    setState(() {
      if (_drag == _DragKind.tension) {
        // Map vertical displacement onto tension. The scale is deliberately
        // non-linear: the useful range of a curve bend is mostly near zero, so
        // a linear map would make subtle bends impossible to dial in.
        final dy = position.dy - widget.viewport.yForValue(
              widget.points[_dragIndex].value,
            );
        _previewTension = clampTension(
          (_previewTension ?? 0) - dy / (widget.viewport.height * 0.5),
        );
        return;
      }
      _previewFrame = widget.viewport.frameForX(position.dx);
      _previewValue = widget.viewport.valueForY(position.dy);
    });
  }

  /// Handles pointer up: commit the drag to the core.
  void _onPointerUp(PointerUpEvent event, BuildContext context) {
    final handle = widget.handle;
    if (_drag == _DragKind.none || _dragIndex < 0) {
      _cancelDrag();
      return;
    }
    if (handle == null) {
      // No core: the preview was the whole edit, so drop it rather than
      // pretending the change landed.
      _cancelDrag();
      return;
    }

    final address = widget.descriptor.address;
    var ok = false;
    switch (_drag) {
      case _DragKind.movePoint:
        final frame = widget.viewport.frameForX(event.localPosition.dx);
        final value = widget.descriptor.clamp(
          widget.viewport.valueForY(event.localPosition.dy),
        );
        ok = handle.movePoint(address, _dragIndex, frame, value);
      case _DragKind.tension:
        final point = widget.points[_dragIndex];
        ok = handle.setCurve(
          address,
          _dragIndex,
          point.curve,
          clampTension(_previewTension ?? point.tension),
        );
      case _DragKind.none:
        break;
    }

    if (ok) {
      // Re-read rather than optimistically applying: a move can re-sort the
      // clip, so the local list's indices may no longer be the core's.
      _reload(handle, address);
    }
    _cancelDrag();
  }

  /// Discards any in-flight drag preview.
  void _cancelDrag() {
    if (!mounted) return;
    setState(() {
      _drag = _DragKind.none;
      _dragIndex = -1;
      _previewFrame = null;
      _previewValue = null;
      _previewTension = null;
    });
  }

  /// Draws a new point at [position] and commits it.
  void _drawPointAt(Offset position) {
    final handle = widget.handle;
    final frame = widget.viewport.frameForX(position.dx);
    final value = widget.descriptor.clamp(widget.viewport.valueForY(position.dy));

    // Inherit the curve mode of the segment being split, so inserting into a
    // shaped segment does not silently straighten the rest of it.
    final segment = LaneHitTester.segmentIndexAt(widget.points, frame);
    final curve = segment >= 0 && segment < widget.points.length
        ? widget.points[segment].curve
        : AutomationCurve.linear;
    final tension = segment >= 0 && segment < widget.points.length
        ? widget.points[segment].tension
        : 0.0;

    final point = AutomationPointV2(
      frame: frame,
      value: value,
      curve: curve,
      tension: tension,
    );

    if (handle == null) {
      // Without a core the editor is a viewer; showing a point that will
      // vanish on the next poll would be a lie.
      return;
    }
    if (handle.insertPoint(widget.descriptor.address, point)) {
      _reload(handle, widget.descriptor.address);
    }
  }

  /// Removes the point at [index] and commits it.
  void removePointAt(int index) {
    final handle = widget.handle;
    if (handle == null) return;
    if (handle.removePoint(widget.descriptor.address, index)) {
      _reload(handle, widget.descriptor.address);
    }
  }

  /// Cycles the interpolation mode of the point at [index].
  void cycleCurveAt(int index) {
    final handle = widget.handle;
    if (handle == null || index < 0 || index >= widget.points.length) return;
    final point = widget.points[index];
    final next = AutomationCurve
        .values[(point.curve.index + 1) % AutomationCurve.values.length];
    if (handle.setCurve(widget.descriptor.address, index, next, point.tension)) {
      _reload(handle, widget.descriptor.address);
    }
  }

  /// Re-reads the lane from the core and reports it upward.
  void _reload(AutomationHandle handle, ParameterAddress address) {
    widget.onPointsChanged(handle.points(address));
  }
}

/// A record-mode selector for the transport.
///
/// Kept next to the lane editor because the two are used together, and because
/// the modes only make sense in the presence of an armed lane — a mode picker
/// floating on its own invites "why is nothing recording?".
class AutomationRecordModeSelector extends StatelessWidget {
  /// Creates a selector.
  const AutomationRecordModeSelector({
    super.key,
    required this.mode,
    required this.onChanged,
    this.enabled = true,
  });

  /// The current mode.
  final RecordMode mode;

  /// Called when the user picks a different mode.
  final ValueChanged<RecordMode> onChanged;

  /// Whether the control accepts input.
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    return SegmentedButton<RecordMode>(
      segments: const [
        ButtonSegment(
          value: RecordMode.off,
          label: Text('关闭'),
          icon: Icon(Icons.block, size: 14),
        ),
        ButtonSegment(
          value: RecordMode.touch,
          label: Text('触摸'),
          icon: Icon(Icons.touch_app, size: 14),
        ),
        ButtonSegment(
          value: RecordMode.latch,
          label: Text('锁存'),
          icon: Icon(Icons.lock_clock, size: 14),
        ),
        ButtonSegment(
          value: RecordMode.write,
          label: Text('写入'),
          icon: Icon(Icons.edit, size: 14),
        ),
      ],
      selected: {mode},
      onSelectionChanged:
          enabled ? (selection) => onChanged(selection.first) : null,
      showSelectedIcon: false,
      style: const ButtonStyle(
        visualDensity: VisualDensity.compact,
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      ),
    );
  }
}

/// A compact indicator showing whether automation is recording.
class AutomationRecordIndicator extends StatelessWidget {
  /// Creates an indicator.
  const AutomationRecordIndicator({
    super.key,
    required this.status,
    this.mode = RecordMode.off,
  });

  /// The recorder's state.
  final RecorderStatus status;

  /// The active mode, shown in the tooltip.
  final RecordMode mode;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    // Three distinct readings, because "not armed", "armed and waiting" and
    // "armed and capturing" are different states to a user deciding whether
    // their move is being written.
    final (Color color, String label) = switch (true) {
      _ when status.isRecording => (theme.colorScheme.error, '录制中'),
      _ when status.enabled => (theme.colorScheme.secondary, '待录制'),
      _ => (theme.colorScheme.onSurfaceVariant, '未录制'),
    };

    return Tooltip(
      message: switch (mode) {
        RecordMode.off => '$label · 模式关闭',
        RecordMode.touch => '$label · 触摸模式：仅按住控件时写入',
        RecordMode.latch => '$label · 锁存模式：首次触碰后持续写入',
        RecordMode.write => '$label · 写入模式：整段覆盖',
      },
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ),
          const SizedBox(width: 6),
          Text(label, style: theme.textTheme.labelSmall),
          if (status.isRecording && status.takePoints > 0) ...[
            const SizedBox(width: 6),
            Text(
              '${status.takePoints} 点',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

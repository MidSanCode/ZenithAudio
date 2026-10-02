import 'dart:ui' as ui;

import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../models/musical_time.dart';
import '../../models/pattern.dart';
import '../../models/playlist.dart';
import '../../providers/project_provider.dart';
import 'arrangement_geometry.dart';
/// The arrangement (playlist) view: Pattern + Playlist blocks over a timeline
/// (PLAN §3.S6a).
///
/// ## What it does
///
/// Shows the project's patterns as a palette and its playlist as blocks on a
/// timeline. Clicking a palette pattern places a block at the playhead; a block
/// can be dragged to move it (snapping to the current grid) or deleted. This is
/// the second layer the plan describes: a reusable pattern referenced in many
/// places, edited once.
///
/// ## Why a screen, not an inline panel
///
/// The editor already hosts the track lanes; the arrangement is a different way
/// of looking at the same music, exactly like the piano roll. Making it a route
/// keeps the two from fighting over vertical space, and matches how the piano
/// roll is opened.
class ArrangementView extends ConsumerStatefulWidget {
  const ArrangementView({super.key});

  @override
  ConsumerState<ArrangementView> createState() => _ArrangementViewState();
}

class _ArrangementViewState extends ConsumerState<ArrangementView> {
  final ArrangementGeometry _geometry = const ArrangementGeometry();

  /// The block currently being dragged, if any.
  String? _draggingId;
  int _dragOffsetTicks = 0;

  /// The grid, in ticks, that placement and dragging snap to. One beat.
  int get _grid => Ticks.ppq;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final project = ref.watch(projectProvider);
    final playlist = project.playlist ?? const Playlist();
    final patternById = {for (final p in project.patterns) p.id: p};

    return Scaffold(
      backgroundColor: cs.surface,
      appBar: AppBar(
        backgroundColor: cs.surface,
        elevation: 0,
        leading: IconButton(
          icon: const Icon(Icons.arrow_back),
          onPressed: () => Navigator.of(context).pop(),
        ),
        title: Text('arrangement.title'.tr()),
        actions: [
          IconButton(
            tooltip: 'arrangement.flatten'.tr(),
            icon: const Icon(Icons.unfold_more),
            onPressed: () =>
                ref.read(projectProvider.notifier).flattenArrangement(),
          ),
        ],
      ),
      body: Row(
        children: [
          _PatternPalette(
            patterns: project.patterns,
            onAdd: () => ref.read(projectProvider.notifier).addPattern(),
            onPlace: _placeAtPlayhead,
          ),
          VerticalDivider(width: 1, color: Theme.of(context).dividerColor),
          Expanded(
            child: LayoutBuilder(
              builder: (context, constraints) => GestureDetector(
                onPanStart: (details) =>
                    _onPanStart(details.localPosition, playlist),
                onPanUpdate: (details) =>
                    _onPanUpdate(details.localPosition, playlist),
                onPanEnd: (_) => _draggingId = null,
                child: CustomPaint(
                  painter: _ArrangementPainter(
                    playlist: playlist,
                    patternById: patternById,
                    geometry: _geometry,
                    grid: _grid,
                    draggingId: _draggingId,
                    dragOffsetTicks: _dragOffsetTicks,
                    colors: _PainterColors(
                      background: cs.surface,
                      lane: cs.surfaceContainerLow,
                      laneAlt: cs.surfaceContainer,
                      grid: cs.outlineVariant,
                      gridStrong: cs.outline,
                      playhead: cs.primary,
                      text: cs.onSurface,
                      blockBorder: cs.onSurface.withValues(alpha: 0.3),
                    ),
                    totalTicks: _totalTicks(project.tracks.length, playlist),
                  ),
                  child: const SizedBox.expand(),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// The timeline length: past the last block and the playhead, rounded to a bar.
  int _totalTicks(int laneCount, Playlist playlist) {
    var end = 0;
    for (final item in playlist.items) {
      if (item.endTicks > end) end = item.endTicks;
    }
    // Always show a handful of bars so an empty arrangement is still usable.
    final bar = Ticks.barTicks(4);
    final minimum = bar * 8;
    final rounded = ((end + bar) ~/ bar) * bar;
    return rounded < minimum ? minimum : rounded;
  }

  void _onPanStart(Offset local, Playlist playlist) {
    final item = _geometry.hitTest(playlist, local.dx, local.dy);
    _draggingId = item?.id;
    // Remember where inside the block the user grabbed, so the block does not
    // jump its start to the cursor.
    _dragOffsetTicks =
        item == null ? 0 : _geometry.xToTick(local.dx) - item.startTicks;
  }

  void _onPanUpdate(Offset local, Playlist playlist) {
    final id = _draggingId;
    if (id == null) return;
    final rawTick = _geometry.xToTick(local.dx) - _dragOffsetTicks;
    final lane = _geometry.yToLane(local.dy);
    final snapped = snapArrangementTick(rawTick, _grid);
    // Track the visual offset so the painter can preview the drag without the
    // provider round-trip lagging behind the pointer.
    setState(() => _dragOffsetTicks = _geometry.xToTick(local.dx) - rawTick);
    ref.read(projectProvider.notifier).movePlaylistItem(
          id,
          startTicks: snapped,
          trackIndex: lane < 0 ? 0 : lane,
        );
  }

  void _placeAtPlayhead(Pattern pattern) {
    // Place at tick 0 on lane 0: the screen has no playhead of its own, and 0
    // is a well-defined, non-surprising default the user can drag from.
    ref.read(projectProvider.notifier).placePattern(
          pattern.id,
          startTicks: 0,
          trackIndex: 0,
        );
  }
}

/// The left palette of patterns.
class _PatternPalette extends StatelessWidget {
  const _PatternPalette({
    required this.patterns,
    required this.onAdd,
    required this.onPlace,
  });

  final List<Pattern> patterns;
  final VoidCallback onAdd;
  final void Function(Pattern pattern) onPlace;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      width: 180,
      color: cs.surfaceContainerLow,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.all(8),
            child: Row(
              children: [
                Expanded(
                  child: Text('arrangement.patterns'.tr(),
                      style: const TextStyle(fontSize: 12, fontWeight: FontWeight.w600)),
                ),
                IconButton(
                  iconSize: 18,
                  tooltip: 'arrangement.newPattern'.tr(),
                  icon: const Icon(Icons.add),
                  onPressed: onAdd,
                ),
              ],
            ),
          ),
          Expanded(
            child: patterns.isEmpty
                ? Center(
                    child: Padding(
                      padding: const EdgeInsets.all(12),
                      child: Text(
                        'arrangement.empty'.tr(),
                        textAlign: TextAlign.center,
                        style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant),
                      ),
                    ),
                  )
                : ListView.builder(
                    itemCount: patterns.length,
                    itemBuilder: (context, i) {
                      final pattern = patterns[i];
                      return ListTile(
                        dense: true,
                        leading: Container(
                          width: 10,
                          height: 24,
                          decoration: BoxDecoration(
                            color: Color(pattern.color),
                            borderRadius: BorderRadius.circular(2),
                          ),
                        ),
                        title: Text(pattern.name, style: const TextStyle(fontSize: 12)),
                        subtitle: Text('${pattern.notes.length} notes',
                            style: const TextStyle(fontSize: 10)),
                        onTap: () => onPlace(pattern),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }
}

/// Colours the painter needs, gathered so the painter does not reach into
/// `BuildContext`.
class _PainterColors {
  final Color background;
  final Color lane;
  final Color laneAlt;
  final Color grid;
  final Color gridStrong;
  final Color playhead;
  final Color text;
  final Color blockBorder;

  const _PainterColors({
    required this.background,
    required this.lane,
    required this.laneAlt,
    required this.grid,
    required this.gridStrong,
    required this.playhead,
    required this.text,
    required this.blockBorder,
  });
}

class _ArrangementPainter extends CustomPainter {
  _ArrangementPainter({
    required this.playlist,
    required this.patternById,
    required this.geometry,
    required this.grid,
    required this.draggingId,
    required this.dragOffsetTicks,
    required this.colors,
    required this.totalTicks,
  });

  final Playlist playlist;
  final Map<String, Pattern> patternById;
  final ArrangementGeometry geometry;
  final int grid;
  final String? draggingId;
  final int dragOffsetTicks;
  final _PainterColors colors;
  final int totalTicks;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = colors.background);

    // Lane stripes.
    final laneCount = (size.height / geometry.laneHeight).ceil() + 1;
    for (var lane = 0; lane < laneCount; lane++) {
      final top = geometry.laneToY(lane);
      final rect = Rect.fromLTWH(0, top, size.width, geometry.laneHeight);
      canvas.drawRect(rect, Paint()..color = lane.isEven ? colors.lane : colors.laneAlt);
    }

    // Grid lines every `grid` ticks, stronger every bar.
    final barTicks = Ticks.barTicks(4);
    final startTick = geometry.xToTick(geometry.laneHeaderWidth);
    for (var tick = (startTick ~/ grid) * grid; ; tick += grid) {
      final x = geometry.tickToX(tick);
      if (x > size.width) break;
      if (x < geometry.laneHeaderWidth) continue;
      final isBar = tick % barTicks == 0;
      canvas.drawLine(
        Offset(x, 0),
        Offset(x, size.height),
        Paint()
          ..color = isBar ? colors.gridStrong : colors.grid
          ..strokeWidth = isBar ? 1.0 : 0.5,
      );
    }

    // Blocks.
    for (final item in playlist.items) {
      final rect = geometry.itemRect(item);
      final pattern = patternById[item.patternId];
      var left = rect.left;
      if (item.id == draggingId) {
        left += dragOffsetTicks * geometry.pixelsPerTick;
      }
      final r = Rect.fromLTWH(
        left,
        rect.top + 2,
        rect.width,
        rect.height - 4,
      );
      final base = pattern == null ? colors.grid : Color(pattern.color);
      final fill = Paint()..color = base.withValues(alpha: 0.75);
      canvas.drawRRect(
        RRect.fromRectAndRadius(r, const Radius.circular(3)),
        fill,
      );
      canvas.drawRRect(
        RRect.fromRectAndRadius(r, const Radius.circular(3)),
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1
          ..color = colors.blockBorder,
      );
      if (pattern != null) {
        _paintLabel(canvas, pattern.name, r);
      }
    }
  }

  void _paintLabel(Canvas canvas, String text, Rect rect) {
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: TextStyle(fontSize: 10, color: colors.text),
      ),
      textDirection: ui.TextDirection.ltr,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: rect.width - 8);
    painter.paint(canvas, Offset(rect.left + 4, rect.top + 4));
  }

  @override
  bool shouldRepaint(covariant _ArrangementPainter old) =>
      old.playlist != playlist ||
      old.geometry.pixelsPerTick != geometry.pixelsPerTick ||
      old.geometry.scrollTicks != geometry.scrollTicks ||
      old.geometry.scrollLane != geometry.scrollLane ||
      old.draggingId != draggingId ||
      old.dragOffsetTicks != dragOffsetTicks ||
      old.totalTicks != totalTicks;
}

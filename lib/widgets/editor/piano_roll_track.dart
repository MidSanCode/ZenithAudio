import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import '../../models/track.dart';
import '../../models/note.dart';
import 'dart:math' as math;

/// Miniature piano-roll preview for an instrument track.
///
/// Renders notes as coloured blocks inside a fixed-height area (≈80 px).
/// Tapping anywhere opens the full-screen editor.
class PianoRollTrack extends StatelessWidget {
  final Track track;
  final double pixelsPerSecond;
  final VoidCallback? onEdit;

  const PianoRollTrack({
    super.key,
    required this.track,
    this.pixelsPerSecond = 50,
    this.onEdit,
  });

  static const double _height = 80;
  static const int _minNote = 12; // C0
  static const int _maxNote = 108; // C8

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final duration = track.computedDuration > 0 ? track.computedDuration : 30.0;

    return GestureDetector(
      onDoubleTap: onEdit,
      onTap: onEdit,
      child: Container(
        height: _height,
        decoration: BoxDecoration(
          color: cs.surfaceContainerLow,
          border: Border(
            bottom: BorderSide(color: Theme.of(context).dividerColor.withAlpha(77)),
          ),
        ),
        child: Stack(
          children: [
            // --- Note preview ---
            Positioned.fill(
              child: CustomPaint(
                painter: _MiniNotePainter(
                  notes: track.notes,
                  duration: duration,
                  minNote: _minNote,
                  maxNote: _maxNote,
                  noteColor: track.color,
                  gridColor: cs.outlineVariant.withAlpha(51),
                  beatColor: cs.outlineVariant.withAlpha(102),
                  bgColor: Colors.transparent,
                  pps: pixelsPerSecond,
                ),
              ),
            ),
            // --- Overlay badge ---
            Positioned(
              top: 4,
              left: 4,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  color: cs.surfaceContainerHighest.withAlpha(179),
                  borderRadius: BorderRadius.circular(4),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(Icons.piano_outlined, size: 12, color: track.color),
                    const SizedBox(width: 4),
                    Text(
                      'pianoRoll.notesCount'.tr(namedArgs: {'n': '${track.notes.length}'}),
                      style: TextStyle(fontSize: 9, color: cs.onSurfaceVariant),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _MiniNotePainter extends CustomPainter {
  final List<Note> notes;
  final double duration;
  final int minNote;
  final int maxNote;
  final Color noteColor;
  final Color gridColor;
  final Color beatColor;
  final Color bgColor;
  final double pps;

  _MiniNotePainter({
    required this.notes,
    required this.duration,
    required this.minNote,
    required this.maxNote,
    required this.noteColor,
    required this.gridColor,
    required this.beatColor,
    required this.bgColor,
    this.pps = 50,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (duration <= 0) return;

    final noteCount = maxNote - minNote + 1;
    final rowH = size.height / noteCount;
    final scaleX = pps;

    // --- Grid lines (every 8 notes = every octave) ---
    final gridPaint = Paint()..strokeWidth = 0.5;
    for (int i = 0; i < noteCount; i++) {
      final pitch = maxNote - i;
      if (pitch % 12 == 0) {
        final y = i * rowH;
        gridPaint.color = gridColor;
        canvas.drawLine(Offset(0, y), Offset(size.width, y), gridPaint);
      }
    }

    // --- Beat markers ---
    final beatPaint = Paint()..strokeWidth = 0.5..color = beatColor;
    final drawDuration = math.min(duration, size.width / scaleX);
    for (double t = 0; t < drawDuration; t += 0.5) {
      final x = t * scaleX;
      if (x > size.width) break;
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), beatPaint);
    }

    // --- Note blocks ---
    final notePaint = Paint()..color = noteColor;
    for (final n in notes) {
      if (n.pitch < minNote || n.pitch > maxNote) continue;
      final x = n.startTime * scaleX;
      final w = math.max(n.duration * scaleX, 2.0);
      final y = (maxNote - n.pitch) * rowH;
      final h = rowH - 0.5;

      final rrect = RRect.fromRectAndRadius(
        Rect.fromLTWH(x, y + 0.5, w, h),
        const Radius.circular(1.5),
      );
      canvas.drawRRect(rrect, notePaint..color = noteColor.withAlpha(180));
      canvas.drawRRect(rrect, Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.5
        ..color = noteColor);
    }
  }

  @override
  bool shouldRepaint(covariant _MiniNotePainter oldDelegate) =>
      oldDelegate.notes != notes ||
      oldDelegate.duration != duration;
}

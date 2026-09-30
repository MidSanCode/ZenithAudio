/// Mixer level meters: dual peak + RMS bars with a peak-hold marker.
///
/// PLAN §3.S3 item 7 requires **both** a peak and an RMS meter per channel plus
/// the master, with a three-second peak hold. The two bars answer different
/// questions and neither replaces the other:
///
/// * **Peak** catches an instantaneous overload that would clip a converter.
///   It is the bar that tells you to pull the fader down.
/// * **RMS** approximates perceived loudness, so it is the bar that tells you
///   whether a mix is balanced.
///
/// A single bar cannot do both: a peak-only meter looks busy and says nothing
/// about loudness, and an RMS-only meter misses the transient that just
/// clipped.
///
/// The widget is a pure renderer. It never reads the engine and never holds
/// audio state; it draws whatever reading it is handed, and holds the marker
/// itself because the hold is a *display* duration rather than an audio one.
library;

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../mixer/mixer_model.dart';

/// A level reading for one channel.
///
/// Mirrors the engine's `ZenithMeterSnapshot`. All values are linear
/// amplitudes, where `1.0` is full scale.
class MeterReading {
  /// Creates a meter reading.
  const MeterReading({
    this.peakL = 0.0,
    this.peakR = 0.0,
    this.rmsL = 0.0,
    this.rmsR = 0.0,
    this.holdL = 0.0,
    this.holdR = 0.0,
  });

  /// Peak magnitude, left.
  final double peakL;

  /// Peak magnitude, right.
  final double peakR;

  /// RMS magnitude, left.
  final double rmsL;

  /// RMS magnitude, right.
  final double rmsR;

  /// Held peak, left.
  final double holdL;

  /// Held peak, right.
  final double holdR;

  /// Whether either channel reached full scale.
  bool get isClipping => peakL >= 1.0 || peakR >= 1.0;

  /// A silent reading.
  static const MeterReading silent = MeterReading();
}

/// The bottom of the meter's scale, in dB.
///
/// Chosen to match the fader floor so the meter and the fader agree about what
/// "as quiet as it goes" means.
const double kMeterFloorDb = -60.0;

/// Converts a linear amplitude to a `0..1` meter position.
///
/// The scale is logarithmic because a linear meter spends most of its height on
/// the top few dB, which is exactly the region a user is trying to control.
/// Anything at or below [kMeterFloorDb] maps to `0.0`.
double meterPosition(double amplitude) {
  if (amplitude.isNaN || amplitude <= 0) return 0.0;
  final db = 20 * (math.log(amplitude) / math.ln10);
  if (db <= kMeterFloorDb) return 0.0;
  final frac = (db - kMeterFloorDb) / (0 - kMeterFloorDb);
  return frac.clamp(0.0, 1.0);
}

/// Draws a stereo peak + RMS meter with a peak-hold marker.
class MixerLevelMeter extends StatefulWidget {
  /// Creates a level meter.
  const MixerLevelMeter({
    super.key,
    required this.reading,
    this.isActive = false,
    this.accent,
    this.orientation = Axis.vertical,
  });

  /// The reading to display, or `null` for silence.
  final MeterReading? reading;

  /// Whether audio is currently flowing.
  final bool isActive;

  /// Accent colour for the bars.
  final Color? accent;

  /// Whether the meter is drawn vertically (strips) or horizontally.
  final Axis orientation;

  @override
  State<MixerLevelMeter> createState() => _MixerLevelMeterState();
}

class _MixerLevelMeterState extends State<MixerLevelMeter> {
  /// Peak-hold marker positions, left and right.
  double _holdL = 0.0;
  double _holdR = 0.0;

  /// Frames remaining before each marker is allowed to fall.
  int _holdFramesL = 0;
  int _holdFramesR = 0;

  /// Roughly 60 display frames for a three-second hold.
  static const int _holdFrames = 180;

  @override
  void didUpdateWidget(MixerLevelMeter oldWidget) {
    super.didUpdateWidget(oldWidget);
    _updateHolds();
  }

  /// Advances the hold markers toward the current reading.
  ///
  /// Rendering is driven by the incoming reading rather than by a timer: the
  /// meter is repainted whenever the engine publishes, which is already at a
  /// UI-friendly rate, so a separate ticker would only add work.
  void _updateHolds() {
    final r = widget.reading;
    if (r == null) return;

    if (r.holdL >= _holdL) {
      _holdL = r.holdL;
      _holdFramesL = _holdFrames;
    } else if (_holdFramesL > 0) {
      _holdFramesL--;
    } else {
      _holdL = math.max(r.peakL, _holdL - 0.01);
    }

    if (r.holdR >= _holdR) {
      _holdR = r.holdR;
      _holdFramesR = _holdFrames;
    } else if (_holdFramesR > 0) {
      _holdFramesR--;
    } else {
      _holdR = math.max(r.peakR, _holdR - 0.01);
    }
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final r = widget.reading ?? MeterReading.silent;
    final accent = widget.accent ?? AppMeterColors.normal;

    return CustomPaint(
      painter: _MeterPainter(
        peakL: meterPosition(r.peakL),
        peakR: meterPosition(r.peakR),
        rmsL: meterPosition(r.rmsL),
        rmsR: meterPosition(r.rmsR),
        holdL: meterPosition(math.max(_holdL, r.holdL)),
        holdR: meterPosition(math.max(_holdR, r.holdR)),
        accent: accent,
        trackColor: cs.surfaceContainerHighest.withAlpha(90),
        holdColor: r.isClipping ? AppMeterColors.clip : AppMeterColors.hold,
        isActive: widget.isActive,
        vertical: widget.orientation == Axis.vertical,
      ),
      size: Size.infinite,
    );
  }
}

/// Meter colours, named so the meaning of each is explicit.
abstract final class AppMeterColors {
  /// Normal signal.
  static const Color normal = Color(0xFF27AE60);

  /// Approaching full scale.
  static const Color warn = Color(0xFFF1C40F);

  /// At or over full scale.
  static const Color clip = Color(0xFFE74C3C);

  /// The peak-hold marker.
  static const Color hold = Color(0xFFECF0F1);
}

/// Paints the meter bars.
class _MeterPainter extends CustomPainter {
  _MeterPainter({
    required this.peakL,
    required this.peakR,
    required this.rmsL,
    required this.rmsR,
    required this.holdL,
    required this.holdR,
    required this.accent,
    required this.trackColor,
    required this.holdColor,
    required this.isActive,
    required this.vertical,
  });

  final double peakL;
  final double peakR;
  final double rmsL;
  final double rmsR;
  final double holdL;
  final double holdR;
  final Color accent;
  final Color trackColor;
  final Color holdColor;
  final bool isActive;
  final bool vertical;

  @override
  void paint(Canvas canvas, Size size) {
    const gap = 1.5;
    if (vertical) {
      final barWidth = (size.width - gap) / 2;
      _paintBar(canvas, Rect.fromLTWH(0, 0, barWidth, size.height), peakL, rmsL, holdL);
      _paintBar(
        canvas,
        Rect.fromLTWH(barWidth + gap, 0, barWidth, size.height),
        peakR,
        rmsR,
        holdR,
      );
    } else {
      final barHeight = (size.height - gap) / 2;
      _paintBarH(canvas, Rect.fromLTWH(0, 0, size.width, barHeight), peakL, rmsL, holdL);
      _paintBarH(
        canvas,
        Rect.fromLTWH(0, barHeight + gap, size.width, barHeight),
        peakR,
        rmsR,
        holdR,
      );
    }
  }

  /// Paints one vertical bar: a dim track, a solid RMS core, and a peak
  /// hairline with a hold marker.
  void _paintBar(
    Canvas canvas,
    Rect rect,
    double peak,
    double rms,
    double hold,
  ) {
    // Track.
    canvas.drawRRect(
      RRect.fromRectAndRadius(rect, const Radius.circular(1)),
      Paint()..color = trackColor,
    );
    if (!isActive && peak <= 0.0 && rms <= 0.0) return;

    // RMS as the solid core: this is the "how loud is it" bar.
    final rmsHeight = rect.height * rms;
    if (rmsHeight > 0.0) {
      final rmsRect = Rect.fromLTWH(
        rect.left,
        rect.bottom - rmsHeight,
        rect.width,
        rmsHeight,
      );
      canvas.drawRect(rmsRect, Paint()..color = accent);
    }

    // Peak as a brighter hairline above the RMS core: this is the "is it
    // about to clip" indicator, and it must stay visible even when the RMS
    // core is short, so it is drawn as a distinct line rather than as a fill.
    final peakHeight = rect.height * peak;
    if (peakHeight > 0.0) {
      final y = rect.bottom - peakHeight;
      canvas.drawRect(
        Rect.fromLTWH(rect.left, y, rect.width, 1.5),
        Paint()..color = peak >= 1.0 ? AppMeterColors.clip : accent.withAlpha(220),
      );
    }

    // Hold marker: a one-pixel line that lingers after the peak falls.
    final holdHeight = rect.height * hold;
    if (holdHeight > 0.0) {
      final y = rect.bottom - holdHeight;
      canvas.drawRect(
        Rect.fromLTWH(rect.left, y, rect.width, 1.0),
        Paint()..color = holdColor,
      );
    }
  }

  /// Horizontal variant, used where a meter sits beside a label.
  void _paintBarH(
    Canvas canvas,
    Rect rect,
    double peak,
    double rms,
    double hold,
  ) {
    canvas.drawRRect(
      RRect.fromRectAndRadius(rect, const Radius.circular(1)),
      Paint()..color = trackColor,
    );
    if (!isActive && peak <= 0.0 && rms <= 0.0) return;

    final rmsWidth = rect.width * rms;
    if (rmsWidth > 0.0) {
      canvas.drawRect(
        Rect.fromLTWH(rect.left, rect.top, rmsWidth, rect.height),
        Paint()..color = accent,
      );
    }

    final peakWidth = rect.width * peak;
    if (peakWidth > 0.0) {
      canvas.drawRect(
        Rect.fromLTWH(rect.left + peakWidth - 1.5, rect.top, 1.5, rect.height),
        Paint()..color = peak >= 1.0 ? AppMeterColors.clip : accent.withAlpha(220),
      );
    }

    final holdWidth = rect.width * hold;
    if (holdWidth > 0.0) {
      canvas.drawRect(
        Rect.fromLTWH(rect.left + holdWidth - 1.0, rect.top, 1.0, rect.height),
        Paint()..color = holdColor,
      );
    }
  }

  /// Clip, peak and hold all have to force a repaint, not just the RMS fill.
  @override
  bool shouldRepaint(_MeterPainter old) =>
      old.peakL != peakL ||
      old.peakR != peakR ||
      old.rmsL != rmsL ||
      old.rmsR != rmsR ||
      old.holdL != holdL ||
      old.holdR != holdR ||
      old.accent != accent ||
      old.isActive != isActive;
}

/// Parses a linear amplitude into a display string in dB.
///
/// Used by meter tooltips and the master readout, where a number is more useful
/// than a bar.
String formatMeterDb(double amplitude) {
  if (amplitude.isNaN || amplitude <= 0) return '-∞';
  final db = 20 * (math.log(amplitude) / math.ln10);
  if (db <= kMeterFloorDb) return '-∞';
  final sign = db > 0 ? '+' : '';
  return '$sign${db.toStringAsFixed(1)}';
}

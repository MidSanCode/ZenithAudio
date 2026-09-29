import 'dart:math' as math;
import 'dart:typed_data';

// ─────────────────────────────────────────────────────────────
// Feed-forward compressor: peak envelope follower + soft-knee
// gain computer. Renders in place; used for fast transient
// control ("快速衰减") on instrument tracks.
// ─────────────────────────────────────────────────────────────
class Compressor {
  final double threshold; // 0..1 linear
  final double ratio; // 1..20
  final double attackSec;
  final double releaseSec;
  final double knee;

  double _env = 0;

  Compressor({
    this.threshold = 0.5,
    this.ratio = 4.0,
    this.attackSec = 0.005,
    this.releaseSec = 0.08,
    this.knee = 0.1,
  });

  void process(Float64List buffer, int sampleRate) {
    if (ratio <= 1.0) return;
    final aAtk = math.exp(-1.0 / (math.max(attackSec, 0.0001) * sampleRate));
    final aRel = math.exp(-1.0 / (math.max(releaseSec, 0.001) * sampleRate));
    final kneeStart = threshold - knee / 2;
    final kneeEnd = threshold + knee / 2;

    for (int i = 0; i < buffer.length; i++) {
      final x = buffer[i];
      final abs = x.abs();
      _env = abs > _env
          ? aAtk * _env + (1 - aAtk) * abs
          : aRel * _env + (1 - aRel) * abs;

      double gr = 1.0;
      if (_env > kneeEnd) {
        // Above knee: out = thr * (in/thr)^(1/ratio) — classic soft scale.
        final over = _env / threshold;
        gr = math.pow(over, 1.0 / ratio - 1.0).toDouble();
      } else if (_env > kneeStart && knee > 1e-6) {
        final t = (_env - kneeStart) / knee;
        final hardGr =
            math.pow(_env / threshold, 1.0 / ratio - 1.0).toDouble();
        gr = 1.0 + t * (hardGr - 1.0);
      }
      buffer[i] = x * gr.clamp(0.0, 1.0);
    }
  }

  /// Rescale so the peak hits 0.95 (auto makeup gain).
  static void autoMakeup(Float64List buffer) {
    double peak = 0;
    for (final s in buffer) {
      final a = s.abs();
      if (a > peak) peak = a;
    }
    if (peak > 1e-9) {
      final g = (0.95 / peak).clamp(0.0, 8.0);
      for (int i = 0; i < buffer.length; i++) {
        buffer[i] *= g;
      }
    }
  }
}

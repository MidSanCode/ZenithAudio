import 'dart:math' as math;

// ─────────────────────────────────────────────────────────────
// State-variable filter (Chamberlin two-integrator loop) — unity DC
// gain, simple and stable for musical cutoff ranges (clamped to sr/8).
// Used by the subtractive synth.
// ─────────────────────────────────────────────────────────────
enum FilterType { lowPass, highPass, bandPass, notch }

class SvfFilter {
  double _low = 0, _band = 0;
  double _f1 = 0, _d = 1.0;

  final FilterType type;
  SvfFilter(this.type);

  void setCutoff(double cutoffHz, double resonance, int sampleRate) {
    final fc = cutoffHz.clamp(10.0, sampleRate / 8);
    _f1 = 2.0 * math.sin(math.pi * fc / sampleRate);
    _d = 1.0 / resonance.clamp(0.3, 20.0); // damping = 1/Q
  }

  double process(double x) {
    final high = x - _low - _d * _band;
    _band += _f1 * high;
    _low += _f1 * _band;
    switch (type) {
      case FilterType.lowPass:
        return _low;
      case FilterType.highPass:
        return high;
      case FilterType.bandPass:
        return _band;
      case FilterType.notch:
        return x - _d * _band;
    }
  }

  void reset() {
    _low = 0;
    _band = 0;
  }
}

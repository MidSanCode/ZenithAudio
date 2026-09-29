import 'dart:math' as math;
import 'dart:typed_data';

// ─────────────────────────────────────────────────────────────
// Wavetables
// ─────────────────────────────────────────────────────────────
class WavTable {
  static const int size = 2048;
  final Float64List data;
  WavTable(this.data);

  factory WavTable.sine() =>
      WavTable(_fromFn((ph) => math.sin(2 * math.pi * ph)));

  factory WavTable.saw() => WavTable(_fromFn((ph) => 2.0 * ph - 1.0));

  factory WavTable.square() => WavTable(_fromFn((ph) => ph < 0.5 ? 1.0 : -1.0));

  factory WavTable.triangle() =>
      WavTable(_fromFn((ph) => ph < 0.5 ? 4.0 * ph - 1.0 : 3.0 - 4.0 * ph));

  factory WavTable.fromHarmonics(List<double> harmonics) {
    return WavTable(_fromFn((ph) {
      var s = 0.0;
      for (int h = 0; h < harmonics.length; h++) {
        s += math.sin(2 * math.pi * ph * (h + 1)) * harmonics[h];
      }
      return s;
    }));
  }

  static Float64List _fromFn(double Function(double) fn) {
    final d = Float64List(size);
    for (int i = 0; i < size; i++) {
      d[i] = fn(i / size);
    }
    double peak = 0;
    for (final v in d) {
      final a = v.abs();
      if (a > peak) peak = a;
    }
    if (peak > 0) {
      for (int i = 0; i < size; i++) {
        d[i] = d[i] / peak * 0.9;
      }
    }
    return d;
  }

  double sample(double phase) {
    var p = phase - phase.floorToDouble();
    final x = p * size;
    final i = x.toInt();
    final f = x - i;
    final a = data[i % size];
    final b = data[(i + 1) % size];
    return a + (b - a) * f;
  }

  /// Read across a frame list at morph position 0..1.
  static double sampleFrames(List<WavTable> frames, double pos, double phase) {
    if (frames.isEmpty) return 0;
    if (frames.length == 1) return frames.first.sample(phase);
    final x = pos.clamp(0.0, 1.0) * (frames.length - 1);
    final i = x.toInt();
    final f = x - i;
    final a = frames[i].sample(phase);
    final b = frames[i + 1 > frames.length - 1 ? frames.length - 1 : i + 1]
        .sample(phase);
    return a + (b - a) * f;
  }
}

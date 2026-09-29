/// Shared sample-rate context passed through the render pipeline.
class RenderContext {
  final int sampleRate;

  const RenderContext({this.sampleRate = 44100});
}

/// Synth engines.
enum SynthEngine { subtractive, wavetable, fm, sample, granular, additive }

SynthEngine engineFromString(String? s) {
  switch (s) {
    case 'subtractive':
      return SynthEngine.subtractive;
    case 'wavetable':
      return SynthEngine.wavetable;
    case 'fm':
      return SynthEngine.fm;
    case 'sample':
      return SynthEngine.sample;
    case 'granular':
      return SynthEngine.granular;
    case 'additive':
    case null:
      return SynthEngine.additive;
    default:
      return SynthEngine.additive;
  }
}

/// xorshift32 noise.
class NoiseGen {
  int _state = 2463534242;
  double next() {
    _state ^= _state << 13;
    _state &= 0xFFFFFFFF;
    _state ^= _state >> 17;
    _state ^= _state << 5;
    _state &= 0xFFFFFFFF;
    return (_state / 0x7FFFFFFF) - 1.0;
  }
}

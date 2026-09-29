import 'dart:math' as math;
import 'dart:typed_data';

import '../../models/instrument.dart';
import 'dsp/compressor.dart';
import 'dsp/svf_filter.dart';
import 'dsp/synth_common.dart';
import 'dsp/wavetable.dart';
import 'soundfont_parser.dart';

/// One voice instance per note (holds per-note DSP state).
class SynthVoice {
  final InstrumentPreset inst;
  final RenderContext ctx;
  final SoundFontBank? bank;
  final SynthEngine engine;
  final NoiseGen _noise = NoiseGen();

  SvfFilter? _filter;
  List<WavTable>? _tables;

  // sample engine
  SoundFontPreset? _sfPreset;
  SoundFontSample? _sfSample;
  double _samplePhase = 0;
  bool _sfDone = false;

  // granular state
  double _grainPhase = 0;
  int _grainCount = 0;
  double _grainDetune = 1.0;

  SynthVoice(this.inst, this.ctx, {this.bank})
      : engine = engineFromString(inst.synthEngine) {
    if (engine == SynthEngine.subtractive) {
      _filter = SvfFilter(_filterTypeFromString(inst.filterType));
    }
    if (engine == SynthEngine.wavetable) {
      _tables = _buildTables(inst);
    }
    if (engine == SynthEngine.sample) {
      final b = bank;
      if (b != null) {
        final preset = b.findPreset(inst.programNumber);
        if (preset != null && preset.samples.isNotEmpty) {
          _sfPreset = preset;
        }
      }
    }
  }

  static FilterType _filterTypeFromString(String? s) {
    switch (s) {
      case 'highPass':
        return FilterType.highPass;
      case 'bandPass':
        return FilterType.bandPass;
      case 'notch':
        return FilterType.notch;
      case 'lowPass':
      case null:
      default:
        return FilterType.lowPass;
    }
  }

  static List<WavTable> _buildTables(InstrumentPreset inst) {
    return [
      if (inst.harmonics.isNotEmpty) WavTable.fromHarmonics(inst.harmonics),
      WavTable.sine(),
      WavTable.triangle(),
      WavTable.saw(),
      WavTable.square(),
    ];
  }

  /// Reset per-note state (called before each note render when a voice is
  /// reused across notes).
  void reset() {
    _samplePhase = 0;
    _sfDone = false;
    _grainPhase = 0;
    _grainCount = 0;
    _grainDetune = 1.0;
    _fallbackTime = 0;
    _filter?.reset();
  }

  /// Render the whole note (+ tail) and return the samples.
  Float64List render({
    required int numSamples,
    required int pitch,
    required int velocity,
    required double noteDuration,
    required double releaseSec,
  }) {
    final out = Float64List(numSamples);
    if (numSamples <= 0) return out;
    reset();

    final freq = 440.0 * math.pow(2, (pitch - 69) / 12).toDouble();
    final sr = ctx.sampleRate;
    final vel = velocity / 127.0;
    final envCurve = inst.envCurve;
    final useEnv = envCurve != null && envCurve.points.isNotEmpty;

    // Resolve the SF2 sample for this pitch (key-range zones).
    if (engine == SynthEngine.sample && _sfPreset != null) {
      _sfSample = bank?.sampleForPitch(_sfPreset!, pitch);
    }

    double phase = 0;
    final detuneRatio = math.pow(2, inst.detuneCents / 1200).toDouble();

    for (int i = 0; i < numSamples; i++) {
      final t = i / sr;
      final double amp;
      if (useEnv) {
        final u = (t / noteDuration).clamp(0.0, 1.0);
        var g = envCurve!.evaluate(u);
        if (t > noteDuration) {
          final rt = ((t - noteDuration) / releaseSec).clamp(0.0, 1.0);
          g *= 1.0 - rt;
        }
        amp = g * (velocity / 100.0);
      } else {
        amp = inst.getEnvelope(t, noteDuration, velocity);
      }

      double sample;
      switch (engine) {
        case SynthEngine.subtractive:
          final saw = 2.0 * phase - 1.0;
          var s = saw;
          if (inst.detuneCents > 0) {
            final p2 = (phase * detuneRatio) % 1.0;
            s += (2.0 * p2 - 1.0) * 0.5;
          }
          if (inst.noiseAttack > 0) {
            s += _noise.next() * inst.noiseAttack * math.exp(-t * 60);
          }
          _filter!.setCutoff(
              (inst.filterCutoff * (1.0 + _filterEnv(t, vel) * inst.filterEnvAmount))
                  .clamp(30.0, sr / 2.2),
              inst.filterResonance,
              sr);
          sample = _filter!.process(s * 0.5);
          break;
        case SynthEngine.wavetable:
          final morphPos =
              (inst.morphRate > 0 ? ((t / inst.morphRate) % 1.0) : 0.0);
          final s1 = WavTable.sampleFrames(_tables!, morphPos, phase);
          double s = s1;
          if (inst.detuneCents > 0) {
            final p2 = (phase * detuneRatio) % 1.0;
            s = s1 * 0.7 +
                WavTable.sampleFrames(_tables!, morphPos, p2) * 0.3;
          }
          sample = s * 0.9;
          break;
        case SynthEngine.fm:
          // 2-op FM with decaying index (bell/EP character).
          final index =
              inst.fmIndex * math.exp(-t / math.max(inst.fmDecay, 0.01)) +
                  inst.fmIndex * inst.fmFeedback * 0.1;
          final mod = math.sin(2 * math.pi * phase * inst.fmRatio) * index;
          sample = math.sin(2 * math.pi * phase + mod) * 0.8;
          break;
        case SynthEngine.sample:
          sample = _sampleOsc(freq, sr);
          break;
        case SynthEngine.granular:
          sample = _granularOsc(t, freq, sr);
          break;
        case SynthEngine.additive:
          sample = inst.synthSample(t, freq, velocity);
          break;
      }

      out[i] = sample * amp;

      phase += freq / sr;
      if (phase >= 1.0) phase -= 1.0;
    }

    if (engine == SynthEngine.sample) {
      Compressor.autoMakeup(out);
    }
    return out;
  }

  double _filterEnv(double t, double vel) {
    final atk = inst.filterAttack <= 0 ? 0.001 : inst.filterAttack;
    if (t < atk) return t / atk;
    final dc = inst.filterDecay <= 0 ? 0.2 : inst.filterDecay;
    return 1.0 - (1.0 - inst.filterSustain) * ((t - atk) / dc).clamp(0.0, 1.0);
  }

  double _fallbackTime = 0;

  double _sampleOsc(double freq, int sr) {
    final sample = _sfSample;
    if (sample == null) {
      // Fallback: additive render when no SoundFont bank is loaded.
      final t = _fallbackTime;
      _fallbackTime += 1.0 / sr;
      return inst.synthSample(t, freq, 100);
    }
    if (_sfDone) return 0; // non-looped one-shot finished → silence
    final baseFreq = sample.baseFreq <= 0 ? 440.0 : sample.baseFreq;
    _samplePhase += (freq / baseFreq) * (sample.sampleRate / sr);
    if (sample.loopMode != 0 && sample.loopEnd > sample.loopStart) {
      if (_samplePhase >= sample.loopEnd) {
        _samplePhase = sample.loopStart +
            (_samplePhase - sample.loopEnd) %
                (sample.loopEnd - sample.loopStart);
      }
    } else if (_samplePhase >= sample.data.length - 1) {
      _sfDone = true;
      return 0;
    }
    final i = _samplePhase.toInt();
    final f = _samplePhase - i;
    final a = sample.data[i];
    final b = sample.data[i + 1 < sample.data.length ? i + 1 : i];
    return a + (b - a) * f;
  }

  double _granularOsc(double t, double freq, int sr) {
    const grainLenSec = 0.018;
    final grainSamples = (grainLenSec * sr).round();
    final idx = _grainCount % grainSamples;
    if (idx == 0) {
      _grainPhase = 0;
      _grainDetune = 1.0 + _noise.next() * 0.012;
    }
    _grainCount++;

    final gp = idx / grainSamples;
    final win = 0.5 * (1 - math.cos(2 * math.pi * gp));
    final tone = math.sin(2 * math.pi * _grainPhase);
    _grainPhase += freq * _grainDetune / sr;
    if (_grainPhase >= 1) _grainPhase -= 1;
    // Deterministic per-grain gate keeps the "particle" texture.
    final gate = (t * 220) % 1.0 < 0.82 ? 1.0 : 0.0;
    return (tone + _noise.next() * 0.12) * win * gate * 0.85;
  }
}

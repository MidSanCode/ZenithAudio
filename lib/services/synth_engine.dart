import 'dart:math' as math;
import 'dart:typed_data';

import '../models/envelope.dart';
import '../models/instrument.dart';
import '../models/note.dart';
import 'soundfont_parser.dart';

/// Shared sample-rate context passed through the render pipeline.
class RenderContext {
  final int sampleRate;

  const RenderContext({this.sampleRate = 44100});
}

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

// ─────────────────────────────────────────────────────────────
// Synth engines
// ─────────────────────────────────────────────────────────────
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
          break;        case SynthEngine.granular:
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

// ─────────────────────────────────────────────────────────────
// Track compressor parameters (persisted on Track).
// ─────────────────────────────────────────────────────────────
class TrackCompressorParams {
  final bool enabled;
  final double threshold; // 0..1
  final double ratio; // 1..20
  final double attack; // seconds
  final double release; // seconds

  const TrackCompressorParams({
    this.enabled = false,
    this.threshold = 0.4,
    this.ratio = 6.0,
    this.attack = 0.003,
    this.release = 0.06,
  });

  static const disabled = TrackCompressorParams();

  TrackCompressorParams copyWith({
    bool? enabled,
    double? threshold,
    double? ratio,
    double? attack,
    double? release,
  }) =>
      TrackCompressorParams(
        enabled: enabled ?? this.enabled,
        threshold: threshold ?? this.threshold,
        ratio: ratio ?? this.ratio,
        attack: attack ?? this.attack,
        release: release ?? this.release,
      );

  Map<String, dynamic> toJson() => {
        'enabled': enabled,
        'threshold': threshold,
        'ratio': ratio,
        'attack': attack,
        'release': release,
      };

  factory TrackCompressorParams.fromJson(Map<String, dynamic> json) =>
      TrackCompressorParams(
        enabled: json['enabled'] as bool? ?? false,
        threshold: (json['threshold'] as num?)?.toDouble() ?? 0.4,
        ratio: (json['ratio'] as num?)?.toDouble() ?? 6.0,
        attack: (json['attack'] as num?)?.toDouble() ?? 0.003,
        release: (json['release'] as num?)?.toDouble() ?? 0.06,
      );
}

// ─────────────────────────────────────────────────────────────
// High-level renderer shared by SynthService (io/web) and
// AudioService.prepareInstrumentTrack. Top-level so it can run
// inside an isolate.
// ─────────────────────────────────────────────────────────────
class SynthRenderJob {
  final List<Note> notes;
  final InstrumentPreset instrument;
  final double totalDuration;
  final int sampleRate;
  final SoundFontBank? bank;
  final TrackCompressorParams? compressor;

  SynthRenderJob({
    required this.notes,
    required this.instrument,
    required this.totalDuration,
    this.sampleRate = 44100,
    this.bank,
    this.compressor,
  });
}

Float64List renderNoteList(SynthRenderJob job) {
  final numSamples = (job.sampleRate * job.totalDuration).ceil();
  final buffer = Float64List(numSamples);
  if (numSamples <= 0 || job.notes.isEmpty) return buffer;

  final voice = SynthVoice(
    job.instrument,
    RenderContext(sampleRate: job.sampleRate),
    bank: job.bank,
  );
  final engine = engineFromString(job.instrument.synthEngine);

  final notes = [...job.notes]
    ..sort((a, b) => a.startTime.compareTo(b.startTime));

  for (final note in notes) {
    final startSample = (note.startTime * job.sampleRate).round();
    if (startSample >= numSamples) break;
    final tail = engine == SynthEngine.sample
        ? 0.35
        : (job.instrument.envCurve != null
            ? (job.instrument.envCurve!.points.last.x > 0.9 ? 0.4 : 0.25)
            : 0.25);
    final noteLen = (note.duration * job.sampleRate).round();
    final total = (noteLen + tail * job.sampleRate).round();
    final available = numSamples - startSample;
    final len = total < available ? total : available;
    if (len <= 0) continue;

    final rendered = voice.render(
      numSamples: len,
      pitch: note.pitch,
      velocity: note.velocity,
      noteDuration: note.duration,
      releaseSec: tail,
    );
    for (int i = 0; i < len; i++) {
      buffer[startSample + i] += rendered[i];
    }
  }

  if (job.compressor?.enabled == true) {
    final c = Compressor(
      threshold: job.compressor!.threshold,
      ratio: job.compressor!.ratio,
      attackSec: job.compressor!.attack,
      releaseSec: job.compressor!.release,
    );
    c.process(buffer, job.sampleRate);
    Compressor.autoMakeup(buffer);
  } else {
    _normalize(buffer);
  }
  return buffer;
}

void _normalize(Float64List buffer) {
  double maxAmp = 0;
  for (final s in buffer) {
    final abs = s.abs();
    if (abs > maxAmp) maxAmp = abs;
  }
  if (maxAmp > 0 && maxAmp > 0.95) {
    final scale = 0.95 / maxAmp;
    for (int i = 0; i < buffer.length; i++) {
      buffer[i] *= scale;
    }
  }
}

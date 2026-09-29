import 'dart:typed_data';

import '../models/instrument.dart';
import '../models/note.dart';
import 'dsp/compressor.dart';
import 'dsp/synth_common.dart';
import 'soundfont_parser.dart';
import 'synth_voice.dart';

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

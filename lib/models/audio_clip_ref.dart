/// Non-destructive audio-clip editing model (PLAN §3.S8 item 1).
///
/// ## The core idea
///
/// A destructive editor rewrites the sample buffer every time the user trims,
/// fades or gain-stages a clip — so the original is lost and undo has to keep
/// whole buffers around. A **non-destructive** clip instead *references* a
/// source buffer and stores a small set of parameters describing how to read
/// it:
///
/// ```text
///   source[sourceOffsetSamples ..]  ->  gain envelope  ->  fades  ->  output
/// ```
///
/// Trimming moves [sourceOffsetSamples]; a fade is two numbers; gain is a
/// curve. Nothing is lost, undo is just parameters, and several clips can share
/// one source without copying it.
///
/// ## Why this is a separate, pure type
///
/// It is the part that must be right: a clip whose fade length exceeds its
/// trimmed length would read past the source, and a gain envelope with points
/// out of order would produce a non-monotonic render. Those are correctness
/// problems independent of any widget, so the model and its envelope math live
/// here and are unit-tested, while the editor UI is a thin view over it.
library;

import 'dart:math' as math;
import 'dart:typed_data';

/// A point on a clip's gain envelope, positioned in seconds from the clip start.
class GainPoint {
  /// Position from the clip start, in seconds.
  final double timeSeconds;

  /// Linear gain at this point (`1.0` is unity).
  final double gain;

  const GainPoint(this.timeSeconds, this.gain);

  @override
  bool operator ==(Object other) =>
      other is GainPoint &&
      other.timeSeconds == timeSeconds &&
      other.gain == gain;

  @override
  int get hashCode => Object.hash(timeSeconds, gain);
}

/// Where a clip reads from, and how it is shaped on the way out.
///
/// Immutable: every edit returns a new reference, so a snapshot for undo is
/// just the old value.
class AudioClipRef {
  /// The source buffer. Shared by every clip that references it.
  final Float32List source;

  /// Source sample rate.
  final int sampleRate;

  /// Where in [source] this clip begins, in samples.
  final int sourceOffsetSamples;

  /// How many source samples the clip plays, starting at
  /// [sourceOffsetSamples]. Never more than the source holds.
  final int lengthSamples;

  /// Fade-in length in samples, from the clip start.
  final int fadeInSamples;

  /// Fade-out length in samples, ending at the clip end.
  final int fadeOutSamples;

  /// Overall linear gain applied to the whole clip.
  final double gain;

  /// Optional gain envelope over the clip. Empty means constant [gain].
  final List<GainPoint> envelope;

  /// Where the clip sits on its timeline, in seconds.
  final double startSeconds;

  AudioClipRef({
    required this.source,
    required this.sampleRate,
    this.sourceOffsetSamples = 0,
    int? lengthSamples,
    this.fadeInSamples = 0,
    this.fadeOutSamples = 0,
    this.gain = 1.0,
    this.envelope = const [],
    this.startSeconds = 0,
  }) : lengthSamples = _clampLength(
          lengthSamples ?? source.length,
          sourceOffsetSamples,
          source.length,
        );

  /// Clamps a requested length to what the source actually holds.
  static int _clampLength(int requested, int offset, int sourceLen) {
    if (offset < 0 || offset >= sourceLen) return 0;
    final available = sourceLen - offset;
    return requested < 0 ? 0 : math.min(requested, available);
  }

  /// The clip's length in seconds.
  double get durationSeconds => sampleRate <= 0 ? 0 : lengthSamples / sampleRate;

  /// The exclusive end offset into [source].
  int get sourceEndSamples => sourceOffsetSamples + lengthSamples;

  /// Whether the clip plays nothing.
  bool get isEmpty => lengthSamples <= 0;

  /// The maximum legal fade length; half the clip, so in/out cannot overlap.
  int get maxFadeSamples => lengthSamples ~/ 2;

  /// Whether the shape (fades/gain) is consistent with the length.
  ///
  /// A caller can assert this to catch a fade that would read past the clip.
  bool get isWellFormed =>
      fadeInSamples >= 0 &&
      fadeOutSamples >= 0 &&
      fadeInSamples <= maxFadeSamples &&
      fadeOutSamples <= maxFadeSamples &&
      gain.isFinite &&
      envelope.every((p) => p.timeSeconds.isFinite && p.gain.isFinite);

  /// Returns a copy with the given fields replaced.
  ///
  /// Fades longer than half the clip are clamped rather than rejected: a user
  /// dragging a fade handle to the middle should stop there, not error.
  AudioClipRef copyWith({
    int? sourceOffsetSamples,
    int? lengthSamples,
    int? fadeInSamples,
    int? fadeOutSamples,
    double? gain,
    List<GainPoint>? envelope,
    double? startSeconds,
  }) {
    final offset = sourceOffsetSamples ?? this.sourceOffsetSamples;
    final length = lengthSamples ?? this.lengthSamples;
    final maxFade = math.max(0, _clampLength(length, offset, source.length) ~/ 2);
    return AudioClipRef(
      source: source,
      sampleRate: sampleRate,
      sourceOffsetSamples: offset,
      lengthSamples: length,
      fadeInSamples: (fadeInSamples ?? this.fadeInSamples).clamp(0, maxFade),
      fadeOutSamples: (fadeOutSamples ?? this.fadeOutSamples).clamp(0, maxFade),
      gain: gain ?? this.gain,
      envelope: envelope ?? this.envelope,
      startSeconds: startSeconds ?? this.startSeconds,
    );
  }

  /// Trims the clip to `[offsetSeconds, offsetSeconds + durationSeconds)` of the
  /// clip's own timeline.
  ///
  /// This is the non-destructive trim: it moves the source offset and length and
  /// never touches the source buffer. Fades are clamped to the new length.
  AudioClipRef trim({required double startSeconds, required double endSeconds}) {
    final from = _secondsToSamples(startSeconds).clamp(0, lengthSamples);
    final to = _secondsToSamples(endSeconds).clamp(from, lengthSamples);
    return copyWith(
      sourceOffsetSamples: sourceOffsetSamples + from,
      lengthSamples: to - from,
      fadeInSamples: 0,
      fadeOutSamples: 0,
    );
  }

  /// Renders the clip to a new `Float32List`, applying the envelope and fades.
  ///
  /// The source buffer is never modified. A render is how the clip is played or
  /// exported; the clip itself stays parameters.
  Float32List render() {
    final out = Float32List(lengthSamples);
    if (lengthSamples == 0) return out;
    final envelope = _normalizedEnvelope();
    for (var i = 0; i < lengthSamples; i++) {
      final env = _envelopeAt(envelope, i);
      final fade = _fadeAt(i);
      final value = source[sourceOffsetSamples + i] * gain * env * fade;
      out[i] = value.isFinite ? value : 0.0;
    }
    return out;
  }

  /// Samples the envelope at sample [i] of the clip.
  ///
  /// The envelope is assumed sorted by time with at least two points; a
  /// single-point or empty envelope means a constant gain of 1.
  double _envelopeAt(List<GainPoint> env, int i) {
    if (env.length < 2) return 1.0;
    final t = i / sampleRate;
    if (t <= env.first.timeSeconds) return env.first.gain;
    if (t >= env.last.timeSeconds) return env.last.gain;
    for (var p = 0; p < env.length - 1; p++) {
      final a = env[p];
      final b = env[p + 1];
      if (t >= a.timeSeconds && t <= b.timeSeconds) {
        final span = b.timeSeconds - a.timeSeconds;
        if (span <= 0) return b.gain;
        final frac = (t - a.timeSeconds) / span;
        return a.gain + (b.gain - a.gain) * frac;
      }
    }
    return env.last.gain;
  }

  /// Sorts and bounds the envelope so evaluation can assume order.
  ///
  /// A caller may insert points in any order; normalizing here means the
  /// evaluation loop does not have to defend against an unsorted list.
  List<GainPoint> _normalizedEnvelope() {
    if (envelope.isEmpty) return const [];
    final sorted = [...envelope]
      ..sort((a, b) => a.timeSeconds.compareTo(b.timeSeconds));
    return sorted;
  }

  /// The combined fade-in / fade-out gain at sample [i].
  double _fadeAt(int i) {
    var g = 1.0;
    if (fadeInSamples > 0 && i < fadeInSamples) {
      g *= i / fadeInSamples;
    }
    final fromEnd = lengthSamples - 1 - i;
    if (fadeOutSamples > 0 && fromEnd < fadeOutSamples) {
      g *= fromEnd / fadeOutSamples;
    }
    return g;
  }

  int _secondsToSamples(double seconds) =>
      (seconds * sampleRate).round();
}

/// Crossfades two clips that overlap on the timeline.
///
/// Returns the mixed sample buffer for the overlapped region, using an
/// equal-power law so the sum stays constant for uncorrelated material. The
/// inputs are rendered, not modified.
Float32List crossfadeClips(
  AudioClipRef a,
  AudioClipRef b, {
  required int overlapSamples,
}) {
  final n = overlapSamples
      .clamp(1, math.min(a.lengthSamples, b.lengthSamples))
      .toInt();
  final out = Float32List(n);
  for (var i = 0; i < n; i++) {
    final t = n == 1 ? 1.0 : i / (n - 1);
    // Quarter-sine pair: constant power.
    final ga = math.cos(t * math.pi / 2);
    final gb = math.sin(t * math.pi / 2);
    final sa = a.source[(a.sourceEndSamples - n) + i];
    final sb = b.source[b.sourceOffsetSamples + i];
    out[i] = sa * ga + sb * gb;
  }
  return out;
}

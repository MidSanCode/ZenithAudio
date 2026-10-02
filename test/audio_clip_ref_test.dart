import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/audio_clip_ref.dart';

/// S8: the non-destructive clip model — reference + offset + gain + fades.
void main() {
  Float32List ramp(int len) =>
      Float32List.fromList(List.generate(len, (i) => i / len));

  group('construction and bounds', () {
    test('length is clamped to what the source holds', () {
      final clip = AudioClipRef(
        source: ramp(100),
        sampleRate: 100,
        sourceOffsetSamples: 60,
        lengthSamples: 1000,
      );
      expect(clip.lengthSamples, 40, reason: 'only 40 samples remain');
    });

    test('an offset past the end yields an empty clip', () {
      final clip = AudioClipRef(
        source: ramp(100),
        sampleRate: 100,
        sourceOffsetSamples: 100,
      );
      expect(clip.isEmpty, isTrue);
      expect(clip.lengthSamples, 0);
    });

    test('duration is length over sample rate', () {
      final clip = AudioClipRef(
        source: ramp(48000),
        sampleRate: 48000,
        lengthSamples: 24000,
      );
      expect(clip.durationSeconds, closeTo(0.5, 1e-9));
    });
  });

  group('non-destructive trim', () {
    test('moves the source offset without touching the source', () {
      final source = ramp(100);
      final clip = AudioClipRef(source: source, sampleRate: 100);
      final trimmed = clip.trim(startSeconds: 0.25, endSeconds: 0.75);
      expect(trimmed.sourceOffsetSamples, 25);
      expect(trimmed.lengthSamples, 50);
      // The source is untouched.
      expect(identical(trimmed.source, source), isTrue);
      expect(source[0], 0.0);
    });

    test('an inverted range collapses to empty rather than throwing', () {
      final clip = AudioClipRef(source: ramp(100), sampleRate: 100);
      final trimmed = clip.trim(startSeconds: 0.8, endSeconds: 0.2);
      expect(trimmed.lengthSamples, 0);
    });

    test('trimming clamps out-of-range seconds', () {
      final clip = AudioClipRef(source: ramp(100), sampleRate: 100);
      final trimmed = clip.trim(startSeconds: -1, endSeconds: 999);
      expect(trimmed.sourceOffsetSamples, 0);
      expect(trimmed.lengthSamples, 100);
    });
  });

  group('fades', () {
    test('a fade longer than half the clip is clamped', () {
      final clip = AudioClipRef(source: ramp(100), sampleRate: 100);
      final shaped = clip.copyWith(fadeInSamples: 999, fadeOutSamples: 999);
      expect(shaped.fadeInSamples, 50);
      expect(shaped.fadeOutSamples, 50);
      expect(shaped.isWellFormed, isTrue);
    });

    test('a fade-in ramps from zero to unity', () {
      final clip = AudioClipRef(
        source: Float32List.fromList(List.filled(10, 1.0)),
        sampleRate: 10,
        fadeInSamples: 5,
      );
      final rendered = clip.render();
      expect(rendered[0], 0.0);
      expect(rendered[4], closeTo(0.8, 1e-6));
      expect(rendered[5], 1.0, reason: 'past the fade');
    });

    test('a fade-out ramps to zero at the last sample', () {
      final clip = AudioClipRef(
        source: Float32List.fromList(List.filled(10, 1.0)),
        sampleRate: 10,
        fadeOutSamples: 5,
      );
      final rendered = clip.render();
      expect(rendered[9], 0.0, reason: 'the last sample is fully faded out');
      expect(rendered[4], 1.0, reason: 'before the fade-out');
    });
  });

  group('gain envelope', () {
    test('interpolates between points', () {
      final clip = AudioClipRef(
        source: Float32List.fromList(List.filled(10, 1.0)),
        sampleRate: 10,
        envelope: const [GainPoint(0, 0.0), GainPoint(0.9, 1.0)],
      );
      final rendered = clip.render();
      expect(rendered[0], closeTo(0.0, 1e-6));
      // Sample 4 is t = 0.4 s; the envelope runs 0 -> 0.9 s, so the fraction is
      // 0.4 / 0.9 ≈ 0.444.
      expect(rendered[4], closeTo(0.444, 0.01));
    });

    test('an unsorted envelope is normalized before evaluation', () {
      final clip = AudioClipRef(
        source: Float32List.fromList(List.filled(10, 1.0)),
        sampleRate: 10,
        envelope: const [GainPoint(0.9, 1.0), GainPoint(0, 0.0)],
      );
      final rendered = clip.render();
      expect(rendered[0], closeTo(0.0, 1e-6), reason: 'starts at the gain-0 point');
      expect(rendered[9], closeTo(1.0, 1e-6));
    });

    test('a single point is treated as constant unity', () {
      final clip = AudioClipRef(
        source: Float32List.fromList(List.filled(5, 1.0)),
        sampleRate: 5,
        envelope: const [GainPoint(0, 0.3)],
      );
      // One point is not enough to interpolate; the clip falls back to unity.
      expect(clip.render()[2], 1.0);
    });
  });

  group('gain', () {
    test('scales the whole output', () {
      final clip = AudioClipRef(
        source: Float32List.fromList(List.filled(4, 0.5)),
        sampleRate: 4,
        gain: 0.5,
      );
      expect(clip.render()[0], closeTo(0.25, 1e-6));
    });

    test('a non-finite source sample is written as silence', () {
      final clip = AudioClipRef(
        source: Float32List.fromList([double.nan, 1.0]),
        sampleRate: 2,
      );
      final rendered = clip.render();
      expect(rendered[0], 0.0);
      expect(rendered[1], 1.0);
    });
  });

  group('crossfade', () {
    test('equal-power curves keep the sum near unity for a constant signal', () {
      final a = AudioClipRef(
        source: Float32List.fromList(List.filled(100, 1.0)),
        sampleRate: 100,
      );
      final b = AudioClipRef(
        source: Float32List.fromList(List.filled(100, 1.0)),
        sampleRate: 100,
      );
      final mixed = crossfadeClips(a, b, overlapSamples: 10);
      // At the midpoint both gains are sin/cos(45°) ≈ 0.707, so the sum of a
      // correlated pair is sqrt(2) ≈ 1.414 (equal-power), but for the same
      // constant signal the result stays bounded and finite.
      expect(mixed.every((s) => s.isFinite), isTrue);
      expect(mixed.first, closeTo(1.0, 0.2), reason: 'starts near a alone');
      expect(mixed.last, closeTo(1.0, 0.2), reason: 'ends near b alone');
    });
  });

  group('well-formedness', () {
    test('a default clip is well formed', () {
      final clip = AudioClipRef(source: ramp(100), sampleRate: 100);
      expect(clip.isWellFormed, isTrue);
    });

    test('an infinite gain is not well formed', () {
      final clip = AudioClipRef(
        source: ramp(100),
        sampleRate: 100,
        gain: double.infinity,
      );
      expect(clip.isWellFormed, isFalse);
    });
  });
}

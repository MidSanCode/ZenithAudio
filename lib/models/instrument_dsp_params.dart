import 'dart:math';

import 'instrument.dart';

/// Additive synthesis + ADSR, split out of `instrument.dart` in S0.
///
/// Declared as an extension so [InstrumentPreset] keeps the exact same public
/// surface (`preset.synthSample(...)`, `preset.getEnvelope(...)`) for every
/// existing caller.
extension InstrumentDspParams on InstrumentPreset {
  double synthSample(double t, double freq, int velocity) {
    final vel = velocity / 127.0;
    final bright = brightnessFactor * vel;

    // ── Per-family character (derived from [category]; no schema change) ──
    // spectralDecay: how quickly upper partials lose energy over time.
    // Static spectra made every preset sound like the same additive organ;
    // real families differ largely in how their spectrum evolves.
    final double spectralDecay;
    final double vibRate;
    final double vibDepth;
    // Formant resonance: a fixed frequency-band emphasis independent of the
    // note pitch — the main cue that separates instruments built from
    // similar 1/k harmonic tables (body resonance for strings, vocal tract
    // for winds, soundboard coloration for keyboards).
    final double formantHz;
    final double formantWidthHz;
    final double formantGain;
    switch (category) {
      case InstrumentCategory.keyboard:
        spectralDecay = 2.2; vibRate = 0; vibDepth = 0;
        formantHz = 900; formantWidthHz = 900; formantGain = 0.35;
        break;
      case InstrumentCategory.string:
        spectralDecay = 2.4; vibRate = 5.2; vibDepth = 0.004;
        formantHz = 1100; formantWidthHz = 600; formantGain = 0.9;
        break;
      case InstrumentCategory.wind:
        spectralDecay = 0.45; vibRate = 5.0; vibDepth = 0.003;
        formantHz = 1900; formantWidthHz = 800; formantGain = 0.7;
        break;
      case InstrumentCategory.synth:
        spectralDecay = 0.08; vibRate = 0; vibDepth = 0;
        formantHz = 0; formantWidthHz = 1; formantGain = 0;
        break;
      case InstrumentCategory.percussion:
        spectralDecay = 3.5; vibRate = 0; vibDepth = 0;
        formantHz = 0; formantWidthHz = 1; formantGain = 0;
        break;
    }

    // Vibrato fades in over the first ~0.4 s like a real player.
    final vibGain = ((t / 0.4).clamp(0.0, 1.0)).toDouble();
    final f = freq * (1.0 + vibDepth * sin(2 * pi * vibRate * t) * vibGain);

    final n = harmonics.length;
    final tiltSpan = n > 1 ? (n - 1) : 1;

    // Multiplicative brightness tilt — boosts upper partials with velocity
    // while preserving the instrument's own spectral identity (the old
    // additive 0.5/h skirt drowned it, making spectra ~99% identical).
    double s = 0;
    for (int h = 0; h < n; h++) {
      final partial = h + 1;
      final amp = harmonics[h] * (1.0 + bright * (partial - 1) / tiltSpan);
      final damp = partial > 1
          ? exp(-t * spectralDecay * (partial - 1) * 0.35)
          : 1.0;
      // Formant: boost partials that fall inside the body/tract resonance.
      final partialFreq = f * partial;
      final d = (partialFreq - formantHz) / formantWidthHz;
      final formant = formantGain > 0
          ? 1.0 + formantGain * exp(-d * d)
          : 1.0;
      s += sin(2 * pi * f * partial * t) * amp * damp * formant;
    }

    // Detuned oscillator (if enabled)
    if (detuneCents > 0) {
      final detuneRatio = pow(2, detuneCents / 1200).toDouble();
      double s2 = 0;
      for (int h = 0; h < n; h++) {
        final partial = h + 1;
        final damp = partial > 1
            ? exp(-t * spectralDecay * (partial - 1) * 0.35)
            : 1.0;
        s2 += sin(2 * pi * f * detuneRatio * partial * t)
            * harmonics[h] * 0.4 * damp;
      }
      s += s2;
    }

    // Noise attack transient
    if (noiseAttack > 0) {
      final noise = InstrumentPreset.sharedNoiseGen.nextDouble() * 2 - 1;
      s += noise * noiseAttack * exp(-t * 80);
    }

    return s * 0.6; // master level
  }

  double getEnvelope(double t, double dur, int velocity) {
    final rStart = dur - release;
    final vel = velocity / 127.0;
    // Velocity affects attack speed slightly
    final att = attack * (1.0 - vel * 0.3);

    if (t < att) return (t / att) * vel;
    if (t < att + decay) return vel - (vel - sustain * vel) * ((t - att) / decay);
    if (t < rStart) return sustain * vel;
    return (sustain * vel) * max(0.0, 1.0 - (t - rStart) / release);
  }
}

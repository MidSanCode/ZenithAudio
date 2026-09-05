import 'dart:math';
import 'package:flutter/material.dart';

enum InstrumentCategory { keyboard, string, wind, synth, percussion }

class InstrumentPreset {
  final String id;
  final String name;
  final String description;
  final IconData icon;
  final InstrumentCategory category;
  final int programNumber;

  // ── Synthesis parameters ──
  final List<double> harmonics;      // amplitude for harmonics 1-16
  final double attack;               // seconds
  final double decay;                // seconds
  final double sustain;              // 0-1 level
  final double release;              // seconds
  final double detuneCents;          // dual-oscillator detune (0 = none)
  final double noiseAttack;          // noise burst amplitude (0 = none)
  final double brightnessFactor;     // velocity brightness scaling

  const InstrumentPreset({
    required this.id,
    required this.name,
    this.description = '',
    this.icon = Icons.music_note_outlined,
    this.category = InstrumentCategory.synth,
    required this.programNumber,
    required this.harmonics,
    this.attack = 0.01,
    this.decay = 0.2,
    this.sustain = 0.7,
    this.release = 0.1,
    this.detuneCents = 0,
    this.noiseAttack = 0,
    this.brightnessFactor = 0.3,
  });

  static final List<InstrumentPreset> _userPresets = [];

  static void addUserPresets(List<InstrumentPreset> presets) {
    for (final p in presets) {
      // Replace existing by id
      final idx = _userPresets.indexWhere((e) => e.id == p.id);
      if (idx >= 0) {
        _userPresets[idx] = p;
      } else {
        _userPresets.add(p);
      }
    }
  }

  static List<InstrumentPreset> get userPresets => List.unmodifiable(_userPresets);

  static List<InstrumentPreset> get allPresets => [...presets, ..._userPresets];

  static InstrumentPreset? fromIdOrNull(String id) {
    try {
      return allPresets.firstWhere((p) => p.id == id);
    } catch (_) {
      return null;
    }
  }

  static const List<InstrumentPreset> presets = [
    // ── Keyboards ──
    InstrumentPreset(
      id: 'piano',
      name: 'Acoustic Grand Piano',
      description: 'Rich, dynamic grand piano',
      icon: Icons.piano,
      category: InstrumentCategory.keyboard,
      programNumber: 0,
      harmonics: [1.0, 0.8, 0.45, 0.30, 0.18, 0.10, 0.06, 0.03],
      attack: 0.003, decay: 0.4, sustain: 0.25, release: 0.15,
      detuneCents: 0.3, noiseAttack: 0.15, brightnessFactor: 0.5,
    ),
    InstrumentPreset(
      id: 'bright_piano',
      name: 'Bright Acoustic Piano',
      description: 'Bright, cutting piano tone',
      icon: Icons.piano,
      category: InstrumentCategory.keyboard,
      programNumber: 1,
      harmonics: [1.0, 0.9, 0.6, 0.45, 0.30, 0.20, 0.12, 0.08],
      attack: 0.002, decay: 0.35, sustain: 0.20, release: 0.12,
      detuneCents: 0.4, noiseAttack: 0.18, brightnessFactor: 0.6,
    ),
    InstrumentPreset(
      id: 'organ',
      name: 'Church Organ',
      description: 'Full, sustaining pipe organ',
      icon: Icons.toc,
      category: InstrumentCategory.keyboard,
      programNumber: 19,
      harmonics: [1.0, 0.5, 0.8, 0.3, 0.6, 0.2, 0.4, 0.1],
      attack: 0.04, decay: 0.05, sustain: 0.95, release: 0.2,
      detuneCents: 1.0, noiseAttack: 0, brightnessFactor: 0.2,
    ),
    InstrumentPreset(
      id: 'accordion',
      name: 'Accordion',
      description: 'Reedy, expressive accordion',
      icon: Icons.toc,
      category: InstrumentCategory.keyboard,
      programNumber: 21,
      harmonics: [1.0, 0.6, 0.9, 0.4, 0.5, 0.25, 0.2, 0.1],
      attack: 0.01, decay: 0.1, sustain: 0.85, release: 0.05,
      detuneCents: 2.0, noiseAttack: 0, brightnessFactor: 0.3,
    ),

    // ── Strings ──
    InstrumentPreset(
      id: 'guitar',
      name: 'Acoustic Guitar (nylon)',
      description: 'Warm nylon-string guitar',
      icon: Icons.music_note_outlined,
      category: InstrumentCategory.string,
      programNumber: 24,
      harmonics: [1.0, 0.7, 0.3, 0.15, 0.08, 0.04, 0.02, 0.01],
      attack: 0.002, decay: 0.15, sustain: 0.6, release: 0.05,
      detuneCents: 0.5, noiseAttack: 0.12, brightnessFactor: 0.4,
    ),
    InstrumentPreset(
      id: 'steel_guitar',
      name: 'Acoustic Guitar (steel)',
      description: 'Bright steel-string acoustic',
      icon: Icons.music_note_outlined,
      category: InstrumentCategory.string,
      programNumber: 25,
      harmonics: [1.0, 0.8, 0.4, 0.20, 0.12, 0.06, 0.04, 0.02],
      attack: 0.001, decay: 0.12, sustain: 0.55, release: 0.04,
      detuneCents: 0.6, noiseAttack: 0.15, brightnessFactor: 0.5,
    ),
    InstrumentPreset(
      id: 'strings',
      name: 'String Ensemble',
      description: 'Lush, sustaining strings',
      icon: Icons.audiotrack,
      category: InstrumentCategory.string,
      programNumber: 48,
      harmonics: [1.0, 0.5, 0.35, 0.25, 0.18, 0.12, 0.08, 0.05],
      attack: 0.08, decay: 0.2, sustain: 0.85, release: 0.4,
      detuneCents: 3.0, noiseAttack: 0, brightnessFactor: 0.25,
    ),
    InstrumentPreset(
      id: 'pizzicato',
      name: 'Pizzicato Strings',
      description: 'Plucked, short strings',
      icon: Icons.audiotrack,
      category: InstrumentCategory.string,
      programNumber: 45,
      harmonics: [1.0, 0.6, 0.3, 0.15, 0.08, 0.04, 0.02, 0.01],
      attack: 0.001, decay: 0.08, sustain: 0, release: 0.02,
      detuneCents: 0.2, noiseAttack: 0.08, brightnessFactor: 0.3,
    ),

    // ── Bass ──
    InstrumentPreset(
      id: 'bass',
      name: 'Acoustic Bass',
      description: 'Warm upright bass',
      icon: Icons.music_note_outlined,
      category: InstrumentCategory.string,
      programNumber: 32,
      harmonics: [1.0, 0.6, 0.3, 0.12, 0.06, 0.03, 0.015, 0.008],
      attack: 0.005, decay: 0.15, sustain: 0.6, release: 0.08,
      detuneCents: 0.4, noiseAttack: 0.08, brightnessFactor: 0.3,
    ),
    InstrumentPreset(
      id: 'electric_bass',
      name: 'Electric Bass (finger)',
      description: 'Deep, punchy electric bass',
      icon: Icons.music_note_outlined,
      category: InstrumentCategory.string,
      programNumber: 33,
      harmonics: [1.0, 0.7, 0.35, 0.18, 0.10, 0.05, 0.025, 0.012],
      attack: 0.003, decay: 0.12, sustain: 0.65, release: 0.06,
      detuneCents: 0.5, noiseAttack: 0.10, brightnessFactor: 0.35,
    ),

    // ── Brass / Wind ──
    InstrumentPreset(
      id: 'brass',
      name: 'Brass Section',
      description: 'Bold, powerful brass ensemble',
      icon: Icons.music_note_outlined,
      category: InstrumentCategory.wind,
      programNumber: 61,
      harmonics: [1.0, 0.8, 0.6, 0.45, 0.3, 0.2, 0.12, 0.08],
      attack: 0.02, decay: 0.15, sustain: 0.85, release: 0.2,
      detuneCents: 1.5, noiseAttack: 0.05, brightnessFactor: 0.4,
    ),
    InstrumentPreset(
      id: 'trumpet',
      name: 'Trumpet',
      description: 'Bright, piercing trumpet',
      icon: Icons.music_note_outlined,
      category: InstrumentCategory.wind,
      programNumber: 56,
      harmonics: [1.0, 0.9, 0.7, 0.55, 0.4, 0.28, 0.18, 0.10],
      attack: 0.01, decay: 0.1, sustain: 0.9, release: 0.15,
      detuneCents: 0.5, noiseAttack: 0.04, brightnessFactor: 0.45,
    ),

    // ── Synth ──
    InstrumentPreset(
      id: 'synth',
      name: 'Synth Lead',
      description: 'Pulsing, cutting synth lead',
      icon: Icons.electric_bolt,
      category: InstrumentCategory.synth,
      programNumber: 80,
      harmonics: [1.0, 0.4, 0.6, 0.2, 0.4, 0.1, 0.2, 0.05],
      attack: 0.005, decay: 0.1, sustain: 0.9, release: 0.05,
      detuneCents: 1.5, noiseAttack: 0, brightnessFactor: 0.2,
    ),
    InstrumentPreset(
      id: 'pad',
      name: 'Synth Pad',
      description: 'Warm, evolving synth pad',
      icon: Icons.waves,
      category: InstrumentCategory.synth,
      programNumber: 88,
      harmonics: [1.0, 0.3, 0.5, 0.2, 0.35, 0.15, 0.25, 0.1],
      attack: 0.3, decay: 0.3, sustain: 0.9, release: 0.8,
      detuneCents: 5.0, noiseAttack: 0, brightnessFactor: 0.15,
    ),
    InstrumentPreset(
      id: 'warm_pad',
      name: 'Warm Pad',
      description: 'Soft, warm analog pad',
      icon: Icons.waves,
      category: InstrumentCategory.synth,
      programNumber: 89,
      harmonics: [1.0, 0.4, 0.2, 0.1, 0.05, 0.025, 0.012, 0.006],
      attack: 0.2, decay: 0.2, sustain: 0.95, release: 0.6,
      detuneCents: 4.0, noiseAttack: 0, brightnessFactor: 0.1,
    ),
  ];

  static InstrumentPreset fromId(String id) =>
      allPresets.firstWhere((p) => p.id == id);

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
      final noise = (Random().nextDouble() * 2 - 1) * noiseAttack * exp(-t * 80);
      s += noise;
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

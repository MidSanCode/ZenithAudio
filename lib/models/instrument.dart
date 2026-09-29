import 'dart:convert';
import 'dart:math';
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'envelope.dart';
import 'instrument_presets_data.dart';

// The extension carries `synthSample` / `getEnvelope`; re-exported so every
// existing `import 'instrument.dart'` site keeps seeing them.
export 'instrument_dsp_params.dart';

enum InstrumentCategory { keyboard, string, wind, synth, percussion }

class InstrumentPreset {
  final String id;
  final String name;
  final String description;
  final IconData icon;
  final InstrumentCategory category;
  final int programNumber;

  // Shared per-preset noise generator (avoids allocating Random per sample).
  // Exposed to `instrument_dsp_params.dart`, which owns the sample loop.
  static final Random sharedNoiseGen = Random(7);

  // ── Synthesis parameters ──
  final List<double> harmonics;      // amplitude for harmonics 1-16
  final double attack;               // seconds
  final double decay;                // seconds
  final double sustain;              // 0-1 level
  final double release;              // seconds
  final double detuneCents;          // dual-oscillator detune (0 = none)
  final double noiseAttack;          // noise burst amplitude (0 = none)
  final double brightnessFactor;     // velocity brightness scaling

  // ── Synth engine extension ──
  // null / 'additive' keeps the legacy harmonic engine. Other values select
  // the corresponding SynthEngine ('subtractive', 'wavetable', 'fm',
  // 'sample', 'granular').
  final String? synthEngine;

  /// Multi-point amplitude envelope (points + curves). When present it
  /// replaces the ADSR from attack/decay/sustain/release.
  final EnvelopeCurve? envCurve;

  // Subtractive filter.
  final String filterType;       // lowPass | highPass | bandPass | notch
  final double filterCutoff;     // Hz
  final double filterResonance;  // SVF q
  final double filterEnvAmount;  // cutoff multiplier at env peak
  final double filterAttack;     // seconds
  final double filterDecay;      // seconds
  final double filterSustain;    // 0..1

  // FM (2-op).
  final double fmRatio;    // modulator ratio (cycles of carrier)
  final double fmIndex;    // modulation index
  final double fmDecay;    // index decay seconds
  final double fmFeedback; // 0..1 extra sustain index fraction

  // Wavetable morph.
  final double morphRate;  // seconds per morph cycle (0 = static frame 0)

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
    this.synthEngine,
    this.envCurve,
    this.filterType = 'lowPass',
    this.filterCutoff = 1200,
    this.filterResonance = 1.2,
    this.filterEnvAmount = 2.0,
    this.filterAttack = 0.005,
    this.filterDecay = 0.3,
    this.filterSustain = 0.3,
    this.fmRatio = 2.0,
    this.fmIndex = 3.0,
    this.fmDecay = 0.8,
    this.fmFeedback = 0.15,
    this.morphRate = 0,
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

  static List<InstrumentPreset> get allPresets =>
      [...presets, ...synthPresets, ..._userPresets, ..._soundFontPresets, ..._customPresets];

  static InstrumentPreset? fromIdOrNull(String id) {
    try {
      return allPresets.firstWhere((p) => p.id == id);
    } catch (_) {
      return null;
    }
  }

  /// Built-in melodic/sampled presets — data lives in
  /// instrument_presets_data.dart (S0 file split).
  static const List<InstrumentPreset> presets = kBuiltinPresets;

  /// Built-in synth-engine presets — data lives in the same file.
  static final List<InstrumentPreset> synthPresets = kSynthPresets;


  static InstrumentPreset fromId(String id) =>
      allPresets.firstWhere((p) => p.id == id);

  // `synthSample` and `getEnvelope` live in `instrument_dsp_params.dart` as an
  // extension on this class (S0 file split); the call sites are unchanged.

  // ── SoundFont virtual preset registry ──
  static final List<InstrumentPreset> _soundFontPresets = [];

  /// Register virtual presets for a loaded SoundFont bank.
  static void registerSoundFontPresets(
      List<({int program, int bank, String name})> presets) {
    clearSoundFontPresets();
    for (final p in presets) {
      _soundFontPresets.add(InstrumentPreset(
        id: 'sf2_${p.bank}_${p.program}',
        name: p.name,
        description: 'SoundFont bank ${p.bank} program ${p.program}',
        icon: Icons.library_music_outlined,
        category: InstrumentCategory.synth,
        programNumber: p.program,
        harmonics: const [1.0],
        synthEngine: 'sample',
        attack: 0.002,
        decay: 0.3,
        sustain: 0.8,
        release: 0.3,
      ));
    }
  }

  static void clearSoundFontPresets() => _soundFontPresets.clear();

  static List<InstrumentPreset> get soundFontPresets =>
      List.unmodifiable(_soundFontPresets);

  // ── Built-in synth-engine presets ──

  // ── User custom presets (persisted via SharedPreferences) ──
  static final List<InstrumentPreset> _customPresets = [];
  static bool _persistLoaded = false;

  static List<InstrumentPreset> get customPresets =>
      List.unmodifiable(_customPresets);

  static Future<void> loadPersisted() async {
    if (_persistLoaded) return;
    try {
      final prefs = await SharedPreferences.getInstance();
      final raw = prefs.getStringList('zenith_user_instruments') ?? const [];
      _customPresets
        ..clear()
        ..addAll(raw
            .map((s) => _presetFromJson(s))
            .whereType<InstrumentPreset>());
      _persistLoaded = true;
    } catch (_) {
      // Non-fatal: persistence is best-effort.
    }
  }

  static Future<void> saveUserCustom(InstrumentPreset preset) async {
    final idx = _customPresets.indexWhere((e) => e.id == preset.id);
    if (idx >= 0) {
      _customPresets[idx] = preset;
    } else {
      _customPresets.add(preset);
    }
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.setStringList('zenith_user_instruments',
          _customPresets.map(_presetToJson).toList());
    } catch (_) {}
  }

  static Future<void> deleteUserCustom(String id) async {
    _customPresets.removeWhere((e) => e.id == id);
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.setStringList('zenith_user_instruments',
          _customPresets.map(_presetToJson).toList());
    } catch (_) {}
  }

  static String _presetToJson(InstrumentPreset p) {
    final m = <String, dynamic>{
      'id': p.id, 'name': p.name, 'description': p.description,
      'category': p.category.name, 'programNumber': p.programNumber,
      'harmonics': p.harmonics,
      'attack': p.attack, 'decay': p.decay, 'sustain': p.sustain,
      'release': p.release, 'detuneCents': p.detuneCents,
      'noiseAttack': p.noiseAttack, 'brightnessFactor': p.brightnessFactor,
      if (p.synthEngine != null) 'synthEngine': p.synthEngine,
      if (p.envCurve != null) 'envCurve': p.envCurve!.toJson(),
      'filterType': p.filterType,
      'filterCutoff': p.filterCutoff, 'filterResonance': p.filterResonance,
      'filterEnvAmount': p.filterEnvAmount,
      'filterAttack': p.filterAttack, 'filterDecay': p.filterDecay,
      'filterSustain': p.filterSustain,
      'fmRatio': p.fmRatio, 'fmIndex': p.fmIndex, 'fmDecay': p.fmDecay,
      'fmFeedback': p.fmFeedback, 'morphRate': p.morphRate,
    };
    return jsonEncode(m);
  }

  static InstrumentPreset? _presetFromJson(String s) {
    try {
      final m = jsonDecode(s) as Map<String, dynamic>;
      return InstrumentPreset(
        id: m['id'] as String,
        name: m['name'] as String? ?? 'Custom',
        description: m['description'] as String? ?? '',
        category: InstrumentCategory.values.firstWhere(
            (c) => c.name == m['category'],
            orElse: () => InstrumentCategory.synth),
        programNumber: m['programNumber'] as int? ?? 80,
        harmonics: ((m['harmonics'] as List?) ?? const [1.0])
            .map((e) => (e as num).toDouble()).toList(),
        attack: (m['attack'] as num?)?.toDouble() ?? 0.01,
        decay: (m['decay'] as num?)?.toDouble() ?? 0.2,
        sustain: (m['sustain'] as num?)?.toDouble() ?? 0.7,
        release: (m['release'] as num?)?.toDouble() ?? 0.1,
        detuneCents: (m['detuneCents'] as num?)?.toDouble() ?? 0,
        noiseAttack: (m['noiseAttack'] as num?)?.toDouble() ?? 0,
        brightnessFactor: (m['brightnessFactor'] as num?)?.toDouble() ?? 0.3,
        synthEngine: m['synthEngine'] as String?,
        envCurve: m['envCurve'] != null
            ? EnvelopeCurve.fromJson(m['envCurve'] as Map<String, dynamic>)
            : null,
        filterType: m['filterType'] as String? ?? 'lowPass',
        filterCutoff: (m['filterCutoff'] as num?)?.toDouble() ?? 1200,
        filterResonance: (m['filterResonance'] as num?)?.toDouble() ?? 1.2,
        filterEnvAmount: (m['filterEnvAmount'] as num?)?.toDouble() ?? 2.0,
        filterAttack: (m['filterAttack'] as num?)?.toDouble() ?? 0.005,
        filterDecay: (m['filterDecay'] as num?)?.toDouble() ?? 0.3,
        filterSustain: (m['filterSustain'] as num?)?.toDouble() ?? 0.3,
        fmRatio: (m['fmRatio'] as num?)?.toDouble() ?? 2.0,
        fmIndex: (m['fmIndex'] as num?)?.toDouble() ?? 3.0,
        fmDecay: (m['fmDecay'] as num?)?.toDouble() ?? 0.8,
        fmFeedback: (m['fmFeedback'] as num?)?.toDouble() ?? 0.15,
        morphRate: (m['morphRate'] as num?)?.toDouble() ?? 0,
      );
    } catch (_) {
      return null;
    }
  }

  InstrumentPreset copyWith({
    String? id,
    String? name,
    String? description,
    InstrumentCategory? category,
    int? programNumber,
    List<double>? harmonics,
    double? attack, double? decay, double? sustain, double? release,
    double? detuneCents, double? noiseAttack, double? brightnessFactor,
    String? synthEngine, EnvelopeCurve? envCurve,
    String? filterType, double? filterCutoff, double? filterResonance,
    double? filterEnvAmount, double? filterAttack, double? filterDecay,
    double? filterSustain,
    double? fmRatio, double? fmIndex, double? fmDecay, double? fmFeedback,
    double? morphRate,
  }) =>
      InstrumentPreset(
        id: id ?? this.id,
        name: name ?? this.name,
        description: description ?? this.description,
        icon: icon,
        category: category ?? this.category,
        programNumber: programNumber ?? this.programNumber,
        harmonics: harmonics ?? this.harmonics,
        attack: attack ?? this.attack,
        decay: decay ?? this.decay,
        sustain: sustain ?? this.sustain,
        release: release ?? this.release,
        detuneCents: detuneCents ?? this.detuneCents,
        noiseAttack: noiseAttack ?? this.noiseAttack,
        brightnessFactor: brightnessFactor ?? this.brightnessFactor,
        synthEngine: synthEngine ?? this.synthEngine,
        envCurve: envCurve ?? this.envCurve,
        filterType: filterType ?? this.filterType,
        filterCutoff: filterCutoff ?? this.filterCutoff,
        filterResonance: filterResonance ?? this.filterResonance,
        filterEnvAmount: filterEnvAmount ?? this.filterEnvAmount,
        filterAttack: filterAttack ?? this.filterAttack,
        filterDecay: filterDecay ?? this.filterDecay,
        filterSustain: filterSustain ?? this.filterSustain,
        fmRatio: fmRatio ?? this.fmRatio,
        fmIndex: fmIndex ?? this.fmIndex,
        fmDecay: fmDecay ?? this.fmDecay,
        fmFeedback: fmFeedback ?? this.fmFeedback,
        morphRate: morphRate ?? this.morphRate,
      );
}


/// `dart:ffi` bindings for the Rust built-in effect suite's query surface (S5).
///
/// ## What this file is, and is not
///
/// This file owns the *effect-descriptor ABI*: what effects exist, what each
/// one's parameters are, and what ranges and units they use. It does **not**
/// implement any audio — every effect runs in Rust, and this file never sees a
/// sample.
///
/// That split is the whole point of PLAN §3.S5's "UI 自动生成": a settings panel
/// is built by asking the core to describe an effect, not by a hand-written
/// Dart class per effect. Adding an effect to `effects/registry.rs` makes it
/// appear here with **no Dart change at all**.
///
/// ## Why there is no handle
///
/// Unlike [AutomationBindings] and the mixer bindings, nothing here needs a
/// `create`/`destroy` pair. The built-in registry is a compile-time constant,
/// so the functions are stateless. [EffectBindings.latencySamples] is the one
/// call that instantiates anything, and it drops the instance before returning.
///
/// ## The allocation rule
///
/// Every pointer this file creates is freed before the call returns, using
/// `try`/`finally`, matching `automation_bindings.dart`.
///
/// ## Borrowed strings
///
/// Keys and labels come back as `Pointer<Utf8>` owned by Rust. They are read
/// into Dart `String`s immediately and the pointer is **never** freed here
/// (ABI §3.3, "Rust → Dart (borrow)").
///
/// ## Real-time note
///
/// Nothing here is safe for an audio callback; all of it is control-thread code.
library;

import 'dart:ffi';

import 'package:ffi/ffi.dart';

import '../native/zenith_core.dart';
import '../automation/native_types.dart';

// ── C signatures: enumeration ──

typedef _CountNative = Uint32 Function();
typedef _CountDart = int Function();

typedef _KindAtNative = Int32 Function(Uint32, Pointer<Uint32>);
typedef _KindAtDart = int Function(int, Pointer<Uint32>);

typedef _IsKnownNative = Uint32 Function(Uint32);
typedef _IsKnownDart = int Function(int);

typedef _IsBuiltinNative = Uint32 Function(Uint32);
typedef _IsBuiltinDart = int Function(int);

// ── C signatures: description ──

typedef _DescribeNative = Int32 Function(Uint32, Pointer<ZenithEffectDescriptor>);
typedef _DescribeDart = int Function(int, Pointer<ZenithEffectDescriptor>);

typedef _DescribeAtNative = Int32 Function(Uint32, Pointer<ZenithEffectDescriptor>);
typedef _DescribeAtDart = int Function(int, Pointer<ZenithEffectDescriptor>);

typedef _NameNative = Pointer<Utf8> Function(Uint32);
typedef _NameDart = Pointer<Utf8> Function(int);

typedef _CategoryNative = Uint32 Function(Uint32);
typedef _CategoryDart = int Function(int);

typedef _OversamplingNative = Uint32 Function(Uint32);
typedef _OversamplingDart = int Function(int);

// ── C signatures: parameters ──

typedef _ParamCountNative = Uint32 Function(Uint32);
typedef _ParamCountDart = int Function(int);

// ── C signatures: latency ──

typedef _LatencyNative = Int32 Function(
  Uint32, // kind
  Uint32, // sample_rate
  Uint32, // max_block
  Uint32, // channels
  Pointer<Uint32>, // out_samples
);
typedef _LatencyDart = int Function(int, int, int, int, Pointer<Uint32>);

// ── C signatures: constant mirrors ──

typedef _SizeofNative = Size Function();
typedef _SizeofDart = int Function();

typedef _U32GetterNative = Uint32 Function();
typedef _U32GetterDart = int Function();

/// One effect's identity and shape, decoded from the ABI into plain Dart.
///
/// The native struct is a borrowed, short-lived view; this is the owned value
/// the UI actually works with, so a caller never holds a pointer into native
/// memory.
final class EffectInfo {
  /// Creates an effect description.
  const EffectInfo({
    required this.kind,
    required this.key,
    required this.label,
    required this.category,
    required this.parameterCount,
    required this.firstParameter,
    required this.hasLatency,
    required this.isAnalysisOnly,
  });

  /// Effect kind id, as stored in a mixer effect slot.
  final int kind;

  /// Stable machine-readable key, e.g. `"compressor"`. Safe as a persistence
  /// key and as widget identity, because it is never localized.
  final String key;

  /// Human-readable label.
  final String label;

  /// Family; see [ZenithEffectCategory].
  final int category;

  /// How many parameters the effect publishes.
  final int parameterCount;

  /// Lower bound of this effect's parameter ordinals. `0` for the built-ins.
  final int firstParameter;

  /// Whether the effect introduces latency PDC must compensate.
  ///
  /// A hint for a UI badge. The authoritative value is
  /// [EffectBindings.latencySamples], which depends on the sample rate and the
  /// effect's current settings.
  final bool hasLatency;

  /// Whether the effect analyses audio and passes it through unchanged.
  final bool isAnalysisOnly;

  /// A display name for the effect's category, e.g. `'Dynamics'`.
  String get categoryName => ZenithEffectCategory.nameFor(category);

  @override
  String toString() => 'EffectInfo($key, kind=$kind, params=$parameterCount)';
}

/// Access to the built-in effect registry.
///
/// Stateless: every method resolves its symbol lazily and reads a compile-time
/// table in Rust. See [ZenithCore.isAvailable] for the "is the core present"
/// question — every method here throws [StateError] when it is not.
abstract final class EffectBindings {
  static DynamicLibrary _library() {
    final lib = ZenithCore.libraryOrNull();
    if (lib == null) {
      throw StateError(
        'zenith_core is not available — build the Rust core first '
        '(`cargo build` or `flutter build windows`).',
      );
    }
    return lib;
  }

  static _CountDart? _count;
  static _KindAtDart? _kindAt;
  static _IsKnownDart? _isKnown;
  static _IsBuiltinDart? _isBuiltin;
  static _DescribeDart? _describe;
  static _DescribeAtDart? _describeAt;
  static _NameDart? _name;
  static _CategoryDart? _category;
  static _OversamplingDart? _oversampling;
  static _ParamCountDart? _paramCount;
  static _LatencyDart? _latency;
  static _SizeofDart? _sizeof;
  static _U32GetterDart? _pluginKindBase;
  static _U32GetterDart? _analysisCategory;

  /// How many built-in effects the core knows about.
  ///
  /// The first half of the two-call protocol: ask for the count, then walk
  /// `0..count` with [kindAt] or [describeAt].
  static int count() {
    _count ??= _library().lookupFunction<_CountNative, _CountDart>(
      'zenith_effect_count',
    );
    return _count!();
  }

  /// The kind id at registry index [index], or `null` when out of range.
  static int? kindAt(int index) {
    final lib = _library();
    _kindAt ??= lib.lookupFunction<_KindAtNative, _KindAtDart>(
      'zenith_effect_kind_at',
    );
    final out = calloc<Uint32>();
    try {
      final status = _kindAt!(index, out);
      if (status != 0) return null;
      return out.value;
    } finally {
      calloc.free(out);
    }
  }

  /// Whether this build can instantiate [kind].
  ///
  /// Check this before storing a kind into a slot. The mixer holds a bare
  /// integer and silently bypasses an unknown kind, so a project referencing an
  /// effect from a newer build would otherwise load with a quietly empty slot —
  /// audible as "my effect vanished" with no error anywhere.
  static bool isKnown(int kind) {
    _isKnown ??= _library().lookupFunction<_IsKnownNative, _IsKnownDart>(
      'zenith_effect_is_known',
    );
    return _isKnown!(kind) == 1;
  }

  /// Whether [kind] lies in the built-in namespace, implemented or not.
  ///
  /// Distinct from [isKnown]: this answers "whose namespace is this?", which is
  /// what a loader needs in order to decide whether to look for a plugin
  /// (ABI §11 Q2).
  static bool isBuiltinKind(int kind) {
    _isBuiltin ??= _library().lookupFunction<_IsBuiltinNative, _IsBuiltinDart>(
      'zenith_effect_is_builtin_kind',
    );
    return _isBuiltin!(kind) == 1;
  }

  /// The lowest plugin kind id, read from the core rather than hard-coded.
  static int pluginKindBase() {
    _pluginKindBase ??= _library().lookupFunction<_U32GetterNative, _U32GetterDart>(
      'zenith_effect_plugin_kind_base',
    );
    return _pluginKindBase!();
  }

  /// The category discriminant reserved for analysis-only effects.
  static int analysisCategory() {
    _analysisCategory ??=
        _library().lookupFunction<_U32GetterNative, _U32GetterDart>(
      'zenith_effect_category_analysis',
    );
    return _analysisCategory!();
  }

  /// Describes the effect [kind], or `null` when the core does not know it.
  ///
  /// The returned strings are copied out of native memory before this returns,
  /// so the [EffectInfo] stays valid for as long as the caller holds it.
  static EffectInfo? describe(int kind) {
    final lib = _library();
    _describe ??= lib.lookupFunction<_DescribeNative, _DescribeDart>(
      'zenith_effect_describe',
    );
    final out = calloc<ZenithEffectDescriptor>();
    try {
      if (_describe!(kind, out) != 0) return null;
      return _decode(out);
    } finally {
      calloc.free(out);
    }
  }

  /// Describes the effect at registry index [index], or `null` when out of
  /// range.
  ///
  /// The index-based twin of [describe], so walking the whole registry takes
  /// one call per effect instead of two.
  static EffectInfo? describeAt(int index) {
    final lib = _library();
    _describeAt ??= lib.lookupFunction<_DescribeAtNative, _DescribeAtDart>(
      'zenith_effect_describe_at',
    );
    final out = calloc<ZenithEffectDescriptor>();
    try {
      if (_describeAt!(index, out) != 0) return null;
      return _decode(out);
    } finally {
      calloc.free(out);
    }
  }

  /// Copies a native descriptor into an owned [EffectInfo].
  static EffectInfo _decode(Pointer<ZenithEffectDescriptor> out) {
    final d = out.ref;
    return EffectInfo(
      kind: d.kind,
      key: d.keyUtf8 == nullptr ? '' : d.keyUtf8.toDartString(),
      label: d.labelUtf8 == nullptr ? '' : d.labelUtf8.toDartString(),
      category: d.category,
      parameterCount: d.paramCount,
      firstParameter: d.firstParam,
      hasLatency: d.hasLatency != 0,
      isAnalysisOnly: d.isAnalysisOnly != 0,
    );
  }

  /// Every effect the core publishes, in registry order.
  ///
  /// Malformed entries are skipped rather than throwing, so one bad entry does
  /// not blank the whole picker — but the caller gets no silent success either,
  /// because the list is simply shorter than [count] would suggest.
  static List<EffectInfo> all() {
    final total = count();
    final effects = <EffectInfo>[];
    for (var index = 0; index < total; index++) {
      final info = describeAt(index);
      if (info != null) effects.add(info);
    }
    return effects;
  }

  /// The effects grouped by category, ordered by [ZenithEffectCategory].
  ///
  /// This is what an effect picker renders: one section per family, in a stable
  /// order that does not depend on registry layout.
  static Map<int, List<EffectInfo>> grouped() {
    final grouped = <int, List<EffectInfo>>{};
    for (final effect in all()) {
      grouped.putIfAbsent(effect.category, () => <EffectInfo>[]).add(effect);
    }
    final ordered = <int, List<EffectInfo>>{};
    for (final category in grouped.keys.toList()
      ..sort((a, b) => ZenithEffectCategory.orderFor(a)
          .compareTo(ZenithEffectCategory.orderFor(b)))) {
      ordered[category] = grouped[category]!;
    }
    return ordered;
  }

  /// The human-readable name of [kind], or `null` when unknown.
  ///
  /// Kept for the case where only an id is available, e.g. a project file
  /// referencing an effect. Prefer [describe] when the full record is wanted.
  static String? name(int kind) {
    _name ??= _library().lookupFunction<_NameNative, _NameDart>(
      'zenith_effect_name',
    );
    final pointer = _name!(kind);
    if (pointer == nullptr) return null;
    return pointer.toDartString();
  }

  /// The category of [kind], or the utility category when unknown.
  static int category(int kind) {
    _category ??= _library().lookupFunction<_CategoryNative, _CategoryDart>(
      'zenith_effect_category',
    );
    return _category!(kind);
  }

  /// The oversampling factor [kind] uses internally, for a quality badge.
  ///
  /// `1` means the effect does not oversample.
  static int oversampling(int kind) {
    _oversampling ??=
        _library().lookupFunction<_OversamplingNative, _OversamplingDart>(
      'zenith_effect_oversampling',
    );
    return _oversampling!(kind);
  }

  /// How many parameters [kind] publishes, or `0` when unknown.
  ///
  /// The static half of the parameter API. The descriptors themselves come from
  /// a live instance, because each one carries an automation address that
  /// depends on which slot the effect occupies.
  static int parameterCount(int kind) {
    _paramCount ??= _library().lookupFunction<_ParamCountNative, _ParamCountDart>(
      'zenith_effect_parameter_count',
    );
    return _paramCount!(kind);
  }

  /// Samples of latency [kind] introduces at the given settings.
  ///
  /// This is the authoritative PDC figure (PLAN §3.S4 requirement 1). A wrong
  /// value does not merely misreport the effect — it misaligns **every other
  /// track**, because the compensation shifts them by what this returns.
  ///
  /// Returns `null` when the core does not know [kind]. The core substitutes
  /// sane defaults for a zero sample rate or block size, so this is safe to
  /// call before the audio device is open.
  static int? latencySamples(
    int kind, {
    int sampleRate = 48000,
    int maxBlock = 256,
    int channels = 2,
  }) {
    final lib = _library();
    _latency ??= lib.lookupFunction<_LatencyNative, _LatencyDart>(
      'zenith_effect_latency_samples',
    );
    final out = calloc<Uint32>();
    try {
      final status = _latency!(kind, sampleRate, maxBlock, channels, out);
      if (status != 0) return null;
      return out.value;
    } finally {
      calloc.free(out);
    }
  }

  /// The core's own `sizeof(ZenithEffectDescriptor)`.
  ///
  /// [verifyStructLayout] compares this with Dart's `sizeOf`. The struct holds
  /// pointers, so its size legitimately differs between 64-bit and `wasm32`;
  /// taking it from the core is what keeps both correct.
  static int nativeDescriptorSize() {
    _sizeof ??= _library().lookupFunction<_SizeofNative, _SizeofDart>(
      'zenith_sizeof_effect_descriptor_checked',
    );
    return _sizeof!();
  }

  /// Throws [StateError] when Dart and Rust disagree on the descriptor layout.
  ///
  /// Called once at startup. Without it a layout drift would not crash — it
  /// would silently reinterpret native memory and produce wrong parameter
  /// ranges, which is far harder to diagnose than a loud failure (ABI §2.3).
  static void verifyStructLayout() {
    final nativeSize = nativeDescriptorSize();
    final dartSize = sizeOf<ZenithEffectDescriptor>();
    if (nativeSize != dartSize) {
      throw StateError(
        'ZenithEffectDescriptor layout mismatch: Rust reports $nativeSize '
        'bytes, Dart computes $dartSize. Update native_types.dart and the Rust '
        'struct together.',
      );
    }
  }
}

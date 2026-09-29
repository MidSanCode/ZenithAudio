/// Automation parameter identity and storage.
///
/// S0 defines the vocabulary only. S2 makes the Rust side authoritative and
/// turns [ParameterStore] into an FFI-backed view over lock-free atomics;
/// keeping the interface here means the UI layer can be written against a
/// stable contract before that happens.
///
/// ## Why ids are not Dart objects
///
/// A [ParameterId] is an interned *string* pair rather than a class instance,
/// because the same identity must round-trip through the C ABI, the project
/// file and the plugin host. A Dart object would need a parallel encoding at
/// every one of those boundaries.
library;

/// Identifies one automatable parameter.
///
/// The pair is `(ownerId, parameterKey)`:
/// * `ownerId` — the track id, mixer channel id, or plugin instance id that
///   owns the parameter.
/// * `parameterKey` — the stable, human-readable key within that owner, e.g.
///   `"volume"`, `"pan"`, `"filter.cutoff"`.
///
/// Keys are dotted for grouping and are never localized — they are an ABI, not
/// a label. The display name comes from the parameter descriptor.
class ParameterId {
  /// Creates a parameter id.
  const ParameterId(this.ownerId, this.parameterKey);

  /// Owner of the parameter (track, mixer channel, or plugin instance).
  final String ownerId;

  /// Stable key of the parameter within its owner.
  final String parameterKey;

  /// Encoded form used at the FFI and project-file boundaries.
  ///
  /// Uses `:` as the separator because owner ids are UUIDs and parameter keys
  /// are dotted — neither can contain a colon, so the encoding is unambiguous
  /// and needs no escaping.
  String get encoded => '$ownerId:$parameterKey';

  /// Parses an [encoded] id.
  ///
  /// Returns `null` for a malformed string instead of throwing: this parses
  /// data read from a project file, where a corrupt id must degrade rather
  /// than abort a project load.
  static ParameterId? tryParse(String encoded) {
    final split = encoded.indexOf(':');
    if (split <= 0 || split == encoded.length - 1) return null;
    return ParameterId(encoded.substring(0, split), encoded.substring(split + 1));
  }

  @override
  bool operator ==(Object other) =>
      other is ParameterId &&
      other.ownerId == ownerId &&
      other.parameterKey == parameterKey;

  @override
  int get hashCode => Object.hash(ownerId, parameterKey);

  @override
  String toString() => encoded;
}

/// How a parameter behaves when automated.
enum ParameterKind {
  /// Stepped integer choices; `min`/`max` bound the choice index.
  discrete,

  /// Continuous value with a meaningful midpoint.
  continuous,

  /// Multiplicative value (gain, frequency); interpolation is exponential so a
  /// sweep sounds linear.
  exponential,

  /// Two-valued on/off parameter.
  toggle,
}

/// A parameter's static description.
///
/// Produced by the Rust side in S2+ and consumed by the UI to build controls
/// generically. S0 defines it so the UI never hardcodes a parameter list.
class ParameterDescriptor {
  /// Creates a descriptor.
  const ParameterDescriptor({
    required this.id,
    required this.label,
    required this.kind,
    required this.minValue,
    required this.maxValue,
    required this.defaultValue,
    this.unit,
    this.stepCount,
  });

  /// The parameter this describes.
  final ParameterId id;

  /// Human-readable label. Localized by the caller, not stored localized.
  final String label;

  /// Automation behaviour.
  final ParameterKind kind;

  /// Lowest legal value, in the parameter's own units.
  final double minValue;

  /// Highest legal value, in the parameter's own units.
  final double maxValue;

  /// Value used when a project does not specify one.
  final double defaultValue;

  /// Display unit, e.g. `"dB"` or `"Hz"`. `null` for unitless parameters.
  final String? unit;

  /// Number of distinct values for [ParameterKind.discrete].
  final int? stepCount;

  /// Clamps [value] into the legal range.
  double clamp(double value) =>
      value < minValue ? minValue : (value > maxValue ? maxValue : value);
}

/// A parameter value at a point in time, in project frames.
///
/// Frames rather than seconds, matching the tick-first model: automation must
/// not drift when the tempo changes.
class AutomationPoint {
  /// Creates an automation point.
  const AutomationPoint({required this.frame, required this.value});

  /// Position, in frames from the project origin.
  final int frame;

  /// Value at [frame].
  final double value;
}

/// Read/write access to parameter values and their automation.
///
/// The Rust core is authoritative from S2 onward; this interface is what the
/// Dart side sees of it. Reads are expected to be lock-free and safe from any
/// isolate.
abstract interface class ParameterStore {
  /// Describes every known parameter.
  ///
  /// Used to build UI generically, so a new Rust-side parameter needs no Dart
  /// change.
  List<ParameterDescriptor> get descriptors;

  /// Looks up a descriptor, or `null` when the id is unknown.
  ParameterDescriptor? descriptorOf(ParameterId id);

  /// Reads the current value, or `null` when the id is unknown.
  double? read(ParameterId id);

  /// Sets the current value.
  ///
  /// Returns `false` when the id is unknown or the value was clamped away
  /// entirely; a successful clamp still returns `true` because the write
  /// landed.
  bool write(ParameterId id, double value);

  /// The automation curve for a parameter, in frame order.
  List<AutomationPoint> automationOf(ParameterId id);

  /// Replaces a parameter's automation.
  void setAutomation(ParameterId id, List<AutomationPoint> points);

  /// Evaluates a parameter at [frame], including automation.
  ///
  /// This is the single source of truth for "what is this parameter doing
  /// right now" — callers must not interpolate on their own.
  double valueAt(ParameterId id, int frame);
}

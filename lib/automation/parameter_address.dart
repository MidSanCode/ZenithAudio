/// Parameter addressing: the Dart-side mirror of the compact C address triple.
///
/// ## The problem this solves
///
/// The UI wants to talk about parameters by name (`channel/<uuid>/volume`),
/// because that is what a project file stores and what a human reads. The audio
/// thread wants an 8-byte integer, because a string hash on the evaluation path
/// is both slow and a licence to allocate.
///
/// [ParameterAddress] is the bridge. A name is resolved to a triple **once**,
/// when a lane is created or a control is built, and every later operation —
/// including every per-block read and write — uses the integer.
///
/// ## Why not just hash the string
///
/// Because a hash collision would silently cross two parameters, and because
/// hashing allocates in some implementations. The triple is assigned by
/// *registration*: the Rust registry decides which `sub` index a key occupies,
/// and Dart never invents one. That makes the mapping total and collision-free
/// by construction.
library;

import 'dart:ffi';
import 'dart:math' as math;

import 'package:ffi/ffi.dart';

/// Which kind of object owns a parameter.
///
/// Discriminants are ABI-frozen (`docs/ABI.md` §3.2): a value must never be
/// reassigned, only appended to. They mirror `automation::ParameterKind` and
/// `zenith_param_kind` in the Rust core.
enum ParameterOwnerKind {
  /// Engine-wide parameter: tempo, master gain, time signature.
  global(0, 'global'),

  /// A mixer channel strip parameter: fader, pan, mute, sends.
  channel(1, 'channel'),

  /// A track parameter: monitoring, record arm, transpose.
  track(2, 'track'),

  /// An effect slot parameter: any control of an effect in a slot.
  effect(3, 'effect'),

  /// A modulation source's own settings: LFO rate, envelope times.
  modulator(4, 'modulator');

  const ParameterOwnerKind(this.wire, this.segment);

  /// The `kind` discriminant sent across the ABI.
  final int wire;

  /// The path segment used in a human-readable address.
  final String segment;

  /// Looks up a kind by its wire value, or `null` when unknown.
  ///
  /// Returning `null` rather than throwing is deliberate: a newer Rust core may
  /// send a kind this build has never heard of, and the right response is to
  /// ignore that parameter, not to crash the UI thread (ABI §2.2).
  static ParameterOwnerKind? fromWire(int wire) {
    for (final kind in ParameterOwnerKind.values) {
      if (kind.wire == wire) return kind;
    }
    return null;
  }

  /// Looks up a kind by its path segment, or `null` when unknown.
  static ParameterOwnerKind? fromSegment(String segment) {
    for (final kind in ParameterOwnerKind.values) {
      if (kind.segment == segment) return kind;
    }
    return null;
  }
}

/// A resolved, hash-free parameter address.
///
/// Eight bytes on the wire, laid out exactly as `ZenithParamId` in
/// `native/zenith_core/src/ffi/types.rs`.
///
/// Equality and [hashCode] are defined on the **triple**, not on a string, so
/// using an address as a map key costs no string work — which matters because
/// the automation editor holds one per lane and rebuilds its index on every
/// repaint.
final class ParameterAddress {
  /// Creates an address from its parts.
  const ParameterAddress({
    required this.kind,
    required this.index,
    required this.sub,
  });

  /// Creates an engine-wide address (`kind = global`, `index` ignored).
  const ParameterAddress.global(this.sub)
      : kind = ParameterOwnerKind.global,
        index = 0;

  /// Creates a mixer channel address.
  const ParameterAddress.channel(int channelIndex, this.sub)
      : kind = ParameterOwnerKind.channel,
        index = channelIndex;

  /// Creates a track address.
  const ParameterAddress.track(int trackIndex, this.sub)
      : kind = ParameterOwnerKind.track,
        index = trackIndex;

  /// Creates an effect slot address.
  ///
  /// `slot` is the index of the effect slot on its channel; `sub` selects a
  /// control within the effect.
  const ParameterAddress.effect(int channelIndex, int slot, this.sub)
      : kind = ParameterOwnerKind.effect,
        index = (channelIndex << 8) | (slot & 0xFF);

  /// Owner category.
  final ParameterOwnerKind kind;

  /// Index of the owning object.
  ///
  /// For [ParameterOwnerKind.effect] this packs the channel in the high bits
  /// and the slot in the low 8, so an effect address stays a single `u32`
  /// across the ABI rather than needing a fourth field.
  final int index;

  /// Ordinal of the parameter within its owner.
  ///
  /// Assigned by the Rust registry at registration time. Dart never invents
  /// one — see the library doc comment.
  final int sub;

  /// The channel an effect address belongs to.
  ///
  /// Only meaningful for [ParameterOwnerKind.effect]; other kinds treat
  /// [index] as the channel index directly.
  int get channelIndex =>
      kind == ParameterOwnerKind.effect ? (index >> 8) : index;

  /// The effect slot within [channelIndex].
  int get effectSlot => kind == ParameterOwnerKind.effect ? (index & 0xFF) : 0;

  /// Whether this build understands [kind] (always true for a Dart-built
  /// address; the useful direction is [tryFromNative], which can fail).
  bool get isKnown => true;

  /// The compact 64-bit key the Rust store binary-searches on.
  ///
  /// Mirrors `ParameterAddress::key()` exactly: `kind << 48 | index << 16 |
  /// sub`. Kept in Dart so the automation editor can sort lanes and detect
  /// duplicates without a round trip.
  int get key => (kind.wire << 48) | ((index & 0xFFFFFFFF) << 16) | (sub & 0xFFFF);

  @override
  bool operator ==(Object other) =>
      other is ParameterAddress &&
      other.kind == kind &&
      other.index == index &&
      other.sub == sub;

  @override
  int get hashCode => Object.hash(kind, index, sub);

  @override
  String toString() {
    switch (kind) {
      case ParameterOwnerKind.global:
        return 'global/$sub';
      case ParameterOwnerKind.channel:
        return 'channel/$index/$sub';
      case ParameterOwnerKind.track:
        return 'track/$index/$sub';
      case ParameterOwnerKind.effect:
        return 'effect/$channelIndex/$effectSlot/$sub';
      case ParameterOwnerKind.modulator:
        return 'modulator/$index/$sub';
    }
  }

  /// Writes this address into a caller-provided native struct.
  ///
  /// Reuses [out] to avoid an allocation on a per-frame path; the caller owns
  /// the buffer and is responsible for freeing it.
  void writeTo(Pointer<ZenithParamId> out) {
    final ref = out.ref;
    ref.kind = kind.wire;
    ref.sub = sub;
    ref.index = index;
  }

  /// Allocates a native [ZenithParamId] on the C heap.
  ///
  /// The caller **must** free it with `calloc.free` — this is the
  /// allocate-and-forget form, appropriate for one-shot calls, not for a
  /// per-frame loop.
  Pointer<ZenithParamId> toNative() {
    final pointer = calloc<ZenithParamId>();
    writeTo(pointer);
    return pointer;
  }

  /// Reads an address out of a native struct.
  static ParameterAddress fromNative(ZenithParamId native) =>
      ParameterAddress(
        kind: ParameterOwnerKind.fromWire(native.kind) ??
            ParameterOwnerKind.global,
        index: native.index,
        sub: native.sub,
      );

  /// Reads an address, reporting `null` for a kind this build does not know.
  ///
  /// This is the safe direction: an unknown kind means a newer core, and the
  /// caller should skip that parameter rather than mis-address a different one.
  static ParameterAddress? tryFromNative(ZenithParamId native) {
    final kind = ParameterOwnerKind.fromWire(native.kind);
    if (kind == null) return null;
    return ParameterAddress(kind: kind, index: native.index, sub: native.sub);
  }
}

/// The `ZenithParamId` struct, mirroring `ffi/types.rs`.
///
/// Field order and width must match the Rust definition exactly; the sizes are
/// asserted by `parameter_address_test.dart` against the values the core
/// reports through `zenith_sizeof_param_id()`.
final class ZenithParamId extends Struct {
  /// Parameter category; see [ParameterOwnerKind].
  @Uint16()
  external int kind;

  /// Ordinal within the owner.
  @Uint16()
  external int sub;

  /// Index of the owning object.
  @Uint32()
  external int index;
}

/// A parameter value at a point in time, with interpolation metadata.
///
/// Supersedes the S0 `AutomationPoint` by adding the interpolation mode and
/// curvature. S2's Rust core stores these three fields per point so a curve can
/// mix straight and shaped segments within one lane, which is what makes a
/// single automation lane expressive enough to draw both a fader ride and a
/// filter sweep.
final class AutomationPointV2 {
  /// Creates a point.
  const AutomationPointV2({
    required this.frame,
    required this.value,
    this.tension = 0,
    this.curve = AutomationCurve.linear,
  });

  /// Position, in frames from the project origin.
  final int frame;

  /// Value at [frame].
  final double value;

  /// Curvature toward the *next* point, `-1.0..=1.0`.
  ///
  /// Positive bends above the straight line, negative below. Ignored unless
  /// [curve] is [AutomationCurve.curve].
  final double tension;

  /// How the segment to the next point is interpolated.
  final AutomationCurve curve;

  /// Returns a copy with the given fields replaced.
  AutomationPointV2 copyWith({
    int? frame,
    double? value,
    double? tension,
    AutomationCurve? curve,
  }) =>
      AutomationPointV2(
        frame: frame ?? this.frame,
        value: value ?? this.value,
        tension: tension ?? this.tension,
        curve: curve ?? this.curve,
      );

  /// Evaluates the segment from this point to [next] at [frame].
  ///
  /// This exists so the editor can draw and hit-test a curve without asking the
  /// core per pixel. It must agree with the Rust implementation — the shared
  /// test vectors in `parameter_address_test.dart` are what keep them honest,
  /// because a mismatch would show as "the curve I drew is not the curve I
  /// hear".
  ///
  /// [frame] is clamped to the segment, so this is total for any input.
  double valueAt(int frame, AutomationPointV2 next) {
    final span = next.frame - this.frame;
    if (span <= 0) return next.value;
    final t = ((frame - this.frame) / span).clamp(0.0, 1.0);
    switch (curve) {
      case AutomationCurve.hold:
        return value;
      case AutomationCurve.linear:
        return value + (next.value - value) * t;
      case AutomationCurve.curve:
        return value + (next.value - value) * _shaped(t);
      case AutomationCurve.exponential:
        return value + (next.value - value) * _exponential(t, this, next);
      case AutomationCurve.logarithmic:
        return value + (next.value - value) * _exponential(t, next, this);
    }
  }

  /// Applies this point's tension to a normalized position.
  ///
  /// Mirrors `clip::shape_tension`: a monotone power curve, so the output is
  /// always within `0..1` and the segment's endpoints stay exact. An earlier
  /// biased-divisor formulation passed through a pole and produced values
  /// outside the range at high tension — audible as a click.
  double _shaped(double t) {
    if (tension == 0) return t;
    final gamma = _expFromTension(-tension);
    return _pow(t, gamma).clamp(0.0, 1.0);
  }

  /// Exponential/geometric interpolation, falling back to linear across zero.
  ///
  /// A geometric blend is undefined when the two values straddle zero (there is
  /// no real root), so the linear form is used instead of producing NaN — the
  /// same guard the Rust side applies.
  static double _exponential(
    double t,
    AutomationPointV2 from,
    AutomationPointV2 to,
  ) {
    final a = from.value.abs();
    final b = to.value.abs();
    if (a <= 1e-12 || b <= 1e-12) {
      // Degenerate: one end is silence, so a ratio is meaningless.
      return t;
    }
    if ((from.value < 0) != (to.value < 0)) return t;
    return (_pow(a, 1 - t) * _pow(b, t)) / a;
  }

  /// `2^(x * 3)`, mirroring `clip::exp_from_tension`.
  static double _expFromTension(double x) => math.pow(2.0, x * 3.0).toDouble();

  /// `pow`, narrowed to `double`.
  ///
  /// `dart:math`'s `pow` returns `num` because an integral base and exponent
  /// yields an `int`; this call site always wants a `double`, and doing the
  /// conversion in one place keeps the arithmetic above readable.
  static double _pow(double base, double exponent) =>
      math.pow(base, exponent).toDouble();

  @override
  String toString() =>
      'AutomationPointV2(frame: $frame, value: $value, '
      'tension: $tension, curve: ${curve.name})';
}

/// Interpolation mode for the segment leaving a point.
///
/// Discriminants mirror `automation::clip::CurveKind` and `zenith_curve_kind`.
enum AutomationCurve {
  /// Straight line.
  linear(0),

  /// Step: hold this point's value until the next.
  hold(1),

  /// Shaped by [AutomationPointV2.tension].
  curve(2),

  /// Fast-then-flat, for a natural-feeling decay.
  exponential(3),

  /// Slow-then-steep, for a natural-feeling attack.
  logarithmic(4);

  const AutomationCurve(this.wire);

  /// The discriminant sent across the ABI.
  final int wire;

  /// Looks up a curve by its wire value, or `null` when unknown.
  static AutomationCurve? fromWire(int wire) {
    for (final curve in AutomationCurve.values) {
      if (curve.wire == wire) return curve;
    }
    return null;
  }
}

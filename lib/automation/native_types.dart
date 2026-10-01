/// `#[repr(C)]` struct mirrors for the S2 parameter and automation ABI.
///
/// ## The rule
///
/// Each class here mirrors one struct in
/// `native/zenith_core/src/ffi/types.rs`, field for field and in the same
/// order. Dart resolves `Struct` fields by declaration order, so **reordering a
/// field here silently reinterprets native memory** — it will not fail to
/// compile, it will produce wrong numbers.
///
/// Two guards exist so that cannot go unnoticed:
///
/// 1. `nativeTypesTest.dart` asserts each `sizeOf<T>()` against the value the
///    core reports through `zenith_sizeof_*` (ABI §2.3);
/// 2. the Rust side asserts the same numbers against hard-coded literals, so a
///    change on one side fails that side's own test rather than drifting.
///
/// Adding a field is therefore a three-place change: this file, the Rust
/// struct, and both size assertions.
library;

import 'dart:ffi';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import 'parameter_address.dart';
/// Parameter unit discriminants, mirroring `zenith_param_unit`.
///
/// Held as raw ints rather than an enum because a newer core may send a unit
/// this build has never seen; the UI shows a formatted value and must degrade
/// to a plain number rather than throw.
abstract final class ZenithParamUnit {
  /// Plain number.
  static const int linear = 0;

  /// Decibels.
  static const int decibels = 1;

  /// Hertz.
  static const int hertz = 2;

  /// Seconds.
  static const int seconds = 3;

  /// Percent, 0..100.
  static const int percent = 4;

  /// A choice among discrete options.
  static const int enumeration = 5;

  /// Beats, for tempo-synced values.
  static const int beats = 6;

  /// A short display suffix for a unit, or an empty string when unitless.
  ///
  /// An unknown unit yields `''` rather than a placeholder: showing a wrong
  /// suffix is worse than showing none.
  static String suffixFor(int unit) {
    switch (unit) {
      case decibels:
        return ' dB';
      case hertz:
        return ' Hz';
      case seconds:
        return ' s';
      case percent:
        return ' %';
      case beats:
        return ' beats';
      default:
        return '';
    }
  }
}

/// Parameter behaviour flag bits, mirroring `zenith_param_flags`.
abstract final class ZenithParamFlags {
  /// The parameter can carry an automation lane.
  static const int automatable = 0x01;

  /// Values are discrete steps.
  static const int discrete = 0x02;

  /// Display on a logarithmic scale.
  static const int logarithmic = 0x04;

  /// The range is bipolar around zero.
  static const int bipolar = 0x08;

  /// Apply the default one-pole smoothing.
  static const int smoothed = 0x10;
}

/// The `ZenithParamDescriptor` struct, mirroring `ffi/types.rs`.
///
/// 48 bytes on a 64-bit target (40 on 32-bit), because of the two pointers.
final class ZenithParamDescriptor extends Struct {
  /// The parameter's compact address.
  external ZenithParamId id;

  /// Lowest legal value.
  @Float()
  external double minValue;

  /// Highest legal value.
  @Float()
  external double maxValue;

  /// Value used when nothing else supplies one.
  @Float()
  external double defaultValue;

  /// Default smoothing time, in milliseconds.
  @Float()
  external double smoothingMs;

  /// Display unit; see [ZenithParamUnit].
  @Uint32()
  external int unit;

  /// Behaviour bits; see [ZenithParamFlags].
  @Uint32()
  external int flags;

  /// Stable machine key. Borrowed from Rust; read only, never freed.
  external Pointer<Utf8> keyUtf8;

  /// Human-readable label. Borrowed from Rust; read only, never freed.
  external Pointer<Utf8> labelUtf8;
}

/// The `ZenithAutomationPoint` struct, mirroring `ffi/types.rs`.
///
/// 24 bytes. The trailing `_reserved` field exists so the struct's size is
/// unambiguous and identical on every target, rather than depending on how a
/// particular compiler pads `f32` after `i64`.
final class ZenithAutomationPoint extends Struct {
  /// Position, in frames from the project origin.
  @Int64()
  external int frame;

  /// Value at [frame].
  @Float()
  external double value;

  /// Curvature toward the next point, `-1.0..=1.0`.
  @Float()
  external double tension;

  /// Interpolation to the next point; see `AutomationCurve`.
  @Uint32()
  external int curve;

  /// Explicit padding, held at zero.
  @Uint32()
  external int reserved;
}

/// The `ZenithAutomationStats` struct, mirroring `ffi/types.rs`.
final class ZenithAutomationStats extends Struct {
  /// Parameters evaluated in the block.
  @Uint32()
  external int evaluated;

  /// Parameters that had a lane contributing.
  @Uint32()
  external int automated;

  /// Parameters that had at least one modulator contributing.
  @Uint32()
  external int modulated;

  /// Parameters written because their value changed.
  @Uint32()
  external int written;

  /// Parameters dropped at the per-block capacity.
  @Uint32()
  external int skipped;

  /// Lane addresses that were not registered in the store.
  @Uint32()
  external int unresolved;

  /// Explicit padding, held at zero.
  @Uint32()
  external int reserved;
}

/// The `ZenithLaneState` struct, mirroring `ffi/types.rs`.
final class ZenithLaneState extends Struct {
  /// The lane's parameter.
  external ZenithParamId id;

  /// Number of points in the lane.
  @Uint32()
  external int pointCount;

  /// Whether the lane plays back.
  @Uint8()
  external int enabled;

  /// Whether the lane accepts recorded takes.
  @Uint8()
  external int armed;

  /// Whether the lane is collapsed in the editor.
  @Uint8()
  external int collapsed;

  /// Explicit padding, held at zero.
  @Uint8()
  external int reserved0;

  /// First frame covered, or 0 when the lane is empty.
  @Int64()
  external int firstFrame;

  /// Last frame covered, or 0 when the lane is empty.
  @Int64()
  external int lastFrame;

  /// Lane colour as `0xRRGGBB`; `0` means the theme default.
  @Uint32()
  external int color;

  /// Editor height in logical pixels; `0` means the default.
  @Float()
  external double height;
}

/// The `ZenithRecorderState` struct, mirroring `ffi/types.rs`.
final class ZenithRecorderState extends Struct {
  /// Whether global recording is enabled.
  @Uint8()
  external int enabled;

  /// Whether a take is currently open.
  @Uint8()
  external int takeOpen;

  /// Explicit padding, held at zero.
  @Uint16()
  external int reserved0;

  /// The active mode; see `RecordMode`.
  @Uint32()
  external int mode;

  /// Parameter being recorded, when [takeOpen] is set.
  external ZenithParamId activeId;

  /// Points captured in the current take.
  @Uint32()
  external int takePoints;

  /// Points captured since the recorder was created.
  @Uint32()
  external int totalCaptured;

  /// Explicit padding, held at zero.
  @Uint32()
  external int reserved1;
}

/// A decoded parameter descriptor.
///
/// The Rust struct holds borrowed string pointers; reading them into Dart
/// strings immediately is deliberate. Holding the pointer would pin native
/// memory for as long as any UI object lived, and the strings never change
/// while the handle is open.
final class NativeParameterDescriptor {
  /// Creates a decoded descriptor.
  const NativeParameterDescriptor({
    required this.address,
    required this.key,
    required this.label,
    required this.unit,
    required this.flags,
    required this.minValue,
    required this.maxValue,
    required this.defaultValue,
    required this.smoothingMs,
  });

  /// Decodes one native descriptor.
  ///
  /// A null string pointer decodes to an empty string rather than throwing: a
  /// missing label is a cosmetic problem and must not abort a UI build.
  ///
  /// Public because the binding layer lives in a sibling library; Dart's `_`
  /// privacy is per-library, so a private constructor here would be
  /// unreachable from `automation_bindings.dart`.
  factory NativeParameterDescriptor.fromNative(ZenithParamDescriptor ref) {
    final address = ParameterAddress.tryFromNative(ref.id);
    return NativeParameterDescriptor(
      address: address ??
          const ParameterAddress(
            kind: ParameterOwnerKind.global,
            index: 0,
            sub: 0,
          ),
      key: ref.keyUtf8 == nullptr ? '' : ref.keyUtf8.toDartString(),
      label: ref.labelUtf8 == nullptr ? '' : ref.labelUtf8.toDartString(),
      unit: ref.unit,
      flags: ref.flags,
      minValue: ref.minValue,
      maxValue: ref.maxValue,
      defaultValue: ref.defaultValue,
      smoothingMs: ref.smoothingMs,
    );
  }

  /// The parameter's resolved address.
  final ParameterAddress address;

  /// Stable machine key, e.g. `"volume"`.
  final String key;

  /// Human-readable label.
  final String label;

  /// Display unit; see [ZenithParamUnit].
  final int unit;

  /// Behaviour bits; see [ZenithParamFlags].
  final int flags;

  /// Lowest legal value.
  final double minValue;

  /// Highest legal value.
  final double maxValue;

  /// Value used when nothing else supplies one.
  final double defaultValue;

  /// Default smoothing time, in milliseconds.
  final double smoothingMs;

  /// Whether the parameter can carry an automation lane.
  bool get isAutomatable => (flags & ZenithParamFlags.automatable) != 0;

  /// Whether values are discrete steps.
  bool get isDiscrete => (flags & ZenithParamFlags.discrete) != 0;

  /// Whether the range is bipolar around zero.
  bool get isBipolar => (flags & ZenithParamFlags.bipolar) != 0;

  /// Whether the display should use a logarithmic scale.
  bool get isLogarithmic => (flags & ZenithParamFlags.logarithmic) != 0;

  /// A display suffix for the unit, or an empty string.
  String get unitSuffix => ZenithParamUnit.suffixFor(unit);

  /// Clamps [value] into the legal range.
  double clamp(double value) =>
      value < minValue ? minValue : (value > maxValue ? maxValue : value);

  /// Formats [value] with its unit, for a tooltip or readout.
  String format(double value) {
    // Discrete parameters read as an index, and a fractional display would be
    // noise; continuous ones get two decimals, which is what a fader readout
    // needs and no more.
    final text = isDiscrete
        ? value.round().toString()
        : value.toStringAsFixed(2);
    return '$text$unitSuffix';
  }
}

/// A decoded lane state.
final class NativeLaneState {
  /// Creates a decoded lane state.
  const NativeLaneState({
    required this.address,
    required this.pointCount,
    required this.enabled,
    required this.armed,
    required this.collapsed,
    required this.firstFrame,
    required this.lastFrame,
    required this.color,
    required this.height,
  });

  /// Decodes one native lane state.
  ///
  /// Public for the same reason as [NativeParameterDescriptor.fromNative].
  factory NativeLaneState.fromNative(ZenithLaneState ref) => NativeLaneState(
        address: ParameterAddress.tryFromNative(ref.id) ??
            const ParameterAddress(
              kind: ParameterOwnerKind.global,
              index: 0,
              sub: 0,
            ),
        pointCount: ref.pointCount,
        enabled: ref.enabled != 0,
        armed: ref.armed != 0,
        collapsed: ref.collapsed != 0,
        firstFrame: ref.firstFrame,
        lastFrame: ref.lastFrame,
        color: ref.color,
        height: ref.height,
      );

  /// The lane's parameter.
  final ParameterAddress address;

  /// Number of points in the lane.
  final int pointCount;

  /// Whether the lane plays back.
  final bool enabled;

  /// Whether the lane accepts recorded takes.
  final bool armed;

  /// Whether the lane is collapsed in the editor.
  final bool collapsed;

  /// First frame covered.
  final int firstFrame;

  /// Last frame covered.
  final int lastFrame;

  /// Lane colour as `0xRRGGBB`; `0` means the theme default.
  final int color;

  /// Editor height in logical pixels; `0` means the default.
  final double height;

  /// Whether the lane has no points.
  bool get isEmpty => pointCount == 0;

  /// The lane's frame span, or `null` when it is empty.
  (int, int)? get frameRange => isEmpty ? null : (firstFrame, lastFrame);
}

/// An `[S2]` struct-size probe: `sizeof` values the Dart mirrors must match.
///
/// Returned as a plain record so the test can diff two named numbers and print
/// a useful failure, rather than comparing anonymous integers.
typedef DartStructSizes = ({
  int paramId,
  int paramDescriptor,
  int automationPoint,
  int automationStats,
  int laneState,
  int recorderState,
});

/// The sizes Dart believes its mirrors have.
///
/// Read from `sizeOf`, never hard-coded: the point is to compare Dart's idea
/// against the core's, and a literal here would just be a second guess.
DartStructSizes dartStructSizes() => (
      paramId: sizeOf<ZenithParamId>(),
      paramDescriptor: sizeOf<ZenithParamDescriptor>(),
      automationPoint: sizeOf<ZenithAutomationPoint>(),
      automationStats: sizeOf<ZenithAutomationStats>(),
      laneState: sizeOf<ZenithLaneState>(),
      recorderState: sizeOf<ZenithRecorderState>(),
    );

/// Byte offsets recovered from a live struct instance, per struct.
///
/// A size check alone would pass with two fields of equal width swapped, and a
/// swap silently reinterprets native memory — it compiles, it runs, and it
/// produces wrong numbers. Offsets are what actually pin a `#[repr(C)]` layout,
/// so they are asserted by `parameter_address_test.dart` as well as here.
typedef DartFieldOffsets = ({
  Map<String, int> automationPoint,
  Map<String, int> laneState,
  Map<String, int> recorderState,
});

/// Returns the byte offset of each named field, measured from a live instance.
///
/// This SDK's `dart:ffi` exposes `sizeOf` but not `offsetOf`, so each field is
/// located by writing a distinctive value through its declared accessor and
/// finding where those bytes land in the struct's image. Probing through the
/// accessor (rather than asking for metadata) has the side benefit of
/// exercising exactly the annotations the binding layer relies on.
DartFieldOffsets dartFieldOffsets() {
  final point = calloc<ZenithAutomationPoint>();
  final lane = calloc<ZenithLaneState>();
  final recorder = calloc<ZenithRecorderState>();
  try {
    return (
      automationPoint: {
        'frame': _offsetOf(
          point,
          sizeOf<ZenithAutomationPoint>(),
          _leBytes(0x1122334455667788, 8),
          () => point.ref.frame = 0x1122334455667788,
        ),
        'value': _offsetOf(
          point,
          sizeOf<ZenithAutomationPoint>(),
          _f32Bytes(0.75),
          () => point.ref.value = 0.75,
        ),
        'tension': _offsetOf(
          point,
          sizeOf<ZenithAutomationPoint>(),
          _f32Bytes(0.375),
          () => point.ref.tension = 0.375,
        ),
        'curve': _offsetOf(
          point,
          sizeOf<ZenithAutomationPoint>(),
          _leBytes(0x11223344, 4),
          () => point.ref.curve = 0x11223344,
        ),
        'reserved': _offsetOf(
          point,
          sizeOf<ZenithAutomationPoint>(),
          _leBytes(0x55667788, 4),
          () => point.ref.reserved = 0x55667788,
        ),
      },
      laneState: {
        // Probes `kind`, the nested struct's first field, because an embedded
        // struct has no bytes of its own to write: writing `index` would land
        // four bytes in and report the wrong offset for the field as a whole.
        'id': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x1234, 2),
          () => lane.ref.id.kind = 0x1234,
        ),
        'pointCount': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x22334455, 4),
          () => lane.ref.pointCount = 0x22334455,
        ),
        'enabled': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x71, 1),
          () => lane.ref.enabled = 0x71,
        ),
        'armed': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x62, 1),
          () => lane.ref.armed = 0x62,
        ),
        'collapsed': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x53, 1),
          () => lane.ref.collapsed = 0x53,
        ),
        'reserved0': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x44, 1),
          () => lane.ref.reserved0 = 0x44,
        ),
        'firstFrame': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x3344556677889900, 8),
          () => lane.ref.firstFrame = 0x3344556677889900,
        ),
        'lastFrame': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x4455667788990011, 8),
          () => lane.ref.lastFrame = 0x4455667788990011,
        ),
        'color': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _leBytes(0x55667788, 4),
          () => lane.ref.color = 0x55667788,
        ),
        'height': _offsetOf(
          lane,
          sizeOf<ZenithLaneState>(),
          _f32Bytes(0.25),
          () => lane.ref.height = 0.25,
        ),
      },
      recorderState: {
        'enabled': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x74, 1),
          () => recorder.ref.enabled = 0x74,
        ),
        'takeOpen': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x65, 1),
          () => recorder.ref.takeOpen = 0x65,
        ),
        'reserved0': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x1234, 2),
          () => recorder.ref.reserved0 = 0x1234,
        ),
        'mode': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x33445566, 4),
          () => recorder.ref.mode = 0x33445566,
        ),
        'activeId': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x7788, 2),
          () => recorder.ref.activeId.kind = 0x7788,
        ),
        'takePoints': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x55667788, 4),
          () => recorder.ref.takePoints = 0x55667788,
        ),
        'totalCaptured': _offsetOf(
          recorder,
          sizeOf<ZenithRecorderState>(),
          _leBytes(0x66778899, 4),
          () => recorder.ref.totalCaptured = 0x66778899,
        ),
      },
    );
  } finally {
    calloc.free(point);
    calloc.free(lane);
    calloc.free(recorder);
  }
}

/// Writes one field and returns the byte offset it landed at.
///
/// [expect] is checked against the bytes at each candidate offset, using the
/// **field's own width**. Comparing a wider window would produce a false match
/// at a lower offset whenever the tail of the pattern is zero — a real bug this
/// helper had, which made `value` appear at offset 4 instead of 8.
///
/// Returning `-1` when nothing matches is deliberate: a missing offset fails
/// the caller's assertion with a readable number instead of throwing from
/// inside a probe.
int _offsetOf<T extends Struct>(
  Pointer<T> pointer,
  int size,
  List<int> expect,
  void Function() write,
) {
  final bytes = pointer.cast<Uint8>().asTypedList(size);
  bytes.fillRange(0, size, 0);
  write();
  final width = expect.length;
  outer:
  for (var offset = 0; offset + width <= size; offset++) {
    for (var i = 0; i < width; i++) {
      if (bytes[offset + i] != expect[i]) continue outer;
    }
    return offset;
  }
  return -1;
}

/// The little-endian bytes a value produces at the given width.
List<int> _leBytes(int value, int width) => [
      for (var i = 0; i < width; i++) (value >> (8 * i)) & 0xFF,
    ];

/// The little-endian bytes an IEEE-754 single produces.
List<int> _f32Bytes(double value) {
  final data = ByteData(4)..setFloat32(0, value, Endian.little);
  return [for (var i = 0; i < 4; i++) data.getUint8(i)];
}

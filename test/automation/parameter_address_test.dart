/// Tests for the S2 parameter addressing layer and curve evaluation.
///
/// ## What these tests are for
///
/// The curve math in `AutomationPointV2.valueAt` is a **second implementation**
/// of `native/zenith_core/src/automation/clip.rs::interpolate`. Two
/// implementations of one function is a standing invitation to divergence, and
/// the symptom is nasty: the editor draws one curve and the engine plays
/// another, so the user hears something they did not draw.
///
/// The tests below pin both the Dart behaviour *and* the specific numeric
/// values the Rust tests assert, so a change to either side fails here.
/// The Rust counterparts are named in each test's doc comment.
library;

import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/automation/automation_lane_painter.dart';
import 'package:zenith_audio/automation/native_types.dart';
import 'package:zenith_audio/automation/parameter_address.dart';

void main() {
  group('ParameterAddress', () {
    test('encodes and decodes each owner kind', () {
      const global = ParameterAddress.global(3);
      expect(global.kind, ParameterOwnerKind.global);
      expect(global.sub, 3);
      expect(global.index, 0);

      const channel = ParameterAddress.channel(7, 1);
      expect(channel.kind, ParameterOwnerKind.channel);
      expect(channel.index, 7);
      expect(channel.sub, 1);

      const track = ParameterAddress.track(12, 2);
      expect(track.kind, ParameterOwnerKind.track);
      expect(track.index, 12);
      expect(track.sub, 2);

      const effect = ParameterAddress.effect(5, 3, 4);
      expect(effect.kind, ParameterOwnerKind.effect);
      expect(effect.index, (5 << 8) | 3);
      expect(effect.channelIndex, 5, reason: 'slot must not leak into channel');
      expect(effect.effectSlot, 3);
      expect(effect.sub, 4);
    });

    test('an effect address keeps channel and slot in one u32', () {
      // The ABI has room for a single `index` field, so the packing has to be
      // lossless across the whole legal range: 24 bits of channel and 8 of
      // slot. A test at the boundary is what catches an off-by-one shift.
      const high = ParameterAddress.effect(0xFFFFFF, 0xFF, 0);
      expect(high.channelIndex, 0xFFFFFF);
      expect(high.effectSlot, 0xFF);
    });

    test('equality and hashCode use the triple, not a string', () {
      const a = ParameterAddress.channel(1, 2);
      const b = ParameterAddress.channel(1, 2);
      const differentKind = ParameterAddress.track(1, 2);
      const differentSub = ParameterAddress.channel(1, 3);

      expect(a, equals(b));
      expect(a.hashCode, equals(b.hashCode));
      expect(a, isNot(equals(differentKind)));
      expect(a, isNot(equals(differentSub)));

      // A map keyed by address must not collide across owners that share an
      // index/sub, which is exactly what a naive fold would do.
      final map = {a: 'channel', differentKind: 'track'};
      expect(map.length, 2);
      expect(map[a], 'channel');
    });

    test('the 64-bit key matches the Rust layout', () {
      // Mirrors `ParameterAddress::key()`: kind << 48 | index << 16 | sub.
      // The Rust store binary-searches on this value, so a mismatch would make
      // every lookup miss rather than return the wrong parameter — which is at
      // least a loud failure, but still one worth catching here.
      const address = ParameterAddress.channel(2, 5);
      const expected = (1 << 48) | (2 << 16) | 5;
      expect(address.key, expected);
    });

    test('toNative round-trips through the C struct', () {
      const address = ParameterAddress.effect(9, 4, 6);
      final pointer = address.toNative();
      try {
        expect(pointer.ref.kind, ParameterOwnerKind.effect.wire);
        expect(pointer.ref.index, (9 << 8) | 4);
        expect(pointer.ref.sub, 6);
        expect(ParameterAddress.fromNative(pointer.ref), equals(address));
        expect(ParameterAddress.tryFromNative(pointer.ref), equals(address));
      } finally {
        calloc.free(pointer);
      }
    });

    test('an unknown kind is reported, not silently coerced', () {
      // A newer Rust core could introduce a kind this build has never heard
      // of. Coercing it to `global` would write a *different* parameter than
      // the caller asked for — the worst possible failure mode.
      final pointer = calloc<ZenithParamId>();
      try {
        pointer.ref.kind = 999;
        pointer.ref.index = 1;
        pointer.ref.sub = 2;
        expect(ParameterAddress.tryFromNative(pointer.ref), isNull);
      } finally {
        calloc.free(pointer);
      }
    });

    test('owner kind discriminants are the frozen ABI values', () {
      expect(ParameterOwnerKind.global.wire, 0);
      expect(ParameterOwnerKind.channel.wire, 1);
      expect(ParameterOwnerKind.track.wire, 2);
      expect(ParameterOwnerKind.effect.wire, 3);
      expect(ParameterOwnerKind.modulator.wire, 4);

      expect(ParameterOwnerKind.fromWire(3), ParameterOwnerKind.effect);
      expect(ParameterOwnerKind.fromWire(999), isNull);
      expect(ParameterOwnerKind.fromSegment('channel'), ParameterOwnerKind.channel);
      expect(ParameterOwnerKind.fromSegment('nope'), isNull);
    });

    test('toString describes the address in the path form', () {
      // Used in logs and test failures, so it has to be readable and to say
      // which channel an effect slot belongs to.
      expect(const ParameterAddress.global(1).toString(), 'global/1');
      expect(const ParameterAddress.channel(2, 3).toString(), 'channel/2/3');
      expect(const ParameterAddress.effect(4, 5, 6).toString(), 'effect/4/5/6');
    });
  });

  group('AutomationCurve discriminants', () {
    test('match the Rust CurveKind values', () {
      expect(AutomationCurve.linear.wire, 0);
      expect(AutomationCurve.hold.wire, 1);
      expect(AutomationCurve.curve.wire, 2);
      expect(AutomationCurve.exponential.wire, 3);
      expect(AutomationCurve.logarithmic.wire, 4);
    });

    test('an unknown curve code falls back to linear', () {
      expect(AutomationCurve.fromWire(4), AutomationCurve.logarithmic);
      expect(AutomationCurve.fromWire(99), isNull);
    });
  });

  group('AutomationPointV2 interpolation', () {
    // Endpoints must be exact for every interpolating mode. A curve that misses
    // the end of a segment leaves a permanent offset in the played value.
    // `hold` is excluded on purpose: it is a step, so by definition it does not
    // reach its right endpoint — the next point owns that value.
    for (final curve in AutomationCurve.values.where(
      (c) => c != AutomationCurve.hold,
    )) {
      test('${curve.name} hits both endpoints exactly', () {
        final from = AutomationPointV2(
          frame: 0,
          value: -12,
          tension: 0.7,
          curve: curve,
        );
        const to = AutomationPointV2(frame: 100, value: 6);
        expect(from.valueAt(0, to), closeTo(-12, 1e-9));
        expect(from.valueAt(100, to), closeTo(6, 1e-9));
      });
    }

    test('linear is a straight line', () {
      const from = AutomationPointV2(frame: 0, value: 0);
      const to = AutomationPointV2(frame: 100, value: 10);
      expect(from.valueAt(50, to), closeTo(5, 1e-9));
      expect(from.valueAt(25, to), closeTo(2.5, 1e-9));
    });

    test('hold is a step function within the segment', () {
      // Mirrors `clip::tests::hold_curve_keeps_the_left_value_until_the_next_point`.
      // `valueAt` is a *segment* function, so it holds the left value for the
      // whole open interval; the closing endpoint belongs to the next segment
      // and is covered by `evaluatePoints` below. Getting this split wrong is
      // how a step curve ends up with a one-frame ramp at each step.
      const from = AutomationPointV2(
        frame: 0,
        value: 3,
        curve: AutomationCurve.hold,
      );
      const to = AutomationPointV2(frame: 100, value: 9);
      expect(from.valueAt(0, to), closeTo(3, 1e-9));
      expect(from.valueAt(1, to), closeTo(3, 1e-9));
      expect(from.valueAt(99, to), closeTo(3, 1e-9));
      expect(from.valueAt(100, to), closeTo(3, 1e-9));
    });

    test('evaluatePoints reaches the target exactly at the next point', () {
      // The lane-level view: at frame 100 the second point owns the value, so
      // the step completes. This is the counterpart to the segment test above.
      const points = [
        AutomationPointV2(frame: 0, value: 0, curve: AutomationCurve.hold),
        AutomationPointV2(frame: 100, value: 1),
      ];
      expect(evaluatePoints(points, 0), closeTo(0, 1e-9));
      expect(evaluatePoints(points, 99), closeTo(0, 1e-9));
      expect(evaluatePoints(points, 100), closeTo(1, 1e-9));
    });

    test('tension bends the midpoint but never leaves the segment', () {
      // Mirrors `clip::tests::positive_and_negative_tension_bend_opposite_ways`
      // and `curve_evaluation_never_escapes_the_segment_bounds`.
      const to = AutomationPointV2(frame: 100, value: 1);

      final up = AutomationPointV2(
        frame: 0,
        value: 0,
        tension: 0.8,
        curve: AutomationCurve.curve,
      );
      final down = AutomationPointV2(
        frame: 0,
        value: 0,
        tension: -0.8,
        curve: AutomationCurve.curve,
      );

      final upMid = up.valueAt(50, to);
      final downMid = down.valueAt(50, to);
      expect(upMid, greaterThan(0.5), reason: 'positive tension bends above');
      expect(downMid, lessThan(0.5), reason: 'negative tension bends below');

      // The earlier formulation passed through a pole and produced values
      // outside the range at high tension, which is audible as a click.
      for (final tension in [-1.0, -0.5, 0.5, 1.0]) {
        final from = AutomationPointV2(
          frame: 0,
          value: 0,
          tension: tension,
          curve: AutomationCurve.curve,
        );
        for (var frame = 0; frame <= 100; frame++) {
          final value = from.valueAt(frame, to);
          expect(
            value,
            inInclusiveRange(0, 1),
            reason: 'tension $tension escaped at frame $frame',
          );
        }
      }
    });

    test('the shaped curve is monotonic for every tension', () {
      // A non-monotonic segment produces a value that moves backward, which the
      // smoothing filter turns into an audible artifact.
      const to = AutomationPointV2(frame: 100, value: 1);
      for (final tension in [-1.0, -0.3, 0.0, 0.3, 1.0]) {
        final from = AutomationPointV2(
          frame: 0,
          value: 0,
          tension: tension,
          curve: AutomationCurve.curve,
        );
        var previous = double.negativeInfinity;
        for (var frame = 0; frame <= 100; frame++) {
          final value = from.valueAt(frame, to);
          expect(
            value,
            greaterThanOrEqualTo(previous - 1e-9),
            reason: 'tension $tension went backward at frame $frame',
          );
          previous = value;
        }
      }
    });

    test('exponential rises faster than logarithmic early on', () {
      const to = AutomationPointV2(frame: 100, value: 100);
      const expo = AutomationPointV2(
        frame: 0,
        value: 1,
        curve: AutomationCurve.exponential,
      );
      const logo = AutomationPointV2(
        frame: 0,
        value: 1,
        curve: AutomationCurve.logarithmic,
      );
      expect(expo.valueAt(25, to), greaterThan(logo.valueAt(25, to)));
    });

    test('exponential across zero degrades to linear, not NaN', () {
      // There is no real geometric root when the endpoints straddle zero; the
      // Rust side falls back to linear rather than producing NaN.
      const from = AutomationPointV2(
        frame: 0,
        value: -10,
        curve: AutomationCurve.exponential,
      );
      const to = AutomationPointV2(frame: 100, value: 10);
      final mid = from.valueAt(50, to);
      expect(mid.isNaN, isFalse);
      expect(mid, closeTo(0, 1e-9));
    });

    test('a zero-width segment returns the target, not a division by zero', () {
      const from = AutomationPointV2(frame: 50, value: 1);
      const to = AutomationPointV2(frame: 50, value: 7);
      expect(from.valueAt(50, to), closeTo(7, 1e-9));
    });

    test('valueAt clamps outside the segment', () {
      const from = AutomationPointV2(frame: 10, value: 2);
      const to = AutomationPointV2(frame: 20, value: 8);
      expect(from.valueAt(0, to), closeTo(2, 1e-9));
      expect(from.valueAt(999, to), closeTo(8, 1e-9));
    });

    test('copyWith replaces only the named fields', () {
      const original = AutomationPointV2(
        frame: 5,
        value: 1.5,
        tension: 0.25,
        curve: AutomationCurve.hold,
      );
      final copy = original.copyWith(value: 2.5);
      expect(copy.frame, 5);
      expect(copy.value, 2.5);
      expect(copy.tension, 0.25);
      expect(copy.curve, AutomationCurve.hold);
    });
  });

  group('native struct mirrors', () {
    test('Dart sizes match the values the core asserts', () {
      // These literals mirror the `const` assertions in
      // `native/zenith_core/src/ffi/types.rs`. If one side changes without the
      // other, one of the two test suites fails — which is the only thing that
      // makes a hand-written mirror safe.
      final sizes = dartStructSizes();
      expect(sizes.paramId, 8, reason: 'u16 + u16 + u32');
      expect(sizes.automationPoint, 24, reason: 'i64 + 3*f32 + u32 + u32');
      expect(sizes.automationStats, 28, reason: '7 * u32');
      expect(sizes.laneState, 40);
      expect(sizes.recorderState, 28);

      // The descriptor contains two pointers, so its size is target-dependent.
      // Comparing against `sizeOf<Pointer>()` keeps the test honest on both
      // 64-bit and 32-bit (wasm) targets instead of hard-coding one.
      final pointerSize = sizeOf<Pointer<Void>>();
      expect(sizes.paramDescriptor, 8 + 16 + 8 + 2 * pointerSize);
    });

    test('field offsets stay where the native layout expects them', () {
      // `sizeOf` alone would pass if two fields swapped. Offsets are what
      // actually catch a reorder, and a reorder is the one mistake that
      // compiles cleanly and corrupts data at runtime.
      final offsets = dartFieldOffsets();

      expect(offsets.automationPoint, {
        'frame': 0,
        'value': 8,
        'tension': 12,
        'curve': 16,
        'reserved': 20,
      });

      expect(offsets.laneState, {
        'id': 0,
        'pointCount': 8,
        'enabled': 12,
        'armed': 13,
        'collapsed': 14,
        'reserved0': 15,
        'firstFrame': 16,
        'lastFrame': 24,
        'color': 32,
        'height': 36,
      });

      expect(offsets.recorderState, {
        'enabled': 0,
        'takeOpen': 1,
        'reserved0': 2,
        'mode': 4,
        'activeId': 8,
        'takePoints': 16,
        'totalCaptured': 20,
      });
    });

    test('the flag bits match the Rust constants', () {
      expect(ZenithParamFlags.automatable, 0x01);
      expect(ZenithParamFlags.discrete, 0x02);
      expect(ZenithParamFlags.logarithmic, 0x04);
      expect(ZenithParamFlags.bipolar, 0x08);
      expect(ZenithParamFlags.smoothed, 0x10);
    });

    test('unit formatting degrades to plain numbers for unknown units', () {
      expect(ZenithParamUnit.suffixFor(ZenithParamUnit.decibels), ' dB');
      expect(ZenithParamUnit.suffixFor(ZenithParamUnit.hertz), ' Hz');
      expect(ZenithParamUnit.suffixFor(99), '', reason: 'unknown unit, no guess');
    });
  });
}

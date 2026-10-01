/// `dart:ffi` bindings for the Rust parameter and automation surface (S2).
///
/// ## Why this is a separate file from `native/zenith_core.dart`
///
/// That file owns the *link* — loading the library, the version handshake, the
/// "is the core even present" question. This file owns the *parameter ABI*.
/// Splitting them keeps the load path (which web builds must stub out entirely)
/// away from code that a test can exercise without a DLL.
///
/// ## The allocation rule
///
/// Every pointer this file creates is freed before the call returns, using
/// `try`/`finally`. A leak here is a slow bleed in a long session, and the
/// `Arena`-less style is chosen deliberately over `package:ffi`'s `calloc`
/// convenience wrappers so the lifetimes are visible at a glance.
///
/// ## Real-time note
///
/// Nothing in this file is safe to call from an audio callback. Dart cannot run
/// on the audio thread at all — the core's audio thread is inside Rust, entered
/// through the driver, and it calls `zenith_automation_advance_block` itself.
/// These bindings are for the controller/UI thread.
library;

import 'dart:ffi';

import 'package:ffi/ffi.dart';

import '../native/zenith_core.dart';
import 'native_types.dart';
import 'parameter_address.dart';

// ── C signatures: lifecycle ──

typedef _CreateNative = Int32 Function(Float, Pointer<Pointer<Void>>);
typedef _CreateDart = int Function(double, Pointer<Pointer<Void>>);

typedef _DestroyNative = Void Function(Pointer<Void>);
typedef _DestroyDart = void Function(Pointer<Void>);

typedef _PrepareNative = Int32 Function(Pointer<Void>, Float, Uint32);
typedef _PrepareDart = int Function(Pointer<Void>, double, int);

// ── C signatures: registration and description ──

typedef _RegisterNative = Int32 Function(
  Pointer<Void>, // handle
  Uint16, // kind
  Uint32, // index
  Uint16, // sub
  Pointer<Utf8>, // key
  Pointer<Utf8>, // label
  Uint32, // unit
  Uint32, // flags
  Float, // min
  Float, // max
  Float, // default
  Float, // smoothing
);
typedef _RegisterDart = int Function(
  Pointer<Void>,
  int,
  int,
  int,
  Pointer<Utf8>,
  Pointer<Utf8>,
  int,
  int,
  double,
  double,
  double,
  double,
);

typedef _ListParamsNative = Int32 Function(
  Pointer<Void>,
  Pointer<ZenithParamDescriptor>,
  Size,
  Pointer<Size>,
);
typedef _ListParamsDart = int Function(
  Pointer<Void>,
  Pointer<ZenithParamDescriptor>,
  int,
  Pointer<Size>,
);

typedef _ValueAtNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Int64,
  Pointer<Float>,
);
typedef _ValueAtDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  int,
  Pointer<Float>,
);

// ── C signatures: parameter read/write ──

typedef _GetNative = Int32 Function(Pointer<Void>, ZenithParamId, Pointer<Float>);
typedef _GetDart = int Function(Pointer<Void>, ZenithParamId, Pointer<Float>);

typedef _SetNative = Int32 Function(Pointer<Void>, ZenithParamId, Float);
typedef _SetDart = int Function(Pointer<Void>, ZenithParamId, double);

typedef _SetSmoothingNative = Int32 Function(Pointer<Void>, ZenithParamId, Float);
typedef _SetSmoothingDart = int Function(Pointer<Void>, ZenithParamId, double);

// ── C signatures: lanes and clips ──

typedef _LaneCreateNative = Int32 Function(Pointer<Void>, ZenithParamId);
typedef _LaneCreateDart = int Function(Pointer<Void>, ZenithParamId);

typedef _LaneRemoveNative = Int32 Function(Pointer<Void>, ZenithParamId);
typedef _LaneRemoveDart = int Function(Pointer<Void>, ZenithParamId);

typedef _LaneStateNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<ZenithLaneState>,
);
typedef _LaneStateDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<ZenithLaneState>,
);

typedef _LaneFlagNative = Int32 Function(Pointer<Void>, ZenithParamId, Uint32);
typedef _LaneFlagDart = int Function(Pointer<Void>, ZenithParamId, int);

typedef _PointCountNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<Size>,
);
typedef _PointCountDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<Size>,
);

typedef _GetPointsNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<ZenithAutomationPoint>,
  Size,
  Pointer<Size>,
);
typedef _GetPointsDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<ZenithAutomationPoint>,
  int,
  Pointer<Size>,
);

typedef _SetPointsNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<ZenithAutomationPoint>,
  Size,
);
typedef _SetPointsDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  Pointer<ZenithAutomationPoint>,
  int,
);

typedef _InsertPointNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  ZenithAutomationPoint,
);
typedef _InsertPointDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  ZenithAutomationPoint,
);

typedef _MovePointNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Size,
  Int64,
  Float,
);
typedef _MovePointDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  int,
  int,
  double,
);

typedef _SetCurveNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Size,
  Uint32,
  Float,
);
typedef _SetCurveDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  int,
  int,
  double,
);

typedef _RemovePointNative = Int32 Function(Pointer<Void>, ZenithParamId, Size);
typedef _RemovePointDart = int Function(Pointer<Void>, ZenithParamId, int);

typedef _RemoveRangeNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Int64,
  Int64,
);
typedef _RemoveRangeDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  int,
  int,
);

typedef _SimpleLaneNative = Int32 Function(Pointer<Void>, ZenithParamId);
typedef _SimpleLaneDart = int Function(Pointer<Void>, ZenithParamId);

typedef _TotalPointsNative = Int32 Function(Pointer<Void>, Pointer<Size>);
typedef _TotalPointsDart = int Function(Pointer<Void>, Pointer<Size>);

// ── C signatures: evaluation ──

typedef _AdvanceNative = Int32 Function(Pointer<Void>, Int64, Uint32);
typedef _AdvanceDart = int Function(Pointer<Void>, int, int);

typedef _StatsNative = Int32 Function(Pointer<Void>, Pointer<ZenithAutomationStats>);
typedef _StatsDart = int Function(Pointer<Void>, Pointer<ZenithAutomationStats>);

// ── C signatures: recording ──

typedef _RecorderEnableNative = Int32 Function(Pointer<Void>, Uint32);
typedef _RecorderEnableDart = int Function(Pointer<Void>, int);

typedef _RecorderMoveNative = Int32 Function(
  Pointer<Void>,
  ZenithParamId,
  Float,
  Int64,
  Uint32,
  Uint32,
  Pointer<Uint32>,
);
typedef _RecorderMoveDart = int Function(
  Pointer<Void>,
  ZenithParamId,
  double,
  int,
  int,
  int,
  Pointer<Uint32>,
);

typedef _RecorderFinishNative = Int32 Function(Pointer<Void>, Pointer<Uint32>);
typedef _RecorderFinishDart = int Function(Pointer<Void>, Pointer<Uint32>);

typedef _RecorderCancelNative = Int32 Function(Pointer<Void>);
typedef _RecorderCancelDart = int Function(Pointer<Void>);

typedef _RecorderStateNative = Int32 Function(
  Pointer<Void>,
  Pointer<ZenithRecorderState>,
);
typedef _RecorderStateDart = int Function(
  Pointer<Void>,
  Pointer<ZenithRecorderState>,
);

// ── C signatures: self-check ──

typedef _SizeofNative = Size Function();
typedef _SizeofDart = int Function();

typedef _FloatQueryNative = Float Function();
typedef _FloatQueryDart = double Function();

/// What a recording gesture did, mirroring `zenith_record_outcome`.
enum RecordOutcome {
  /// A point was captured.
  captured(0),

  /// The value had not moved enough to be worth a point.
  thinned(1),

  /// The lane is not armed, or the mode is [RecordMode.off].
  notArmed(2),

  /// The transport is not running.
  transportStopped(3),

  /// The point arrived out of order and was dropped.
  outOfOrder(4);

  const RecordOutcome(this.wire);

  /// The discriminant the core sends.
  final int wire;

  /// Looks up an outcome by wire value, defaulting to [notArmed] for an
  /// unrecognized code so an unknown future outcome degrades benignly.
  static RecordOutcome fromWire(int wire) {
    for (final outcome in RecordOutcome.values) {
      if (outcome.wire == wire) return outcome;
    }
    return RecordOutcome.notArmed;
  }
}

/// How the recorder treats a moving control.
///
/// Discriminants mirror `automation::lane::RecordMode` and
/// `zenith_record_mode`.
enum RecordMode {
  /// Not recording.
  off(0),

  /// Writes only while a control is touched; on release the value returns to
  /// whatever the existing automation says.
  touch(1),

  /// Writes from first touch until the transport stops.
  latch(2),

  /// Rewrites the whole pass, touched or not.
  write(3);

  const RecordMode(this.wire);

  /// The discriminant sent across the ABI.
  final int wire;

  /// Looks up a mode by wire value, or `null` when unknown.
  static RecordMode? fromWire(int wire) {
    for (final mode in RecordMode.values) {
      if (mode.wire == wire) return mode;
    }
    return null;
  }
}

/// Whether the recorder is currently capturing, for the transport indicator.
///
/// A plain value type rather than a live view of native memory: the core's
/// recorder state changes on the control thread, and a UI that read it lazily
/// would render a torn combination of fields.
final class RecorderStatus {
  /// Creates a status snapshot.
  const RecorderStatus({
    required this.enabled,
    required this.isRecording,
    required this.activeAddress,
    required this.takePoints,
    required this.totalCaptured,
  });

  /// Whether global automation recording is armed.
  final bool enabled;

  /// Whether a take is currently open.
  final bool isRecording;

  /// The parameter being recorded, when [isRecording] is set.
  final ParameterAddress? activeAddress;

  /// Points captured in the current take.
  final int takePoints;

  /// Points captured since the session began.
  final int totalCaptured;

  /// A status representing "nothing is happening".
  static const RecorderStatus idle = RecorderStatus(
    enabled: false,
    isRecording: false,
    activeAddress: null,
    takePoints: 0,
    totalCaptured: 0,
  );
}

/// Player statistics for the most recent block.
///
/// Exposed so the UI can show automation load, and so a support report can
/// distinguish "no automation" from "automation that failed to resolve" —
/// which look identical from the outside but have opposite fixes.
final class AutomationStats {
  /// Creates a stats snapshot.
  const AutomationStats({
    required this.evaluated,
    required this.automated,
    required this.modulated,
    required this.written,
    required this.skipped,
    required this.unresolved,
  });

  /// Parameters evaluated in the block.
  final int evaluated;

  /// Parameters that had a lane contributing.
  final int automated;

  /// Parameters that had at least one modulator contributing.
  final int modulated;

  /// Parameters written because their value changed.
  final int written;

  /// Parameters dropped because the per-block capacity was reached.
  ///
  /// Non-zero here means automation is being silently ignored, which is the
  /// one statistic a user must be warned about.
  final int skipped;

  /// Lane addresses that were not registered in the store.
  ///
  /// Non-zero means a lane survived a parameter's removal — a bug in the
  /// unlinking path, worth surfacing rather than hiding.
  final int unresolved;

  /// A stats value representing an idle engine.
  static const AutomationStats empty = AutomationStats(
    evaluated: 0,
    automated: 0,
    modulated: 0,
    written: 0,
    skipped: 0,
    unresolved: 0,
  );
}

/// A handle to the Rust automation subsystem.
///
/// Owns a native handle and must be [dispose]d. Use [AutomationHandle.open] to
/// create one; it returns `null` when the core is unavailable, which lets a
/// test or a web build run without the DLL.
final class AutomationHandle {
  AutomationHandle._(this._handle, this._bindings);

  Pointer<Void> _handle;
  final _AutomationBindings _bindings;
  bool _disposed = false;

  /// Whether the native handle is still valid.
  bool get isOpen => !_disposed;

  /// The struct sizes the loaded core reports, for mirror verification.
  ///
  /// This is the check that the hand-written Dart mirrors still describe the
  /// native structs. A test compares it against [dartStructSizes]; a mismatch
  /// is a memory-safety bug rather than a cosmetic one, so it is part of the
  /// public surface rather than a private detail.
  ///
  /// Returns [NativeStructSizes.unknown] when the core is absent, so a caller
  /// can distinguish "no core" from "core disagrees".
  static NativeStructSizes nativeStructSizes() => _AutomationBindings.nativeSizes();

  /// Opens the automation subsystem, or returns `null` when the Rust core is
  /// not loadable.
  ///
  /// [sampleRate] is used only to convert millisecond smoothing times into
  /// coefficients; a non-positive value falls back to 48 kHz inside the core.
  static AutomationHandle? open({double sampleRate = 48000}) {
    final bindings = _AutomationBindings.tryLoad();
    if (bindings == null) return null;

    final out = calloc<Pointer<Void>>();
    try {
      final status = bindings.create(sampleRate, out);
      if (status != 0) return null;
      return AutomationHandle._(out.value, bindings);
    } finally {
      calloc.free(out);
    }
  }

  /// Releases the native handle. Idempotent.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _bindings.destroy(_handle);
    _handle = nullptr;
  }

  /// Runs [body] with a native address, freeing it afterwards.
  ///
  /// This is the whole reason the binding layer can be this short: every call
  /// that needs an address goes through one place that owns the lifetime.
  T _withAddress<T>(ParameterAddress address, T Function(Pointer<ZenithParamId>) body) {
    final pointer = calloc<ZenithParamId>();
    try {
      address.writeTo(pointer);
      return body(pointer);
    } finally {
      calloc.free(pointer);
    }
  }

  /// Rebuilds the sample rate and reserves room for [laneCount] lanes.
  bool prepare({double sampleRate = 48000, int laneCount = 0}) =>
      _bindings.prepare(_handle, sampleRate, laneCount) == 0;

  // ── Registration ──

  /// Registers a parameter so it can be read, written and automated.
  ///
  /// `unit` and `flags` are the raw discriminants; the Rust registry is
  /// authoritative for what they mean, and this layer must not reinterpret
  /// them.
  bool registerParameter({
    required ParameterAddress address,
    required String key,
    required String label,
    int unit = 0,
    int flags = 0,
    double minValue = 0,
    double maxValue = 1,
    double defaultValue = 0,
    double smoothingMs = 10,
  }) {
    final keyPtr = key.toNativeUtf8();
    final labelPtr = label.toNativeUtf8();
    try {
      final status = _bindings.register(
        _handle,
        address.kind.wire,
        address.index,
        address.sub,
        keyPtr,
        labelPtr,
        unit,
        flags,
        minValue,
        maxValue,
        defaultValue,
        smoothingMs,
      );
      return status == 0;
    } finally {
      calloc.free(keyPtr);
      calloc.free(labelPtr);
    }
  }

  /// Reads every registered descriptor.
  ///
  /// Uses the two-call protocol: ask for the count, allocate exactly, then
  /// fetch. A single oversized buffer would work today but would silently
  /// truncate once the registry grows past the guess, and the whole point of
  /// this call is that Dart does not need to know the parameter list.
  List<NativeParameterDescriptor> listParameters() {
    final countPtr = calloc<Size>();
    try {
      var status = _bindings.listParameters(_handle, nullptr, 0, countPtr);
      if (status != 0) return const [];
      final count = countPtr.value;
      if (count == 0) return const [];

      final buffer = calloc<ZenithParamDescriptor>(count);
      try {
        status = _bindings.listParameters(_handle, buffer, count, countPtr);
        if (status != 0) return const [];
        // Trust the *reported* count over the pre-allocated one: the registry
        // can shrink between the two calls, and reading stale slots would hand
        // the UI descriptors for parameters that no longer exist.
        final written = countPtr.value < count ? countPtr.value : count;
        return [
          for (var i = 0; i < written; i++)
            NativeParameterDescriptor.fromNative((buffer + i).ref),
        ];
      } finally {
        calloc.free(buffer);
      }
    } finally {
      calloc.free(countPtr);
    }
  }

  // ── Values ──

  /// Reads a parameter's current value, or `null` when it is unregistered.
  double? read(ParameterAddress address) => _withAddress(address, (pointer) {
        final out = calloc<Float>();
        try {
          final status = _bindings.get(_handle, pointer.ref, out);
          return status == 0 ? out.value : null;
        } finally {
          calloc.free(out);
        }
      });

  /// Writes a parameter's value, clamped by the core to its descriptor range.
  bool write(ParameterAddress address, double value) => _withAddress(
        address,
        (pointer) => _bindings.set(_handle, pointer.ref, value) == 0,
      );

  /// Sets a parameter's smoothing time; the core clamps it to 1..50 ms.
  bool setSmoothing(ParameterAddress address, double milliseconds) =>
      _withAddress(
        address,
        (pointer) => _bindings.setSmoothing(_handle, pointer.ref, milliseconds) == 0,
      );

  /// Evaluates a parameter at [frame], including automation and modulation.
  ///
  /// This is the single source of truth for the parameter's value at a point in
  /// time — the same order the audio thread uses, without the smoothing filter,
  /// so the UI shows what the automation *says* rather than the faded value.
  double? valueAt(ParameterAddress address, int frame) =>
      _withAddress(address, (pointer) {
        final out = calloc<Float>();
        try {
          final status = _bindings.valueAt(_handle, pointer.ref, frame, out);
          return status == 0 ? out.value : null;
        } finally {
          calloc.free(out);
        }
      });

  // ── Lanes ──

  /// Creates a lane for a parameter, if it does not already have one.
  bool createLane(ParameterAddress address) => _withAddress(
        address,
        (pointer) => _bindings.laneCreate(_handle, pointer.ref) == 0,
      );

  /// Removes a lane.
  bool removeLane(ParameterAddress address) => _withAddress(
        address,
        (pointer) => _bindings.laneRemove(_handle, pointer.ref) == 0,
      );

  /// Reads a lane's metadata, or `null` when it does not exist.
  NativeLaneState? laneState(ParameterAddress address) =>
      _withAddress(address, (pointer) {
        final out = calloc<ZenithLaneState>();
        try {
          final status = _bindings.laneState(_handle, pointer.ref, out);
          return status == 0 ? NativeLaneState.fromNative(out.ref) : null;
        } finally {
          calloc.free(out);
        }
      });

  /// Enables or disables a lane's playback.
  bool setLaneEnabled(ParameterAddress address, bool enabled) => _withAddress(
        address,
        (pointer) =>
            _bindings.laneSetEnabled(_handle, pointer.ref, enabled ? 1 : 0) == 0,
      );

  /// Arms or disarms a lane for recording.
  bool setLaneArmed(ParameterAddress address, bool armed) => _withAddress(
        address,
        (pointer) =>
            _bindings.laneSetArmed(_handle, pointer.ref, armed ? 1 : 0) == 0,
      );

  /// Collapses or expands a lane in the editor.
  bool setLaneCollapsed(ParameterAddress address, bool collapsed) =>
      _withAddress(
        address,
        (pointer) => _bindings.laneSetCollapsed(
          _handle,
          pointer.ref,
          collapsed ? 1 : 0,
        ) ==
            0,
      );

  /// Number of points in a lane, or `null` when it does not exist.
  int? pointCount(ParameterAddress address) => _withAddress(address, (pointer) {
        final out = calloc<Size>();
        try {
          final status = _bindings.pointCount(_handle, pointer.ref, out);
          return status == 0 ? out.value : null;
        } finally {
          calloc.free(out);
        }
      });

  /// Reads every point in a lane, in frame order.
  ///
  /// Same two-call protocol as [listParameters], for the same reason: a lane
  /// can hold thousands of points and a wrong guess either truncates a curve
  /// or wastes memory.
  List<AutomationPointV2> points(ParameterAddress address) =>
      _withAddress(address, (pointer) {
        final countPtr = calloc<Size>();
        try {
          var status = _bindings.getPoints(_handle, pointer.ref, nullptr, 0, countPtr);
          if (status != 0) return const <AutomationPointV2>[];
          final count = countPtr.value;
          if (count == 0) return const <AutomationPointV2>[];

          final buffer = calloc<ZenithAutomationPoint>(count);
          try {
            status = _bindings.getPoints(
              _handle,
              pointer.ref,
              buffer,
              count,
              countPtr,
            );
            if (status != 0) return const <AutomationPointV2>[];
            final written = countPtr.value < count ? countPtr.value : count;
            return [
              for (var i = 0; i < written; i++)
                _pointFrom((buffer + i).ref),
            ];
          } finally {
            calloc.free(buffer);
          }
        } finally {
          calloc.free(countPtr);
        }
      });

  /// Replaces a lane's points wholesale.
  ///
  /// This is how the editor commits an edit: the Rust side sorts and indexes
  /// once, here, so evaluation never pays for it.
  bool setPoints(ParameterAddress address, List<AutomationPointV2> points) =>
      _withAddress(address, (pointer) {
        if (points.isEmpty) {
          return _bindings.setPoints(_handle, pointer.ref, nullptr, 0) == 0;
        }
        final buffer = calloc<ZenithAutomationPoint>(points.length);
        try {
          for (var i = 0; i < points.length; i++) {
            _pointTo(points[i], (buffer + i).ref);
          }
          return _bindings.setPoints(
                _handle,
                pointer.ref,
                buffer,
                points.length,
              ) ==
              0;
        } finally {
          calloc.free(buffer);
        }
      });

  /// Inserts one point, keeping the clip sorted.
  bool insertPoint(ParameterAddress address, AutomationPointV2 point) =>
      _withAddress(address, (pointer) {
        final struct = calloc<ZenithAutomationPoint>();
        try {
          _pointTo(point, struct.ref);
          return _bindings.insertPoint(_handle, pointer.ref, struct.ref) == 0;
        } finally {
          calloc.free(struct);
        }
      });

  /// Moves the point at [index], re-sorting the clip if its frame changed.
  ///
  /// After a re-sort the indices shift; re-read with [points] before moving
  /// another one. See the index-invalidation note in `docs/ABI.md` §6.4.
  bool movePoint(
    ParameterAddress address,
    int index,
    int frame,
    double value,
  ) =>
      _withAddress(
        address,
        (pointer) =>
            _bindings.movePoint(_handle, pointer.ref, index, frame, value) == 0,
      );

  /// Sets a point's interpolation mode and tension.
  bool setCurve(
    ParameterAddress address,
    int index,
    AutomationCurve curve,
    double tension,
  ) =>
      _withAddress(
        address,
        (pointer) => _bindings.setCurve(
          _handle,
          pointer.ref,
          index,
          curve.wire,
          tension,
        ) ==
            0,
      );

  /// Removes the point at [index].
  bool removePoint(ParameterAddress address, int index) => _withAddress(
        address,
        (pointer) => _bindings.removePoint(_handle, pointer.ref, index) == 0,
      );

  /// Removes every point in `startFrame..=endFrame`.
  bool removeRange(ParameterAddress address, int startFrame, int endFrame) =>
      _withAddress(
        address,
        (pointer) => _bindings.removeRange(
          _handle,
          pointer.ref,
          startFrame,
          endFrame,
        ) ==
            0,
      );

  /// Removes every point in a lane.
  bool clearPoints(ParameterAddress address) => _withAddress(
        address,
        (pointer) => _bindings.clearPoints(_handle, pointer.ref) == 0,
      );

  /// Total points across every lane.
  int totalPoints() {
    final out = calloc<Size>();
    try {
      return _bindings.totalPoints(_handle, out) == 0 ? out.value : 0;
    } finally {
      calloc.free(out);
    }
  }

  // ── Evaluation ──

  /// Advances automation and modulation by one block.
  ///
  /// Provided for tests and for an offline render loop. In a live session the
  /// Rust audio thread calls this itself; calling it from Dart as well would
  /// double-advance every modulator.
  bool advanceBlock(int frame, int frames) =>
      _bindings.advance(_handle, frame, frames) == 0;

  /// The player's statistics for the most recent block.
  AutomationStats stats() {
    final out = calloc<ZenithAutomationStats>();
    try {
      if (_bindings.stats(_handle, out) != 0) return AutomationStats.empty;
      final ref = out.ref;
      return AutomationStats(
        evaluated: ref.evaluated,
        automated: ref.automated,
        modulated: ref.modulated,
        written: ref.written,
        skipped: ref.skipped,
        unresolved: ref.unresolved,
      );
    } finally {
      calloc.free(out);
    }
  }

  // ── Recording ──

  /// Arms or disarms global automation recording.
  bool setRecordingEnabled(bool enabled) =>
      _bindings.recorderSetEnabled(_handle, enabled ? 1 : 0) == 0;

  /// Reports that a control moved, so an armed lane can capture it.
  ///
  /// [touching] must reflect whether the user is *currently* holding the
  /// control. A mouse-drag UI knows this from the gesture; a keyboard entry
  /// that cannot distinguish touch from move should pass `true`, which is the
  /// conservative choice — it records rather than silently dropping.
  RecordOutcome onControlMove({
    required ParameterAddress address,
    required double value,
    required int frame,
    required bool touching,
    required RecordMode mode,
  }) =>
      _withAddress(address, (pointer) {
        final out = calloc<Uint32>();
        try {
          final status = _bindings.recorderOnMove(
            _handle,
            pointer.ref,
            value,
            frame,
            touching ? 1 : 0,
            mode.wire,
            out,
          );
          return status == 0 ? RecordOutcome.fromWire(out.value) : RecordOutcome.notArmed;
        } finally {
          calloc.free(out);
        }
      });

  /// Ends the open take and merges it into its lane.
  ///
  /// Returns whether anything was actually committed; `false` means there was
  /// no take to finish, which the caller should not treat as an error.
  bool finishRecording() {
    final out = calloc<Uint32>();
    try {
      if (_bindings.recorderFinish(_handle, out) != 0) return false;
      return out.value != 0;
    } finally {
      calloc.free(out);
    }
  }

  /// Discards the open take without committing it.
  bool cancelRecording() => _bindings.recorderCancel(_handle) == 0;

  /// The recorder's current state, for the transport indicator.
  RecorderStatus recorderStatus() {
    final out = calloc<ZenithRecorderState>();
    try {
      if (_bindings.recorderState(_handle, out) != 0) {
        return RecorderStatus.idle;
      }
      final ref = out.ref;
      final recording = ref.takeOpen != 0;
      return RecorderStatus(
        enabled: ref.enabled != 0,
        isRecording: recording,
        activeAddress: recording
            ? ParameterAddress.tryFromNative(ref.activeId)
            : null,
        takePoints: ref.takePoints,
        totalCaptured: ref.totalCaptured,
      );
    } finally {
      calloc.free(out);
    }
  }
}

/// Reads a native point struct into a Dart value.
AutomationPointV2 _pointFrom(ZenithAutomationPoint ref) => AutomationPointV2(
      frame: ref.frame,
      value: ref.value,
      tension: ref.tension,
      curve: AutomationCurve.fromWire(ref.curve) ?? AutomationCurve.linear,
    );

/// Writes a Dart point into a native struct.
void _pointTo(AutomationPointV2 point, ZenithAutomationPoint ref) {
  ref.frame = point.frame;
  ref.value = point.value;
  ref.tension = point.tension;
  ref.curve = point.curve.wire;
  ref.reserved = 0;
}

/// Resolved function pointers for the S2 surface.
///
/// Held together so a failed lookup fails as a unit — a partially-resolved
/// binding set is how you get a crash on the one method a test happened to
/// exercise.
final class _AutomationBindings {
  _AutomationBindings._(DynamicLibrary library)
      : create = library.lookupFunction<_CreateNative, _CreateDart>(
          'zenith_automation_create',
        ),
        destroy = library.lookupFunction<_DestroyNative, _DestroyDart>(
          'zenith_automation_destroy',
        ),
        prepare = library.lookupFunction<_PrepareNative, _PrepareDart>(
          'zenith_automation_prepare',
        ),
        register = library.lookupFunction<_RegisterNative, _RegisterDart>(
          'zenith_automation_register_parameter',
        ),
        listParameters = library
            .lookupFunction<_ListParamsNative, _ListParamsDart>(
          'zenith_automation_list_parameters',
        ),
        valueAt = library.lookupFunction<_ValueAtNative, _ValueAtDart>(
          'zenith_automation_value_at',
        ),
        get = library.lookupFunction<_GetNative, _GetDart>(
          'zenith_automation_param_get',
        ),
        set = library.lookupFunction<_SetNative, _SetDart>(
          'zenith_automation_param_set',
        ),
        setSmoothing = library
            .lookupFunction<_SetSmoothingNative, _SetSmoothingDart>(
          'zenith_automation_param_set_smoothing',
        ),
        laneCreate = library.lookupFunction<_LaneCreateNative, _LaneCreateDart>(
          'zenith_automation_lane_create',
        ),
        laneRemove = library.lookupFunction<_LaneRemoveNative, _LaneRemoveDart>(
          'zenith_automation_lane_remove',
        ),
        laneState = library.lookupFunction<_LaneStateNative, _LaneStateDart>(
          'zenith_automation_lane_state',
        ),
        laneSetEnabled = library
            .lookupFunction<_LaneFlagNative, _LaneFlagDart>(
          'zenith_automation_lane_set_enabled',
        ),
        laneSetArmed = library.lookupFunction<_LaneFlagNative, _LaneFlagDart>(
          'zenith_automation_lane_set_armed',
        ),
        laneSetCollapsed = library
            .lookupFunction<_LaneFlagNative, _LaneFlagDart>(
          'zenith_automation_lane_set_collapsed',
        ),
        pointCount = library
            .lookupFunction<_PointCountNative, _PointCountDart>(
          'zenith_automation_clip_point_count',
        ),
        getPoints = library.lookupFunction<_GetPointsNative, _GetPointsDart>(
          'zenith_automation_clip_get_points',
        ),
        setPoints = library.lookupFunction<_SetPointsNative, _SetPointsDart>(
          'zenith_automation_clip_set_points',
        ),
        insertPoint = library
            .lookupFunction<_InsertPointNative, _InsertPointDart>(
          'zenith_automation_clip_insert_point',
        ),
        movePoint = library.lookupFunction<_MovePointNative, _MovePointDart>(
          'zenith_automation_clip_move_point',
        ),
        setCurve = library.lookupFunction<_SetCurveNative, _SetCurveDart>(
          'zenith_automation_clip_set_curve',
        ),
        removePoint = library
            .lookupFunction<_RemovePointNative, _RemovePointDart>(
          'zenith_automation_clip_remove_point',
        ),
        removeRange = library
            .lookupFunction<_RemoveRangeNative, _RemoveRangeDart>(
          'zenith_automation_clip_remove_range',
        ),
        clearPoints = library.lookupFunction<_SimpleLaneNative, _SimpleLaneDart>(
          'zenith_automation_clip_clear',
        ),
        totalPoints = library
            .lookupFunction<_TotalPointsNative, _TotalPointsDart>(
          'zenith_automation_total_points',
        ),
        advance = library.lookupFunction<_AdvanceNative, _AdvanceDart>(
          'zenith_automation_advance_block',
        ),
        stats = library.lookupFunction<_StatsNative, _StatsDart>(
          'zenith_automation_stats',
        ),
        recorderSetEnabled = library
            .lookupFunction<_RecorderEnableNative, _RecorderEnableDart>(
          'zenith_automation_recorder_set_enabled',
        ),
        recorderOnMove = library
            .lookupFunction<_RecorderMoveNative, _RecorderMoveDart>(
          'zenith_automation_recorder_on_control_move',
        ),
        recorderFinish = library
            .lookupFunction<_RecorderFinishNative, _RecorderFinishDart>(
          'zenith_automation_recorder_finish',
        ),
        recorderCancel = library
            .lookupFunction<_RecorderCancelNative, _RecorderCancelDart>(
          'zenith_automation_recorder_cancel',
        ),
        recorderState = library
            .lookupFunction<_RecorderStateNative, _RecorderStateDart>(
          'zenith_automation_recorder_state',
        );

  /// Resolves the binding set, or returns `null` when the core is unavailable
  /// or a symbol is missing.
  ///
  /// A missing symbol means the DLL predates S2, which the version handshake
  /// should already have caught; returning `null` rather than throwing keeps a
  /// stale library from taking down a test run.
  static _AutomationBindings? tryLoad() {
    final library = ZenithCore.libraryOrNull();
    if (library == null) return null;
    try {
      return _AutomationBindings._(library);
    } on Object {
      return null;
    }
  }

  final _CreateDart create;
  final _DestroyDart destroy;
  final _PrepareDart prepare;
  final _RegisterDart register;
  final _ListParamsDart listParameters;
  final _ValueAtDart valueAt;
  final _GetDart get;
  final _SetDart set;
  final _SetSmoothingDart setSmoothing;
  final _LaneCreateDart laneCreate;
  final _LaneRemoveDart laneRemove;
  final _LaneStateDart laneState;
  final _LaneFlagDart laneSetEnabled;
  final _LaneFlagDart laneSetArmed;
  final _LaneFlagDart laneSetCollapsed;
  final _PointCountDart pointCount;
  final _GetPointsDart getPoints;
  final _SetPointsDart setPoints;
  final _InsertPointDart insertPoint;
  final _MovePointDart movePoint;
  final _SetCurveDart setCurve;
  final _RemovePointDart removePoint;
  final _RemoveRangeDart removeRange;
  final _SimpleLaneDart clearPoints;
  final _TotalPointsDart totalPoints;
  final _AdvanceDart advance;
  final _StatsDart stats;
  final _RecorderEnableDart recorderSetEnabled;
  final _RecorderMoveDart recorderOnMove;
  final _RecorderFinishDart recorderFinish;
  final _RecorderCancelDart recorderCancel;
  final _RecorderStateDart recorderState;

  /// Sizes reported by the core, for mirror verification.
  ///
  /// Kept off the constructor so a missing `zenith_sizeof_*` symbol surfaces as
  /// [NativeStructSizes.unknown] rather than failing the whole binding set.
  static NativeStructSizes nativeSizes() {
    final library = ZenithCore.libraryOrNull();
    if (library == null) return NativeStructSizes.unknown;
    int Function()? sizeOf(String name) {
      try {
        return library.lookupFunction<_SizeofNative, _SizeofDart>(name);
      } on Object {
        return null;
      }
    }

    double Function()? floatQuery(String name) {
      try {
        return library.lookupFunction<_FloatQueryNative, _FloatQueryDart>(name);
      } on Object {
        return null;
      }
    }

    final paramId = sizeOf('zenith_sizeof_param_id');
    final descriptor = sizeOf('zenith_sizeof_param_descriptor');
    final point = sizeOf('zenith_sizeof_automation_point');
    final stats = sizeOf('zenith_sizeof_automation_stats');
    final lane = sizeOf('zenith_sizeof_lane_state');
    final recorder = sizeOf('zenith_sizeof_recorder_state');
    if (paramId == null ||
        descriptor == null ||
        point == null ||
        stats == null ||
        lane == null ||
        recorder == null) {
      return NativeStructSizes.unknown;
    }
    return NativeStructSizes(
      paramId: paramId(),
      paramDescriptor: descriptor(),
      automationPoint: point(),
      automationStats: stats(),
      laneState: lane(),
      recorderState: recorder(),
      smoothingMin: floatQuery('zenith_smoothing_ms_min')?.call() ?? 1,
      smoothingMax: floatQuery('zenith_smoothing_ms_max')?.call() ?? 50,
    );
  }
}

/// Native struct sizes, as reported by the loaded core.
///
/// Used to verify the Dart mirrors at runtime. A mismatch is a
/// memory-safety bug that would otherwise show up as corrupted automation, so
/// this is checked rather than assumed.
final class NativeStructSizes {
  /// Creates a size report.
  const NativeStructSizes({
    required this.paramId,
    required this.paramDescriptor,
    required this.automationPoint,
    required this.automationStats,
    required this.laneState,
    required this.recorderState,
    required this.smoothingMin,
    required this.smoothingMax,
  });

  /// The report when the core is unavailable.
  static const NativeStructSizes unknown = NativeStructSizes(
    paramId: 0,
    paramDescriptor: 0,
    automationPoint: 0,
    automationStats: 0,
    laneState: 0,
    recorderState: 0,
    smoothingMin: 0,
    smoothingMax: 0,
  );

  /// `sizeof(ZenithParamId)`.
  final int paramId;

  /// `sizeof(ZenithParamDescriptor)`.
  final int paramDescriptor;

  /// `sizeof(ZenithAutomationPoint)`.
  final int automationPoint;

  /// `sizeof(ZenithAutomationStats)`.
  final int automationStats;

  /// `sizeof(ZenithLaneState)`.
  final int laneState;

  /// `sizeof(ZenithRecorderState)`.
  final int recorderState;

  /// Lowest legal smoothing time, in milliseconds.
  final double smoothingMin;

  /// Highest legal smoothing time, in milliseconds.
  final double smoothingMax;

  /// Whether the core reported real values.
  bool get isKnown => paramId > 0;
}

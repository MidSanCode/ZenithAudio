/// `dart:ffi` bindings for the offline audio-edit algorithms (PLAN §3.S8).
///
/// These wrap `zenith_time_stretch` / `zenith_pitch_shift` /
/// `zenith_detect_transients` / `zenith_crossfade`. They are **not real-time**:
/// each allocates and runs a heavy transform, so they belong on a worker thread
/// (or `Isolate.run`), never on the audio callback.
///
/// ## Memory
///
/// The core returns a Rust-allocated buffer. This layer copies it into a Dart
/// `Float32List` and immediately frees the native buffer with
/// `zenith_edit_buffer_free`, so the caller never sees a raw pointer (ABI
/// principle P3).
library;

import 'dart:ffi';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import '../../native/zenith_core.dart';

typedef _StretchNative = Pointer<Float> Function(
  Pointer<Float>,
  Size,
  Float,
  Pointer<Size>,
);
typedef _StretchDart = Pointer<Float> Function(
  Pointer<Float>,
  int,
  double,
  Pointer<Size>,
);

typedef _DetectNative = Int32 Function(
  Pointer<Float>,
  Size,
  Pointer<Uint32>,
  Size,
  Pointer<Size>,
);
typedef _DetectDart = int Function(
  Pointer<Float>,
  int,
  Pointer<Uint32>,
  int,
  Pointer<Size>,
);

typedef _CrossfadeNative = Pointer<Float> Function(
  Pointer<Float>,
  Size,
  Pointer<Float>,
  Size,
  Size,
  Uint32,
  Pointer<Size>,
);
typedef _CrossfadeDart = Pointer<Float> Function(
  Pointer<Float>,
  int,
  Pointer<Float>,
  int,
  int,
  int,
  Pointer<Size>,
);

typedef _BufferFreeNative = Void Function(Pointer<Float>, Size);
typedef _BufferFreeDart = void Function(Pointer<Float>, int);

/// The crossfade shapes, mirroring `edit::FadeCurve`.
abstract final class FadeCurveCode {
  /// Linear amplitude: sums to a constant when the two sides are correlated.
  static const int linear = 0;

  /// Equal-power: constant power when the two sides are uncorrelated.
  static const int equalPower = 1;
}

/// Offline audio-editing operations.
///
/// Every method returns `null` when the native core is unavailable or the call
/// fails, rather than throwing: these run on user-supplied audio where a failure
/// should surface as "no change", not a crash.
abstract final class AudioEditBindings {
  static _StretchDart? _stretch;
  static _StretchDart? _pitch;
  static _DetectDart? _detect;
  static _CrossfadeDart? _crossfade;
  static _BufferFreeDart? _free;

  static DynamicLibrary? _library() => ZenithCore.libraryOrNull();

  static bool get isAvailable => _library() != null;

  static _StretchDart? _bindStretch(DynamicLibrary lib) =>
      _stretch ??= lib.lookupFunction<_StretchNative, _StretchDart>(
        'zenith_time_stretch',
      );

  static _StretchDart? _bindPitch(DynamicLibrary lib) =>
      _pitch ??= lib.lookupFunction<_StretchNative, _StretchDart>(
        'zenith_pitch_shift',
      );

  static _BufferFreeDart? _bindFree(DynamicLibrary lib) =>
      _free ??= lib.lookupFunction<_BufferFreeNative, _BufferFreeDart>(
        'zenith_edit_buffer_free',
      );

  /// Copies a native `(pointer, count)` result into a `Float32List` and frees
  /// the native buffer.
  static Float32List? _collect(
    Pointer<Float> pointer,
    int count,
    _BufferFreeDart? free,
  ) {
    if (pointer == nullptr || count == 0) return null;
    final list = Float32List.fromList(pointer.asTypedList(count));
    free?.call(pointer, count);
    return list;
  }

  /// Stretches [input] in time by [factor] (>1 longer, <1 shorter), preserving
  /// pitch.
  static Float32List? timeStretch(Float32List input, double factor) {
    final lib = _library();
    if (lib == null || input.isEmpty) return null;
    final stretch = _bindStretch(lib);
    if (stretch == null) return null;
    final inPtr = calloc<Float>(input.length);
    final outCount = calloc<Size>();
    try {
      inPtr.asTypedList(input.length).setAll(0, input);
      final out = stretch(inPtr, input.length, factor, outCount);
      return _collect(out, outCount.value, _bindFree(lib));
    } finally {
      calloc.free(inPtr);
      calloc.free(outCount);
    }
  }

  /// Shifts [input] in pitch by [semitones], preserving length.
  static Float32List? pitchShift(Float32List input, double semitones) {
    final lib = _library();
    if (lib == null || input.isEmpty) return null;
    final pitch = _bindPitch(lib);
    if (pitch == null) return null;
    final inPtr = calloc<Float>(input.length);
    final outCount = calloc<Size>();
    try {
      inPtr.asTypedList(input.length).setAll(0, input);
      final out = pitch(inPtr, input.length, semitones, outCount);
      return _collect(out, outCount.value, _bindFree(lib));
    } finally {
      calloc.free(inPtr);
      calloc.free(outCount);
    }
  }

  /// Detects transient positions (in samples) in [input].
  static List<int> detectTransients(Float32List input) {
    final lib = _library();
    if (lib == null || input.isEmpty) return const [];
    final detect = _detect ??=
        lib.lookupFunction<_DetectNative, _DetectDart>('zenith_detect_transients');
    final inPtr = calloc<Float>(input.length);
    // A generous bound: at most one transient per 256-sample hop.
    final capacity = (input.length ~/ 256) + 1;
    final outPtr = calloc<Uint32>(capacity);
    final outCount = calloc<Size>();
    try {
      inPtr.asTypedList(input.length).setAll(0, input);
      final code = detect(inPtr, input.length, outPtr, capacity, outCount);
      if (code != 0) return const [];
      final count = outCount.value.clamp(0, capacity);
      final view = outPtr.asTypedList(capacity);
      return [for (var i = 0; i < count; i++) view[i]];
    } finally {
      calloc.free(inPtr);
      calloc.free(outPtr);
      calloc.free(outCount);
    }
  }

  /// Crossfades [outgoing] into [incoming] over [fade] samples.
  static Float32List? crossfade(
    Float32List outgoing,
    Float32List incoming, {
    int fade = 256,
    int curve = FadeCurveCode.equalPower,
  }) {
    final lib = _library();
    if (lib == null) return null;
    final cross = _crossfade ??=
        lib.lookupFunction<_CrossfadeNative, _CrossfadeDart>('zenith_crossfade');
    final aPtr = calloc<Float>(outgoing.isEmpty ? 1 : outgoing.length);
    final bPtr = calloc<Float>(incoming.isEmpty ? 1 : incoming.length);
    final outCount = calloc<Size>();
    try {
      if (outgoing.isNotEmpty) aPtr.asTypedList(outgoing.length).setAll(0, outgoing);
      if (incoming.isNotEmpty) bPtr.asTypedList(incoming.length).setAll(0, incoming);
      final out = cross(
        aPtr,
        outgoing.length,
        bPtr,
        incoming.length,
        fade,
        curve,
        outCount,
      );
      return _collect(out, outCount.value, _bindFree(lib));
    } finally {
      calloc.free(aPtr);
      calloc.free(bPtr);
      calloc.free(outCount);
    }
  }
}

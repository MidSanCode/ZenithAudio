/// `dart:ffi` bindings for the Rust real-time engine surface (S1).
///
/// ## Scope
///
/// This wraps the `zenith_engine_*` / `zenith_transport_*` functions declared in
/// `docs/ABI.md` §6.2–§6.3 and implemented in
/// `native/zenith_core/src/ffi/engine_api.rs`. It owns one opaque
/// `*mut ZenithEngine` for its lifetime and releases it in [dispose].
///
/// ## Real-time note
///
/// Nothing here runs on the audio thread. The Rust engine's audio thread is
/// entered through the driver inside Rust; these bindings are for the
/// controller/UI thread and for a test that pulls blocks from the render
/// function.
///
/// ## Allocation rule
///
/// Every pointer created here is freed before the call returns (`try`/`finally`),
/// matching the style of the other bindings in this codebase.
///
/// Registered as C-013 in `docs/COORDINATION.md`.
library;

import 'dart:ffi';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

import '../../native/zenith_core.dart';
import 'engine_types.dart';

// ── C signatures ──

typedef _CreateNative = Int32 Function(
  Pointer<ZenithEngineConfig>,
  Pointer<Pointer<Void>>,
);
typedef _CreateDart = int Function(
  Pointer<ZenithEngineConfig>,
  Pointer<Pointer<Void>>,
);

typedef _DestroyNative = Void Function(Pointer<Void>);
typedef _DestroyDart = void Function(Pointer<Void>);

typedef _SimpleNative = Int32 Function(Pointer<Void>);
typedef _SimpleDart = int Function(Pointer<Void>);

typedef _TempoNative = Int32 Function(Pointer<Void>, Double);
typedef _TempoDart = int Function(Pointer<Void>, double);

typedef _TimeSigNative = Int32 Function(Pointer<Void>, Uint32, Uint32);
typedef _TimeSigDart = int Function(Pointer<Void>, int, int);

typedef _SeekNative = Int32 Function(Pointer<Void>, ZenithMusicalTime);
typedef _SeekDart = int Function(Pointer<Void>, ZenithMusicalTime);

typedef _LoopNative = Int32 Function(
  Pointer<Void>,
  ZenithMusicalTime,
  ZenithMusicalTime,
  Uint8,
);
typedef _LoopDart = int Function(
  Pointer<Void>,
  ZenithMusicalTime,
  ZenithMusicalTime,
  int,
);

typedef _RenderNative = Int32 Function(
  Pointer<Void>,
  Pointer<Float>,
  Uint32,
  Uint32,
);
typedef _RenderDart = int Function(
  Pointer<Void>,
  Pointer<Float>,
  int,
  int,
);

typedef _StatusNative = Int32 Function(
  Pointer<Void>,
  Pointer<ZenithEngineStatus>,
);
typedef _StatusDart = int Function(
  Pointer<Void>,
  Pointer<ZenithEngineStatus>,
);

typedef _DriverSupportedNative = Uint32 Function(Uint32);
typedef _DriverSupportedDart = int Function(int);

typedef _SizeofNative = Size Function();
typedef _SizeofDart = int Function();

// ── S4: offline rendering ──

typedef _RenderOfflineNative = Int32 Function(
  Pointer<Void>,
  ZenithMusicalTime,
  ZenithMusicalTime,
  Uint32,
  Pointer<Pointer<Float>>,
  Pointer<Size>,
);
typedef _RenderOfflineDart = int Function(
  Pointer<Void>,
  ZenithMusicalTime,
  ZenithMusicalTime,
  int,
  Pointer<Pointer<Float>>,
  Pointer<Size>,
);

typedef _BufferFreeNative = Void Function(Pointer<Float>, Size);
typedef _BufferFreeDart = void Function(Pointer<Float>, int);

typedef _PdcLatencyNative = Int32 Function(Pointer<Void>, Pointer<Uint32>);
typedef _PdcLatencyDart = int Function(Pointer<Void>, Pointer<Uint32>);

/// An `Int32` result of `ZENITH_OK` means success for this surface.
const int _ok = 0;

/// A `ZenithMusicalTime` pointer for a tick position, at the default PPQ.
///
/// The caller owns the allocation and must free it.
Pointer<ZenithMusicalTime> _musicalTime(int ticks, {int ppq = 960}) {
  final time = calloc<ZenithMusicalTime>();
  time.ref
    ..ticks = ticks
    ..ppq = ppq
    ..reserved = 0;
  return time;
}

/// A handle to one native engine instance.
final class ZenithEngineHandle {
  ZenithEngineHandle._(this._bindings, this._handle);

  final _EngineBindings _bindings;
  final Pointer<Void> _handle;
  bool _disposed = false;

  /// Creates an engine, or returns `null` when the core is unavailable or the
  /// configuration is rejected.
  ///
  /// `config` is a plain value; the native struct is built and freed here so a
  /// caller never has to manage it.
  static ZenithEngineHandle? create({
    int sampleRate = 48000,
    int blockSize = 256,
    int maxChannels = 64,
    int maxTracks = 128,
    int driverKind = ZenithDriverKind.offline,
    int flags = 0,
  }) {
    final bindings = _EngineBindings.tryLoad();
    if (bindings == null) return null;

    final config = calloc<ZenithEngineConfig>();
    final out = calloc<Pointer<Void>>();
    try {
      config.ref
        ..sampleRate = sampleRate
        ..blockSize = blockSize
        ..maxChannels = maxChannels
        ..maxTracks = maxTracks
        ..driverKind = driverKind
        ..flags = flags
        ..abiMajor = 0
        ..reserved = 0;
      if (bindings.create(config, out) != _ok) return null;
      if (out.value == nullptr) return null;
      return ZenithEngineHandle._(bindings, out.value);
    } finally {
      calloc.free(config);
      calloc.free(out);
    }
  }

  /// The negotiated block size, in frames.
  int get blockSize {
    final status = readStatus();
    return status.blockSize;
  }

  /// Reads the current status snapshot. Lock-free on the native side.
  NativeEngineStatus readStatus() {
    final out = calloc<ZenithEngineStatus>();
    try {
      if (_bindings.status(_handle, out) != _ok) {
        return _fallbackStatus();
      }
      return NativeEngineStatus.fromNative(out.ref);
    } finally {
      calloc.free(out);
    }
  }

  /// Starts playback.
  bool play() => _bindings.play(_handle) == _ok;

  /// Pauses in place.
  bool pause() => _bindings.pause(_handle) == _ok;

  /// Stops and rewinds.
  bool stop() => _bindings.stop(_handle) == _ok;

  /// Seeks to a tick position.
  bool seekTicks(int ticks, {int ppq = 960}) {
    final time = _musicalTime(ticks, ppq: ppq);
    try {
      return _bindings.seek(_handle, time.ref) == _ok;
    } finally {
      calloc.free(time);
    }
  }

  /// Sets the loop region, in ticks.
  bool setLoop(int startTicks, int endTicks, {bool enabled = true, int ppq = 960}) {
    final start = _musicalTime(startTicks, ppq: ppq);
    final end = _musicalTime(endTicks, ppq: ppq);
    try {
      return _bindings.setLoop(_handle, start.ref, end.ref, enabled ? 1 : 0) == _ok;
    } finally {
      calloc.free(start);
      calloc.free(end);
    }
  }

  /// Sets the tempo, in beats per minute.
  bool setTempo(double bpm) => _bindings.setTempo(_handle, bpm) == _ok;

  /// Sets the time signature.
  bool setTimeSignature(int numerator, int denominator) =>
      _bindings.setTimeSignature(_handle, numerator, denominator) == _ok;

  /// Renders `frames` interleaved stereo frames into `out`.
  ///
  /// `out` must hold at least `frames * 2` floats. Returns whether the render
  /// succeeded; the samples are written into `out` in place.
  bool render(Float32List out, int frames) {
    final pointer = calloc<Float>(frames * 2);
    try {
      final code = _bindings.render(_handle, pointer, frames, 2);
      if (code != _ok) return false;
      final native = pointer.asTypedList(frames * 2);
      final n = out.length < frames * 2 ? out.length : frames * 2;
      for (var i = 0; i < n; i++) {
        out[i] = native[i];
      }
      return true;
    } finally {
      calloc.free(pointer);
    }
  }

  /// Releases the native engine. Idempotent.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _bindings.destroy(_handle);
  }

  /// Whether [dispose] has run.
  bool get isDisposed => _disposed;

  // ── S4: offline rendering and PDC ──

  /// Renders `[startTicks, endTicks)` to a fresh interleaved stereo buffer.
  ///
  /// Returns the samples as a [Float32List], or `null` when the core is
  /// unavailable or the range is invalid. The native buffer is freed here
  /// (ABI principle P3): the caller never sees the raw pointer.
  ///
  /// This is **not** real-time safe — it drives the transport and allocates the
  /// whole output — so it belongs on a control/worker thread, not the audio
  /// callback.
  Float32List? renderOffline({
    required int startTicks,
    required int endTicks,
    int targetSampleRate = 0,
    int ppq = 960,
  }) {
    final start = _musicalTime(startTicks, ppq: ppq);
    final end = _musicalTime(endTicks, ppq: ppq);
    final outBuffer = calloc<Pointer<Float>>();
    final outFrames = calloc<Size>();
    try {
      final code = _bindings.renderOffline(
        _handle,
        start.ref,
        end.ref,
        targetSampleRate,
        outBuffer,
        outFrames,
      );
      if (code != _ok) return null;
      final pointer = outBuffer.value;
      final frames = outFrames.value;
      if (pointer == nullptr || frames == 0) return null;
      // Copy out before freeing so the returned list owns its bytes.
      final copy = Float32List.fromList(
        pointer.asTypedList(frames * 2),
      );
      _bindings.bufferFree(pointer, frames);
      return copy;
    } finally {
      calloc.free(start);
      calloc.free(end);
      calloc.free(outBuffer);
      calloc.free(outFrames);
    }
  }

  /// The engine's current delay-compensation latency, in samples.
  int pdcLatency() {
    final out = calloc<Uint32>();
    try {
      if (_bindings.pdcLatency(_handle, out) != _ok) return 0;
      return out.value;
    } finally {
      calloc.free(out);
    }
  }
}

/// Reads the `zenith_sizeof_*` values for the S1 structs, or `null` when the
/// core is unavailable.
final class EngineStructSizes {
  /// Creates a size report.
  const EngineStructSizes({
    required this.engineConfig,
    required this.engineStatus,
    required this.musicalTime,
  });

  /// `sizeof(ZenithEngineConfig)`.
  final int engineConfig;

  /// `sizeof(ZenithEngineStatus)`.
  final int engineStatus;

  /// `sizeof(ZenithMusicalTime)`.
  final int musicalTime;

  /// Reads the sizes from the loaded core, or `null` when unavailable.
  static EngineStructSizes? read() {
    final library = ZenithCore.libraryOrNull();
    if (library == null) return null;
    int Function()? query(String name) {
      try {
        return library.lookupFunction<_SizeofNative, _SizeofDart>(name);
      } on Object {
        return null;
      }
    }

    final config = query('zenith_sizeof_engine_config');
    final status = query('zenith_sizeof_engine_status');
    final time = query('zenith_sizeof_musical_time');
    if (config == null || status == null || time == null) return null;
    return EngineStructSizes(
      engineConfig: config(),
      engineStatus: status(),
      musicalTime: time(),
    );
  }
}

/// Resolves the S1 engine symbols once and holds them together.
final class _EngineBindings {
  _EngineBindings._(DynamicLibrary library)
      : create = library.lookupFunction<_CreateNative, _CreateDart>(
          'zenith_engine_create',
        ),
        destroy = library.lookupFunction<_DestroyNative, _DestroyDart>(
          'zenith_engine_destroy',
        ),
        play = library.lookupFunction<_SimpleNative, _SimpleDart>(
          'zenith_transport_play',
        ),
        pause = library.lookupFunction<_SimpleNative, _SimpleDart>(
          'zenith_transport_pause',
        ),
        stop = library.lookupFunction<_SimpleNative, _SimpleDart>(
          'zenith_transport_stop',
        ),
        seek = library.lookupFunction<_SeekNative, _SeekDart>(
          'zenith_transport_seek',
        ),
        setLoop = library.lookupFunction<_LoopNative, _LoopDart>(
          'zenith_transport_set_loop',
        ),
        setTempo = library.lookupFunction<_TempoNative, _TempoDart>(
          'zenith_transport_set_tempo',
        ),
        setTimeSignature = library.lookupFunction<_TimeSigNative, _TimeSigDart>(
          'zenith_transport_set_time_signature',
        ),
        render = library.lookupFunction<_RenderNative, _RenderDart>(
          'zenith_engine_render',
        ),
        status = library.lookupFunction<_StatusNative, _StatusDart>(
          'zenith_engine_status',
        ),
        driverSupported = library
            .lookupFunction<_DriverSupportedNative, _DriverSupportedDart>(
          'zenith_engine_driver_supported',
        ),
        renderOffline = library
            .lookupFunction<_RenderOfflineNative, _RenderOfflineDart>(
          'zenith_render_offline',
        ),
        bufferFree = library.lookupFunction<_BufferFreeNative, _BufferFreeDart>(
          'zenith_buffer_free',
        ),
        pdcLatency = library.lookupFunction<_PdcLatencyNative, _PdcLatencyDart>(
          'zenith_engine_pdc_latency',
        );

  /// Resolves the bindings, or returns `null` when the core is unavailable or a
  /// symbol is missing (a stale library, which the version handshake should
  /// already have caught).
  static _EngineBindings? tryLoad() {
    final library = ZenithCore.libraryOrNull();
    if (library == null) return null;
    try {
      return _EngineBindings._(library);
    } on Object {
      return null;
    }
  }

  final _CreateDart create;
  final _DestroyDart destroy;
  final _SimpleDart play;
  final _SimpleDart pause;
  final _SimpleDart stop;
  final _SeekDart seek;
  final _LoopDart setLoop;
  final _TempoDart setTempo;
  final _TimeSigDart setTimeSignature;
  final _RenderDart render;
  final _StatusDart status;
  final _DriverSupportedDart driverSupported;
  final _RenderOfflineDart renderOffline;
  final _BufferFreeDart bufferFree;
  final _PdcLatencyDart pdcLatency;
}

/// Whether the core can use `kind` on this build/platform.
bool engineDriverSupported(int kind) {
  final bindings = _EngineBindings.tryLoad();
  if (bindings == null) return false;
  return bindings.driverSupported(kind) != 0;
}

NativeEngineStatus _fallbackStatus() => const NativeEngineStatus(
      playheadFrames: 0,
      bpm: 120,
      cpuLoad: 0,
      xrunCount: 0,
      activeVoices: 0,
      maxVoices: 0,
      degradeLevel: 0,
      state: ZenithTransportState.stopped,
      driverKind: ZenithDriverKind.offline,
      sampleRate: 48000,
      blockSize: 256,
    );

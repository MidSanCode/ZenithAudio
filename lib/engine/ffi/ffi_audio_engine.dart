/// An [AudioEngine] backed by the Rust core over `dart:ffi` (S1).
///
/// ## Why this is not yet the provider default
///
/// The plan's S1.1 step is to repoint `audioEngineProvider` at the FFI engine.
/// That requires a **device driver** so the engine is audible, and the core's
/// `cpal` driver is an optional feature that needs an external crate — not
/// available in the environment this was written in. Without it,
/// `zenith_engine_start` correctly returns `Unsupported` and a live swap would
/// leave the application silent.
///
/// So this class is the finished FFI implementation, ready to become the
/// default the moment a device driver is compiled in, but the provider keeps
/// delegating to the legacy service until then. That keeps the "do not break
/// existing behaviour" rule (S0 acceptance) intact while the engine itself is
/// landed and tested.
///
/// ## Transport mapping
///
/// The Rust transport works in frames and ticks; this adapter presents the
/// frame-based [AudioEngine] surface and converts to ticks for seek/loop using
/// the engine's own tempo, so the two never disagree.
library;

import 'dart:async';
import 'dart:typed_data';

import '../engine.dart';
import 'engine_bindings.dart';

/// An [AudioEngine] implemented by the native core.
class FfiAudioEngine implements AudioEngine {
  FfiAudioEngine._(this._handle) {
    // Publish a status snapshot on a UI-friendly interval. The core's snapshot
    // is lock-free; a timer is only needed to turn polls into a stream.
    _positionTimer = Timer.periodic(
      const Duration(milliseconds: 33),
      (_) => _emitPosition(),
    );
  }

  /// Creates an engine, or returns `null` when the native core is unavailable
  /// or rejects the configuration.
  static FfiAudioEngine? create({
    int sampleRate = 48000,
    int blockSize = 256,
    int maxChannels = 64,
    int maxTracks = 128,
  }) {
    final handle = ZenithEngineHandle.create(
      sampleRate: sampleRate,
      blockSize: blockSize,
      maxChannels: maxChannels,
      maxTracks: maxTracks,
    );
    if (handle == null) return null;
    return FfiAudioEngine._(handle);
  }

  final ZenithEngineHandle _handle;
  Timer? _positionTimer;
  bool _disposed = false;

  final StreamController<TransportPosition> _positionController =
      StreamController<TransportPosition>.broadcast();
  final StreamController<LevelSnapshot> _levelController =
      StreamController<LevelSnapshot>.broadcast();

  int _sampleRate = 48000;
  int _blockSize = 256;

  @override
  int get sampleRate {
    final status = _handle.readStatus();
    if (status.sampleRate > 0) _sampleRate = status.sampleRate;
    return _sampleRate;
  }

  @override
  int get blockSize {
    final status = _handle.readStatus();
    if (status.blockSize > 0) _blockSize = status.blockSize;
    return _blockSize;
  }

  @override
  SampleFormat get sampleFormat => SampleFormat.float32;

  @override
  bool get isPlaying => _handle.readStatus().isPlaying;

  @override
  TransportState get transportState {
    final status = _handle.readStatus();
    if (status.isPlaying) return TransportState.playing;
    if (status.isPaused) return TransportState.paused;
    return TransportState.stopped;
  }

  @override
  Stream<TransportPosition> get positionStream => _positionController.stream;

  @override
  Stream<LevelSnapshot> get levelStream => _levelController.stream;

  @override
  Future<void> initialize() async {
    // The native engine is prepared at creation; the version handshake is the
    // provider's responsibility. Nothing to do here.
  }

  @override
  Future<void> play(PlaybackRequest request) async {
    if (request.startFrame != 0) {
      await seekToFrame(request.startFrame);
    }
    if (request.isLooping) {
      final status = _handle.readStatus();
      final bpm = status.bpm > 0 ? status.bpm : 120.0;
      final startTicks = _framesToTicks(request.loopStartFrame ?? 0, bpm);
      final endTicks = _framesToTicks(request.loopEndFrame ?? 0, bpm);
      _handle.setLoop(startTicks, endTicks);
    }
    _handle.play();
  }

  @override
  Future<void> pause() async {
    _handle.pause();
  }

  @override
  Future<void> stop() async {
    _handle.stop();
  }

  @override
  Future<void> seekToFrame(int frame) async {
    final status = _handle.readStatus();
    final bpm = status.bpm > 0 ? status.bpm : 120.0;
    _handle.seekTicks(_framesToTicks(frame, bpm));
  }

  @override
  void setMasterGain(double gain) {
    // Master gain is a parameter in the S2 store, addressed as a global
    // parameter once the mixer wiring is complete. Until then there is no
    // engine-level gain entry point, so this is a documented no-op rather than
    // a fabricated one.
  }

  @override
  void setTempo(double bpm) {
    _handle.setTempo(bpm);
  }

  @override
  Future<void> shutdown() async {
    if (_disposed) return;
    _disposed = true;
    _positionTimer?.cancel();
    await _positionController.close();
    await _levelController.close();
    _handle.dispose();
  }

  /// Renders `frames` interleaved stereo frames, for offline use and tests.
  ///
  /// Not part of the [AudioEngine] interface: a live engine is pulled by its
  /// driver inside Rust, not by Dart. This exists so a test can verify the
  /// native engine end to end without an audio device.
  bool renderForTest(Float32List out, int frames) => _handle.render(out, frames);

  void _emitPosition() {
    if (_disposed || _positionController.isClosed) return;
    final status = _handle.readStatus();
    _positionController.add(TransportPosition(
      frame: status.playheadFrames,
      state: transportState,
      sampleRate: sampleRate,
    ));
  }

  int _framesToTicks(int frames, double bpm) {
    const ppq = 960;
    if (_sampleRate <= 0) return 0;
    return (frames * bpm * ppq / (_sampleRate * 60.0)).round();
  }
}

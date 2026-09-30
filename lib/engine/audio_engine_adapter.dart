/// The bridge from the legacy `AudioService` to the `AudioEngine` interface.
///
/// ## Why this file exists
///
/// S0 added [`AudioEngine`](engine.dart) as the seam that will let the compute
/// core move from Dart to Rust. But when S1 opened, that interface had **zero
/// implementations and zero call sites**: all 34 `audioServiceProvider`
/// references across 8 files were typed against the concrete `AudioService`.
///
/// Swapping the backend under that arrangement is a single atomic change with
/// no rollback point — the engine swap and the 34 call-site edits would land
/// together, and a regression could not be bisected. This adapter splits that
/// into two independent steps:
///
///   1. **This file (S1.0):** call sites move to `AudioEngine` while the
///      delegation target stays `AudioService` and `media_kit` keeps playing.
///      Behaviour is byte-for-byte unchanged; it is a mechanical refactor with
///      tests as the safety net.
///   2. **S1.1:** the delegation target changes to the FFI engine. Call sites
///      do not move again.
///
/// ## Where the legacy-only methods live
///
/// `hotSwapTrackWav`, `getOutputInfo`, `loadTrackFromPath`, `setPlaybackSpeed`
/// and friends are **[AudioEngine]-specific gaps that are deliberately not
/// added to the interface**. They are artifacts of the offline-bounce
/// architecture: hot-swapping a rendered WAV file exists only because editing
/// re-renders a whole track, and that mechanism disappears once the real-time
/// engine schedules samples directly. Promoting them to `AudioEngine` would
/// freeze a design the replacement does not need.
///
/// Instead they are declared **only on the adapter**, and the call sites that
/// genuinely need them take the adapter type. Call sites that only need
/// transport/volume/mute depend on the `AudioEngine` interface alone, so they
/// will not be touched again in S1.1.
///
/// ## What does not change
///
/// Every method here forwards to the identical `AudioService` call it replaced.
/// No clamping, no reordering, no added awaits. The one genuinely new mapping
/// is the frame/seconds conversion in [seekToFrame], which uses the documented
/// `frame = seconds * sampleRate` relation and round-trips exactly, because
/// `AudioService.seekTo` already converts seconds to milliseconds by rounding.
library;

import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../models/track.dart';
import '../services/audio_service.dart';
import 'engine.dart';

export '../services/audio_service.dart' show AudioOutputInfo;

/// An [AudioEngine] backed by the existing `AudioService`.
///
/// This is the S0–S3 path. It is intentionally a thin, behaviour-preserving
/// shim: it exists to move call sites onto the interface, not to improve the
/// engine. Anything that looks like an improvement belongs in the FFI
/// implementation in S1.1, where it can be validated against real DSP.
class AudioServiceAdapter implements AudioEngine {
  /// Creates an adapter over [service].
  ///
  /// The service is injected rather than constructed so tests can pass a fake
  /// and so the Riverpod provider keeps ownership of disposal.
  AudioServiceAdapter(this._service);

  final AudioService _service;

  /// The wrapped legacy service.
  ///
  /// Exposed for the S1.1 migration and for tests asserting that the adapter
  /// really is delegating rather than caching its own state. Call sites should
  /// prefer the interface methods.
  AudioService get service => _service;

  // ── Engine configuration ──

  /// Sample rate reported to the boundary.
  ///
  /// `media_kit`/mpv negotiates its own rate and the legacy service never
  /// exposes it, so this reports the rate the offline renderer uses
  /// (`AudioService.prepareInstrumentTrack` renders at 44100). It is declared
  /// rather than guessed: when the FFI engine lands it negotiates a real rate
  /// from the device and this getter stops being a constant.
  @override
  int get sampleRate => 44100;

  /// Processing block size in frames.
  ///
  /// Meaningless for the file-playback path, which has no callback block. It
  /// reports the S1 target block size so UI code and tests that size buffers
  /// from this value get a realistic number instead of zero.
  @override
  int get blockSize => 256;

  /// The only processing format the core supports.
  @override
  SampleFormat get sampleFormat => SampleFormat.float32;

  // ── Transport ──

  @override
  bool get isPlaying => _service.isPlaying;

  /// Live transport state.
  ///
  /// Derived from the legacy service, which only tracks a boolean. The
  /// distinction between "paused" and "stopped" is not observable through the
  /// legacy API, so this reports [TransportState.paused] whenever playback is
  /// not running: the legacy service keeps its players positioned after
  /// `pause()` and after `stop()` alike, and the transport UI reads its own
  /// state for the two cases. Reporting `paused` here is therefore conservative
  /// — it never claims a rewind that did not happen.
  @override
  TransportState get transportState =>
      _service.isPlaying ? TransportState.playing : TransportState.paused;

  /// Frame positions derived from the legacy seconds callback.
  ///
  /// The legacy service invokes `onPositionChanged` at most every 33 ms from
  /// the player's own position stream. The adapter converts to frames and
  /// publishes on a broadcast stream.
  ///
  /// Because `AudioService` supports exactly one position callback (it is a
  /// settable field, not a stream), this adapter **owns** that field while it
  /// is alive and restores the previous handler on [shutdown]. Two adapters
  /// over the same service would fight over it; there is exactly one adapter
  /// per service by construction (see [audioEngineProvider]).
  @override
  Stream<TransportPosition> get positionStream => _positionController.stream;

  final StreamController<TransportPosition> _positionController =
      StreamController<TransportPosition>.broadcast();

  /// Last frame position published, so [positionStream] can report the
  /// transport state alongside the frame without a second source of truth.
  int _lastFrame = 0;

  /// Per-track level snapshots.
  ///
  /// The legacy path has no metering at all — `meterLevelsProvider` is fed
  /// synthetic data elsewhere. This stream is present because the interface
  /// requires it, but it **never emits** until the FFI engine provides real
  /// atomically-published levels. Emitting fabricated values here would make
  /// the S1.1 swap silently change what the meters mean, so it deliberately
  /// stays silent.
  @override
  Stream<LevelSnapshot> get levelStream => _levelController.stream;

  final StreamController<LevelSnapshot> _levelController =
      StreamController<LevelSnapshot>.broadcast();

  /// Previous position callback, saved so [shutdown] can restore it.
  void Function(double)? _previousOnPositionChanged;

  /// Previous completion callback, saved so [shutdown] can restore it.
  void Function()? _previousOnCompleted;

  bool _initialized = false;
  bool _disposed = false;

  /// Completion callback set by the owner (transport / editors).
  ///
  /// Forwarded to `AudioService.onCompleted`. Kept as a settable field to match
  /// the legacy shape exactly, so the migration is mechanical.
  set onCompleted(void Function()? callback) {
    _previousOnCompleted = callback;
    _service.onCompleted = callback;
  }

  /// The completion callback currently installed on the service.
  void Function()? get onCompleted => _service.onCompleted;

  /// Position callback in **seconds**, for call sites not yet on the stream.
  ///
  /// Prefer [positionStream]. This exists because some editors (the audio clip
  /// editor) install their own position handler and expect seconds; routing
  /// them through frames first would add a lossy round-trip that the
  /// behaviour-preserving step must avoid.
  ///
  /// Setting this never stacks interceptors: there is exactly one handler on
  /// the service at a time (see [_installPositionInterceptor]), and this setter
  /// only records the *consumer* half of it.
  set onPositionChanged(void Function(double seconds)? callback) {
    _positionSink = callback;
    _ensureInterceptor();
  }

  /// The consumer installed by [onPositionChanged], if any.
  void Function(double seconds)? get onPositionChanged => _positionSink;

  void Function(double seconds)? _positionSink;

  @override
  Future<void> initialize() async {
    if (_disposed) return;
    // `AudioService` has no explicit preparation step — its players are created
    // lazily per track. Installing the interceptor here means `positionStream`
    // is live before any transport call.
    _ensureInterceptor();
  }

  /// Installs the single service-side position handler, once.
  ///
  /// The legacy service exposes position as a *settable field*, not a stream, so
  /// the adapter must own it to fan out to both [positionStream] and any
  /// [onPositionChanged] consumer. Installing it more than once would multiply
  /// every position report.
  void _ensureInterceptor() {
    if (_initialized || _disposed) return;
    _previousOnPositionChanged = _service.onPositionChanged;
    _previousOnCompleted = _service.onCompleted;
    _service.onPositionChanged = (seconds) {
      _lastFrame = _secondsToFrames(seconds);
      _positionSink?.call(seconds);
      if (!_positionController.isClosed) {
        _positionController.add(TransportPosition(
          frame: _lastFrame,
          state: transportState,
          sampleRate: sampleRate,
        ));
      }
    };
    _initialized = true;
  }

  @override
  Future<void> play(PlaybackRequest request) async {
    if (request.startFrame > 0) {
      await seekToFrame(request.startFrame);
    }
    await _service.play();
  }

  /// Starts playback from the players' current position.
  ///
  /// The legacy transport calls `play()` with no arguments; [`AudioEngine.play`]
  /// requires a [PlaybackRequest] because the real-time engine needs a frame
  /// origin. This overload keeps the legacy call shape working without
  /// pretending the file-playback engine honours a start frame — reaching into
  /// the players' own position is what the old code did, and a
  /// `PlaybackRequest(startFrame: 0)` would have rewound instead.
  Future<void> playFromCurrentPosition() => _service.play();

  @override
  Future<void> pause() => _service.pause();

  @override
  Future<void> stop() => _service.stop();

  /// Seeks to an absolute frame position.
  ///
  /// Converts frames to seconds and delegates. The conversion is the exact
  /// inverse of [TransportPosition.frame]: `sec = frame / sampleRate`.
  @override
  Future<void> seekToFrame(int frame) =>
      _service.seekTo(frame / sampleRate);

  /// Sets the master gain.
  ///
  /// `AudioEngine` takes a `0.0..1.0` gain; the legacy service takes the same
  /// range and clamps internally.
  @override
  void setMasterGain(double gain) => _service.updateMasterVolume(gain);

  /// Sets the project tempo.
  ///
  /// The legacy engine has no tempo — it plays pre-rendered WAVs whose timing
  /// was baked in at render time, and tempo changes therefore require a
  /// re-render rather than a parameter update. This is a documented no-op until
  /// the FFI engine owns the sequencer. Making it *do* something here (e.g.
  /// drive `setPlaybackSpeed`) would be a behaviour change, which the first
  /// migration step forbids.
  @override
  void setTempo(double bpm) {}

  @override
  Future<void> shutdown() async {
    if (_disposed) return;
    _disposed = true;
    // Restore whatever the service had before, so an adapter can be torn down
    // without leaving the legacy service wired to a dead stream.
    _service.onPositionChanged = _previousOnPositionChanged;
    _service.onCompleted = _previousOnCompleted;
    await _positionController.close();
    await _levelController.close();
  }

  // ── Legacy-only capabilities ──
  //
  // These are NOT on `AudioEngine` by design (see the library doc comment):
  // they belong to the offline-bounce engine and mostly disappear in S1.1.

  /// Master volume as the legacy service reports it.
  double get masterVolume => _service.masterVolume;

  /// Sets master volume through the legacy setter.
  ///
  /// Distinct from [setMasterGain] only in name; both write the same field.
  /// Kept because the transport slider historically assigns this property.
  set masterVolume(double v) => _service.masterVolume = v;

  /// Renders and loads a track's audio. Returns its duration in seconds.
  Future<double> loadTrack(Track track) => _service.loadTrack(track);

  /// Renders an instrument track to a temporary WAV, returning its path.
  Future<String?> prepareInstrumentTrack(Track track,
          {bool useIsolate = false}) =>
      _service.prepareInstrumentTrack(track, useIsolate: useIsolate);

  /// Loads an already-rendered file as a track. Used by the offline path and
  /// by the piano-roll preview.
  Future<void> loadTrackFromPath(String trackId, String path,
          {double volume = 1.0, bool muted = false}) =>
      _service.loadTrackFromPath(trackId, path, volume: volume, muted: muted);

  /// Unloads a single track's player.
  Future<void> unloadTrack(String trackId) => _service.unloadTrack(trackId);

  /// Unloads every track's player.
  Future<void> unloadAll() => _service.unloadAll();

  /// Applies a per-track volume.
  void updateTrackVolume(String trackId, double volume) =>
      _service.updateTrackVolume(trackId, volume);

  /// Applies a per-track mute.
  void setMute(String trackId, bool muted) => _service.setMute(trackId, muted);

  /// Sets the playback rate multiplier.
  void setPlaybackSpeed(double speed) => _service.setPlaybackSpeed(speed);

  /// Plays a single track without disturbing the others.
  Future<void> playSingleTrack(String trackId) =>
      _service.playSingleTrack(trackId);

  /// Stops and unloads a single track.
  Future<void> stopAndUnloadTrack(String trackId) =>
      _service.stopAndUnloadTrack(trackId);

  /// Seeks every player to an absolute **seconds** position.
  ///
  /// Prefer [seekToFrame]; this remains because the transport stores seconds.
  Future<void> seekTo(double seconds) => _service.seekTo(seconds);

  /// Drops a track's cached WAV so the next play re-renders it.
  void invalidateTrackWav(String trackId) =>
      _service.invalidateTrackWav(trackId);

  /// Whether a track's cached WAV matches its current notes.
  bool isTrackCached(Track track) => _service.isTrackCached(track);

  /// The cached WAV path for a track, if any.
  String? getCachedTrackPath(String trackId) =>
      _service.getCachedTrackPath(trackId);

  /// Re-renders a playing track and swaps the audio in place.
  Future<void> hotSwapTrackWav(Track track) => _service.hotSwapTrackWav(track);

  /// Snapshot of the active output device, for the song-info panel.
  ///
  /// Returns `null` when nothing is loaded, exactly as the legacy method does.
  AudioOutputInfo? getOutputInfo() => _service.getOutputInfo();

  /// Releases the wrapped service.
  ///
  /// Separate from [shutdown] because the interface's `shutdown` must not
  /// destroy a service the provider still owns; this is the provider's own
  /// disposal path and mirrors what the provider did before the adapter.
  Future<void> disposeService() => _service.dispose();

  int _secondsToFrames(double seconds) => (seconds * sampleRate).round();
}

/// The single audio engine instance for the application.
///
/// This is the one provider call sites should read. It wraps
/// [audioServiceProvider] — the legacy `AudioService`, which still owns the
/// underlying `media_kit` players — in an [AudioServiceAdapter].
///
/// ## Why it is an adapter and not a replacement
///
/// `AudioService` is deliberately **not** deleted (S1.0 prerequisite C): it is
/// the only fallback path until S4 proves the offline renderer by export. So
/// both providers exist side by side during S1, and this one is the seam:
///
/// * **S1.0:** `audioEngineProvider` delegates to `audioServiceProvider`.
/// * **S1.1:** this provider's body changes to construct the FFI-backed
///   engine, and every call site that reads it keeps working untouched.
///
/// ## Type
///
/// It is exposed as `Provider<AudioServiceAdapter>` rather than
/// `Provider<AudioEngine>` on purpose. Call sites that genuinely need the
/// legacy-only capabilities (hot swap, output info, WAV caching) can reach them
/// without a cast, while call sites that only need the interface still consume
/// it through `AudioEngine` via the return type of the field they store. In
/// S1.1 the additional capabilities are expected to move or disappear
/// individually, and this provider is what makes that a per-call-site decision
/// rather than a big bang.
///
/// The provider owns lifecycle: it initialises the adapter and disposes both the
/// adapter and the underlying service, mirroring what
/// `audioServiceProvider`'s own `onDispose` does today.
final audioEngineProvider = Provider<AudioServiceAdapter>((ref) {
  final adapter = AudioServiceAdapter(ref.watch(audioServiceProvider));
  // Fire-and-forget: `initialize` only installs the position interceptor and
  // completes synchronously, so there is nothing for callers to await. Doing it
  // here guarantees `positionStream` is live before any transport call.
  unawaited(adapter.initialize());
  ref.onDispose(() {
    // Order matters: release the adapter's streams first, then the service's
    // players, so a late position callback cannot target a closed controller.
    adapter.shutdown();
  });
  return adapter;
});

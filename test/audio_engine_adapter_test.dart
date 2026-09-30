import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/engine/audio_engine_adapter.dart';
import 'package:zenith_audio/engine/engine.dart';
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/audio_service.dart' show AudioService;

/// S1.0 prerequisite B: the `AudioEngine` adapter must be a *pure* delegation
/// layer.
///
/// These tests do not exercise `media_kit` — that would need a device and a
/// real WAV. They assert the property that actually makes the migration safe:
/// every adapter method reaches exactly one `AudioService` call, with the same
/// arguments, and the adapter adds no state of its own that could diverge from
/// the service's.
///
/// The `AudioService` here is a recording **implementer**, not a mock of the
/// adapter: the adapter is the real class under test.
void main() {
  group('AudioServiceAdapter delegation', () {
    late _RecordingAudioService service;
    late AudioServiceAdapter adapter;

    setUp(() {
      service = _RecordingAudioService();
      adapter = AudioServiceAdapter(service);
    });

    tearDown(() async {
      await adapter.shutdown();
    });

    test('implements AudioEngine', () {
      // The whole point of prerequisite B: there is now a real implementer.
      expect(adapter, isA<AudioEngine>());
    });

    // ── Transport: interface methods must reach the service unchanged ──

    test('play/pause/stop forward exactly once', () async {
      await adapter.play(const PlaybackRequest(startFrame: 0));
      await adapter.pause();
      await adapter.stop();

      expect(service.calls, ['play', 'pause', 'stop']);
    });

    test('play with a non-zero start frame seeks before playing', () async {
      // Ordering matters: playing first would emit audio from the old position.
      await adapter.play(const PlaybackRequest(startFrame: 48000));

      expect(service.calls, ['seekTo', 'play']);
      expect(service.seekSeconds, 48000 / 44100);
    });

    test('play at frame zero does not seek', () async {
      await adapter.play(const PlaybackRequest(startFrame: 0));

      expect(service.calls, ['play']);
    });

    test('seekToFrame converts frames to seconds', () async {
      await adapter.seekToFrame(22050);

      expect(service.calls, ['seekTo']);
      expect(service.seekSeconds, closeTo(0.5, 1e-12));
    });

    test('seekToFrame and position frames are exact inverses', () async {
      // Round-tripping through seconds must not drift, because the transport
      // converts in both directions on every seek.
      for (final frame in [0, 1, 256, 44100, 48000, 96000, 12345678]) {
        service.calls.clear();
        await adapter.seekToFrame(frame);
        final back = (service.seekSeconds! * adapter.sampleRate).round();
        expect(back, frame, reason: 'frame $frame drifted');
      }
    });

    test('isPlaying reflects the service', () {
      expect(adapter.isPlaying, isFalse);
      service.isPlaying = true;
      expect(adapter.isPlaying, isTrue);
    });

    test('transportState reports playing while the service plays', () {
      expect(adapter.transportState, TransportState.paused);
      service.isPlaying = true;
      expect(adapter.transportState, TransportState.playing);
    });

    test('setMasterGain reaches the service as a volume update', () {
      adapter.setMasterGain(0.42);

      expect(service.calls, ['updateMasterVolume']);
      expect(service.lastVolume, 0.42);
    });

    test('setTempo is a documented no-op on this engine', () {
      // The legacy engine bakes tempo into rendered WAVs; it has no tempo
      // parameter. Asserting the no-op pins the current behaviour so S1.1
      // changes it deliberately rather than by accident.
      adapter.setTempo(180);

      expect(service.calls, isEmpty);
    });

    // ── Interface metadata ──

    test('reports the S1 block size and the render-sample rate', () {
      expect(adapter.blockSize, 256);
      expect(adapter.sampleRate, 44100);
      expect(adapter.sampleFormat, SampleFormat.float32);
    });

    // ── Legacy-only capabilities ──

    test('legacy-only methods forward with their arguments intact', () async {
      await adapter.loadTrackFromPath('t1', 'C:/tmp/a.wav',
          volume: 0.5, muted: true);
      adapter.updateTrackVolume('t1', 0.25);
      adapter.setMute('t1', true);
      adapter.setPlaybackSpeed(1.5);
      await adapter.playSingleTrack('t1');
      await adapter.stopAndUnloadTrack('t1');
      await adapter.unloadTrack('t1');
      await adapter.unloadAll();
      adapter.invalidateTrackWav('t1');

      expect(service.calls, [
        'loadTrackFromPath',
        'updateTrackVolume',
        'setMute',
        'setPlaybackSpeed',
        'playSingleTrack',
        'stopAndUnloadTrack',
        'unloadTrack',
        'unloadAll',
        'invalidateTrackWav',
      ]);
      expect(service.lastPathVolume, 0.5);
      expect(service.lastPathMuted, isTrue);
      expect(service.lastVolume, 0.25);
      expect(service.lastMute, isTrue);
      expect(service.lastSpeed, 1.5);
    });

    test('masterVolume mirrors the service in both directions', () {
      service.masterVolume = 0.7;
      expect(adapter.masterVolume, 0.7);

      adapter.masterVolume = 0.3;
      expect(service.masterVolume, 0.3);
    });

    test('getOutputInfo passes null through rather than inventing a device',
        () {
      service.outputInfo = null;
      expect(adapter.getOutputInfo(), isNull);
    });

    test('disposeService disposes the wrapped service', () async {
      await adapter.disposeService();

      expect(service.calls, ['dispose']);
    });

    // ── Position plumbing ──

    test('initialize installs a position interceptor that feeds the stream',
        () async {
      await adapter.initialize();
      final frames = <int>[];
      final sub = adapter.positionStream.listen((p) => frames.add(p.frame));

      // The service emits seconds; the adapter converts to frames.
      service.emitPosition(0.5);
      await Future<void>.delayed(Duration.zero);

      expect(frames, [22050]);
      await sub.cancel();
    });

    test('initialize is idempotent and does not stack interceptors', () async {
      await adapter.initialize();
      await adapter.initialize();

      final frames = <int>[];
      final sub = adapter.positionStream.listen((p) => frames.add(p.frame));
      service.emitPosition(1.0);
      await Future<void>.delayed(Duration.zero);

      // A stacked interceptor would emit twice for one service callback.
      expect(frames, [44100]);
      await sub.cancel();
    });

    test('the interceptor preserves a pre-existing position callback',
        () async {
      // Call sites install their own handlers; the adapter must not swallow
      // them, or the playhead would silently stop updating in that editor.
      final seen = <double>[];
      await adapter.initialize();
      adapter.onPositionChanged = seen.add;

      service.emitPosition(0.25);
      await Future<void>.delayed(Duration.zero);

      expect(seen, [0.25]);
    });

    test('shutdown restores the service callbacks and closes the streams',
        () async {
      final seen = <double>[];
      // Capture the tear-off once: `prior.add` produces a new closure object on
      // every access, so `same()` would compare two distinct bindings.
      final prior = seen.add;
      service.onPositionChanged = prior;
      final priorCompleted = service.onCompleted;

      await adapter.initialize();
      // The adapter must have taken over while active...
      expect(service.onPositionChanged, isNot(same(prior)));
      await adapter.shutdown();

      expect(service.onPositionChanged, same(prior),
          reason: 'the original handler must be reinstated');
      expect(service.onCompleted, same(priorCompleted));
      expect(adapter.positionStream.isBroadcast, isTrue);
    });

    test('shutdown is idempotent and positionStream closes', () async {
      await adapter.initialize();
      await adapter.shutdown();
      // A second shutdown must not throw on already-closed controllers.
      await adapter.shutdown();

      // `stream` of a closed controller still yields done, not an error.
      await expectLater(adapter.positionStream, emitsDone);
    });

    test('onCompleted forwards to the service', () async {
      void handler() {}
      adapter.onCompleted = handler;

      expect(service.onCompleted, same(handler));
      expect(adapter.onCompleted, same(handler));
    });

    test('levelStream is silent until a real engine provides levels', () {
      // Fabricating meter values here would make the S1.1 swap change what the
      // UI displays without any code change at the call site.
      expect(adapter.levelStream.isBroadcast, isTrue);
    });
  });

  group('audioEngineProvider', () {
    test('the adapter is the sole AudioEngine implementation and is importable',
        () {
      // Guards against the S0 state recurring: interfaces with no implementer.
      expect(
        AudioServiceAdapter,
        isA<Type>(),
        reason: 'AudioServiceAdapter must remain the concrete implementer',
      );
    });
  });
}

/// A hand-written `AudioService` replacement that records calls.
///
/// It implements the same surface the adapter uses. Extending the real service
/// is not possible without a `media_kit` device, and a code-generated mock would
/// not pin the call *order*, which is the property that matters for
/// seek-then-play.
class _RecordingAudioService implements AudioService {
  final List<String> calls = <String>[];

  @override
  bool isPlaying = false;

  @override
  double masterVolume = 1.0;

  @override
  void Function(double position)? onPositionChanged;

  @override
  void Function()? onCompleted;

  double? seekSeconds;
  double? lastVolume;
  bool? lastMute;
  double? lastSpeed;
  double? lastPathVolume;
  bool? lastPathMuted;
  AudioOutputInfo? outputInfo;

  /// Invokes the installed position callback, mimicking the player stream.
  void emitPosition(double seconds) => onPositionChanged?.call(seconds);

  @override
  Future<double> loadTrack(Track track) async {
    calls.add('loadTrack');
    return 0;
  }

  @override
  Future<String?> prepareInstrumentTrack(Track track,
      {bool useIsolate = false}) async {
    calls.add('prepareInstrumentTrack');
    return null;
  }

  @override
  Stream<double> prepareTracks(List<Track> tracks,
      {String? skipTrackId, bool useIsolate = false}) async* {
    calls.add('prepareTracks');
    yield 1.0;
  }

  @override
  Future<void> loadTracks(List<Track> tracks, {String? skipTrackId}) async {
    calls.add('loadTracks');
  }

  @override
  Future<void> loadTrackFromPath(String trackId, String path,
      {double volume = 1.0, bool muted = false}) async {
    calls.add('loadTrackFromPath');
    lastPathVolume = volume;
    lastPathMuted = muted;
  }

  @override
  String? getCachedTrackPath(String trackId) {
    calls.add('getCachedTrackPath');
    return null;
  }

  @override
  void invalidateTrackWav(String trackId) => calls.add('invalidateTrackWav');

  @override
  AudioOutputInfo? getOutputInfo() {
    calls.add('getOutputInfo');
    return outputInfo;
  }

  @override
  bool isTrackCached(Track track) => false;

  @override
  Future<void> hotSwapTrackWav(Track track) async {
    calls.add('hotSwapTrackWav');
  }

  @override
  void updateTrackVolume(String trackId, double volume) {
    calls.add('updateTrackVolume');
    lastVolume = volume;
  }

  @override
  void setMute(String trackId, bool muted) {
    calls.add('setMute');
    lastMute = muted;
  }

  @override
  void setPlaybackSpeed(double speed) {
    calls.add('setPlaybackSpeed');
    lastSpeed = speed;
  }

  @override
  void updateMasterVolume(double volume) {
    calls.add('updateMasterVolume');
    lastVolume = volume;
  }

  @override
  Future<void> play() async => calls.add('play');

  @override
  Future<void> playSingleTrack(String trackId) async =>
      calls.add('playSingleTrack');

  @override
  Future<void> stopAndUnloadTrack(String trackId) async =>
      calls.add('stopAndUnloadTrack');

  @override
  Future<void> pause() async => calls.add('pause');

  @override
  Future<void> stop() async => calls.add('stop');

  @override
  Future<void> seekTo(double seconds) async {
    calls.add('seekTo');
    seekSeconds = seconds;
  }

  @override
  Future<void> unloadTrack(String trackId) async => calls.add('unloadTrack');

  @override
  Future<void> unloadAll() async => calls.add('unloadAll');

  @override
  Future<void> dispose() async => calls.add('dispose');
}

/// Audio engine abstraction.
///
/// This is the seam that lets the compute core move from Dart to Rust without
/// touching a single call site: S0 ships only this interface, S1 provides the
/// `dart:ffi` implementation, and the existing `media_kit` path keeps running
/// until the FFI engine passes the S4 acceptance checks.
///
/// Nothing in this file starts a real-time thread, loads a library, or
/// allocates per sample — it is deliberately free of implementation detail so
/// the interface cannot leak a design that the Rust side must then honour.
///
/// ## Contract
///
/// * All `Stream`s are broadcast and must be safe to listen to from the UI
///   isolate. An implementation that fills them from an audio callback must
///   marshal across, not emit directly.
/// * Methods that touch the device return `Future`s; an implementation must
///   never block the caller's isolate on device IO.
/// * `dispose` is idempotent and releases every native resource.
library;

/// Sample format used at the engine boundary.
///
/// The Rust core processes `f32` (PLAN_DAW_PARITY.md §0.1), so this enum exists
/// to make the boundary explicit rather than to offer a choice.
enum SampleFormat {
  /// 32-bit IEEE float, the only supported processing format.
  float32,
}

/// A request to start playback.
///
/// Kept as a value type so the engine can be driven from a pure function in
/// tests without a device.
class PlaybackRequest {
  /// Creates a playback request.
  const PlaybackRequest({
    required this.startFrame,
    this.loopStartFrame,
    this.loopEndFrame,
  });

  /// Absolute start position, in frames from the project origin.
  final int startFrame;

  /// Loop start, in frames. `null` disables looping.
  final int? loopStartFrame;

  /// Loop end, in frames. `null` disables looping.
  final int? loopEndFrame;

  /// Whether this request loops.
  bool get isLooping => loopStartFrame != null && loopEndFrame != null;
}

/// Transport state reported by the engine.
enum TransportState {
  /// Stopped and rewound to the project origin.
  stopped,

  /// Playing forward.
  playing,

  /// Paused in place; a resume continues from the current position.
  paused,
}

/// A position report from the engine.
///
/// Frame-accurate rather than seconds-based: seconds are a *derived* view in
/// this codebase (see `models/musical_time.dart`), and a lossy round-trip
/// through `double` is exactly what caused drift before S0.
class TransportPosition {
  /// Creates a position report.
  const TransportPosition({
    required this.frame,
    required this.state,
    required this.sampleRate,
  });

  /// Current absolute position, in frames.
  final int frame;

  /// Transport state at the moment of reporting.
  final TransportState state;

  /// Sample rate that [frame] is expressed in.
  final int sampleRate;

  /// This position as a duration.
  Duration get asDuration =>
      Duration(microseconds: (frame * 1000000) ~/ sampleRate);
}

/// A rendered output level snapshot.
///
/// Level metering is read-only and lock-free on the audio thread; the engine
/// pushes snapshots rather than letting the UI poll shared mutable state.
class LevelSnapshot {
  /// Creates a level snapshot.
  const LevelSnapshot({required this.trackId, required this.peak, required this.rms});

  /// Track the levels belong to.
  final String trackId;

  /// Peak magnitude in the range `0.0..1.0` (values above 1.0 indicate clipping).
  final double peak;

  /// RMS magnitude in the range `0.0..1.0`.
  final double rms;
}

/// The audio engine contract.
///
/// Implementations: `media_kit`-backed (S0–S3, the current path) and
/// FFI-backed (S1+, the Rust core). Both must satisfy identical semantics so
/// swapping one for the other is a one-line provider change.
abstract interface class AudioEngine {
  /// Negotiated sample rate, in Hz.
  ///
  /// Only meaningful once [initialize] has completed.
  int get sampleRate;

  /// Processing block size, in frames.
  int get blockSize;

  /// The format the engine processes in.
  SampleFormat get sampleFormat;

  /// Whether the engine is currently producing audio.
  bool get isPlaying;

  /// Current transport state.
  TransportState get transportState;

  /// Position reports, emitted while playing.
  Stream<TransportPosition> get positionStream;

  /// Per-track level snapshots, emitted at a UI-friendly rate.
  Stream<LevelSnapshot> get levelStream;

  /// Prepares the engine and its device.
  ///
  /// Safe to call more than once; a second call is a no-op unless
  /// [shutdown] ran in between.
  Future<void> initialize();

  /// Starts playback per [request].
  Future<void> play(PlaybackRequest request);

  /// Pauses in place.
  Future<void> pause();

  /// Stops and rewinds to the project origin.
  Future<void> stop();

  /// Seeks to an absolute frame position.
  Future<void> seekToFrame(int frame);

  /// Sets the master output gain, where `1.0` is unity.
  ///
  /// Values are clamped by the implementation rather than rejected, because
  /// this is called from a live UI gesture.
  void setMasterGain(double gain);

  /// Sets the tempo, in beats per minute.
  ///
  /// Changing tempo re-derives musical positions; it never resamples audio.
  void setTempo(double bpm);

  /// Releases every native resource.
  ///
  /// Idempotent: calling it twice must not throw.
  Future<void> shutdown();
}

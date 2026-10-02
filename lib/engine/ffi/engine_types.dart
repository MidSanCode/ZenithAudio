/// `#[repr(C)]` struct mirrors for the S1 engine and transport ABI.
///
/// ## The rule
///
/// Each class here mirrors one struct in
/// `native/zenith_core/src/ffi/types.rs` (the `// ── S1 engine & transport ──`
/// section), field for field and in the same order. Dart resolves `Struct`
/// fields by declaration order, so **reordering a field here silently
/// reinterprets native memory** — it will not fail to compile, it will produce
/// wrong numbers.
///
/// Two guards exist so that cannot go unnoticed:
///
/// 1. the Rust side asserts each `size_of` against a hard-coded literal;
/// 2. the FFI smoke test compares `sizeOf<T>()` here against the core's
///    `zenith_sizeof_*` at runtime (ABI §2.3).
///
/// Registered as C-013 in `docs/COORDINATION.md`.
library;

import 'dart:ffi';

/// Driver kind discriminants, mirroring `zenith_driver_kind`.
///
/// Held as raw ints rather than an enum because a newer core may send a kind
/// this build has never seen.
abstract final class ZenithDriverKind {
  /// Platform default.
  static const int auto = 0;

  /// A real device via `cpal`; unsupported on `wasm32`.
  static const int cpal = 1;

  /// Driven by the web `AudioWorklet`.
  static const int worklet = 2;

  /// Offline rendering, no device.
  static const int offline = 3;
}

/// Engine flag bits, mirroring `zenith_engine_flags`.
abstract final class ZenithEngineFlags {
  /// Real-time safety assertions on (debug/test builds only).
  static const int strictRealtime = 0x01;

  /// Allow the web degradation strategy to intervene.
  static const int webDegrade = 0x02;

  /// Offline rendering uses the large-buffer fast path.
  static const int offlineFast = 0x04;
}

/// Transport state codes, mirroring the core's status snapshot.
abstract final class ZenithTransportState {
  /// Stopped and rewound.
  static const int stopped = 0;

  /// Playing forward.
  static const int playing = 1;

  /// Paused in place.
  static const int paused = 2;
}

/// The `ZenithMusicalTime` struct, mirroring `ffi/types.rs`.
///
/// 16 bytes: `i64 + u32 + u32`, with no implicit padding after the 8-byte field.
final class ZenithMusicalTime extends Struct {
  /// Absolute position, in ticks (PPQ = 960).
  @Int64()
  external int ticks;

  /// Pulses per quarter note, for independent validation.
  @Uint32()
  external int ppq;

  /// Explicit padding, held at zero.
  @Uint32()
  external int reserved;
}

/// The `ZenithEngineConfig` struct, mirroring `ffi/types.rs`.
///
/// 32 bytes: eight `u32`s, no pointer or 64-bit field to shift alignment.
final class ZenithEngineConfig extends Struct {
  /// Target sample rate, e.g. 48000.
  @Uint32()
  external int sampleRate;

  /// Frames per block, 64..2048.
  @Uint32()
  external int blockSize;

  /// Preallocated mixer channels.
  @Uint32()
  external int maxChannels;

  /// Preallocated tracks.
  @Uint32()
  external int maxTracks;

  /// Driver kind; see [ZenithDriverKind].
  @Uint32()
  external int driverKind;

  /// Flag bits; see [ZenithEngineFlags].
  @Uint32()
  external int flags;

  /// The major version the caller expects, for a pre-create check.
  @Uint32()
  external int abiMajor;

  /// Explicit padding, held at zero.
  @Uint32()
  external int reserved;
}

/// The `ZenithEngineStatus` struct, mirroring `ffi/types.rs`.
///
/// 56 bytes: `i64 + f64 + f32 + 9 × u32`, laid out largest-first so there is no
/// implicit padding.
final class ZenithEngineStatus extends Struct {
  /// Playhead position, in frames.
  @Int64()
  external int playheadFrames;

  /// Tempo, in beats per minute.
  @Double()
  external double bpm;

  /// Real-time load, `0.0..1.0`.
  @Float()
  external double cpuLoad;

  /// Buffer underruns accumulated.
  @Uint32()
  external int xrunCount;

  /// Voices currently sounding.
  @Uint32()
  external int activeVoices;

  /// Voice pool size.
  @Uint32()
  external int maxVoices;

  /// Web degradation tier: 0, 1 or 2.
  @Uint32()
  external int degradeLevel;

  /// Transport state; see [ZenithTransportState].
  @Uint32()
  external int state;

  /// Driver kind in use; see [ZenithDriverKind].
  @Uint32()
  external int driverKind;

  /// Negotiated sample rate.
  @Uint32()
  external int sampleRate;

  /// Block size.
  @Uint32()
  external int blockSize;

  /// Explicit padding, held at zero.
  @Uint32()
  external int reserved;
}

/// A decoded engine status snapshot.
///
/// The native struct is plain data, so this is a convenience view for UI code
/// rather than a lifetime-bound handle.
final class NativeEngineStatus {
  /// Creates a decoded status.
  const NativeEngineStatus({
    required this.playheadFrames,
    required this.bpm,
    required this.cpuLoad,
    required this.xrunCount,
    required this.activeVoices,
    required this.maxVoices,
    required this.degradeLevel,
    required this.state,
    required this.driverKind,
    required this.sampleRate,
    required this.blockSize,
  });

  /// Decodes one native status.
  factory NativeEngineStatus.fromNative(ZenithEngineStatus ref) =>
      NativeEngineStatus(
        playheadFrames: ref.playheadFrames,
        bpm: ref.bpm,
        cpuLoad: ref.cpuLoad,
        xrunCount: ref.xrunCount,
        activeVoices: ref.activeVoices,
        maxVoices: ref.maxVoices,
        degradeLevel: ref.degradeLevel,
        state: ref.state,
        driverKind: ref.driverKind,
        sampleRate: ref.sampleRate,
        blockSize: ref.blockSize,
      );

  /// Playhead position, in frames.
  final int playheadFrames;

  /// Tempo, in beats per minute.
  final double bpm;

  /// Real-time load, `0.0..1.0`.
  final double cpuLoad;

  /// Buffer underruns accumulated.
  final int xrunCount;

  /// Voices currently sounding.
  final int activeVoices;

  /// Voice pool size.
  final int maxVoices;

  /// Web degradation tier.
  final int degradeLevel;

  /// Transport state; see [ZenithTransportState].
  final int state;

  /// Driver kind in use.
  final int driverKind;

  /// Negotiated sample rate.
  final int sampleRate;

  /// Block size.
  final int blockSize;

  /// Whether the transport is playing.
  bool get isPlaying => state == ZenithTransportState.playing;

  /// Whether the transport is paused.
  bool get isPaused => state == ZenithTransportState.paused;

  /// Whether the transport is stopped.
  bool get isStopped => state == ZenithTransportState.stopped;
}

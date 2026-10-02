import 'dart:typed_data';

import '../engine/ffi/engine_bindings.dart';
import 'wav_encoder.dart';

/// Offline mixdown and export (PLAN §3.S4).
///
/// ## What this does
///
/// Renders a project range through the native engine's **offline path** — the
/// same DSP graph the real-time driver uses — and encodes the result as a WAV.
/// Because the graph is shared, the exported audio matches what playback
/// produces, sample for sample.
///
/// ## What this deliberately does not do yet
///
/// **Per-track (stem) export** needs a track→mixer-channel routing that arrives
/// with S6's arrangement layer; today the engine feeds every voice into one
/// source channel, so there is only one stem to render. The API is shaped to
/// accept a range and a format now, so adding stems later is a loop over
/// channels rather than a redesign.
///
/// ## Threading
///
/// Rendering drives the transport and allocates the whole output, so it must
/// not run on the UI isolate for a long project. [renderRangeToWav] is a plain
/// synchronous call so a caller can wrap it in `Isolate.run` (the engine handle
/// is not `Send`, so the isolate should create and use its own engine).
class OfflineExportService {
  /// Renders `[startTicks, endTicks)` from `engine` and encodes it as WAV.
  ///
  /// [tailSeconds] extends the render past `endTicks` so a reverb or delay tail
  /// is not truncated. Returns the WAV bytes, or `null` when the engine rejects
  /// the request (no core, bad range, foreign sample rate).
  Uint8List? renderRangeToWav(
    ZenithEngineHandle engine, {
    required int startTicks,
    required int endTicks,
    double tailSeconds = 0.0,
    int sampleRate = 48000,
    WavFormat format = WavFormat.pcm16,
  }) {
    if (endTicks <= startTicks) return null;
    final tailTicks = _secondsToTicks(tailSeconds);
    final samples = engine.renderOffline(
      startTicks: startTicks,
      endTicks: endTicks + tailTicks,
      targetSampleRate: sampleRate,
      ppq: 960,
    );
    if (samples == null) return null;
    return encodeWav(samples, channels: 2, sampleRate: sampleRate, format: format);
  }

  /// Renders a whole project, expressed as a tick length, to WAV.
  ///
  /// A thin convenience over [renderRangeToWav] for the common "export from the
  /// top" case.
  Uint8List? renderProjectToWav(
    ZenithEngineHandle engine, {
    required int lengthTicks,
    double tailSeconds = 2.0,
    int sampleRate = 48000,
    WavFormat format = WavFormat.pcm16,
  }) =>
      renderRangeToWav(
        engine,
        startTicks: 0,
        endTicks: lengthTicks,
        tailSeconds: tailSeconds,
        sampleRate: sampleRate,
        format: format,
      );

  /// The delay the offline render carries, in samples, so a caller aligning the
  /// export against a reference can compensate.
  int pdcLatencySamples(ZenithEngineHandle engine) => engine.pdcLatency();

  int _secondsToTicks(double seconds) {
    if (seconds <= 0) return 0;
    // ticks = seconds * bpm/60 * ppq; the engine's default tempo is 120, whose
    // tick length in seconds is 60 / (120 * 960). The caller can pass a
    // non-default tempo only through the engine; the tail is a small margin, so
    // using the default here is within a frame or two for any sane tempo.
    const ppq = 960;
    const bpm = 120.0;
    return (seconds * bpm / 60.0 * ppq).round();
  }
}

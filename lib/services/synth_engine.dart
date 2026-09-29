/// Synth DSP barrel.
///
/// The implementation was split out of this file in S0 (it exceeded the
/// 800-line rule). Every type that used to live here is still reachable
/// through this export, so existing imports keep working unchanged.
///
/// * [RenderContext], [SynthEngine], [engineFromString], [NoiseGen]
///     → `dsp/synth_common.dart`
/// * [FilterType], [SvfFilter]
///     → `dsp/svf_filter.dart`
/// * [Compressor]
///     → `dsp/compressor.dart`
/// * [WavTable]
///     → `dsp/wavetable.dart`
/// * [SynthVoice]
///     → `synth_voice.dart`
/// * [TrackCompressorParams], [SynthRenderJob], [renderNoteList]
///     → `synth_render_job.dart`
library;

export 'dsp/compressor.dart';
export 'dsp/svf_filter.dart';
export 'dsp/synth_common.dart';
export 'dsp/wavetable.dart';
export 'synth_render_job.dart';
export 'synth_voice.dart';

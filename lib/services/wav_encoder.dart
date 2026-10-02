/// Waveform Audio File Format (WAV) encoding (PLAN §3.S4 export).
///
/// ## Why hand-rolled
///
/// WAV is a trivial container: a `RIFF` header, a `fmt ` chunk and a `data`
/// chunk. Pulling in a package for it would add a dependency for ~60 lines, and
/// this code needs full control over the sample format (bit depth and
/// float-vs-PCM), which generic encoders usually hide behind defaults.
///
/// ## Formats
///
/// | [WavFormat] | Bits | Notes |
/// |---|---|---|
/// | [WavFormat.pcm16] | 16 | The universally compatible default. |
/// | [WavFormat.pcm24] | 24 | More headroom, still integer PCM. |
/// | [WavFormat.float32] | 32 | Lossless for the engine's `f32` output. |
///
/// Samples arrive interleaved (the engine's own layout), so no conversion is
/// needed beyond quantisation and byte assembly.
library;

import 'dart:typed_data';

/// The sample format a WAV file is written in.
enum WavFormat {
  /// 16-bit signed integer PCM.
  pcm16,

  /// 24-bit signed integer PCM.
  pcm24,

  /// 32-bit IEEE float.
  float32,
}

/// The number of bits per sample for a [WavFormat].
int wavBitsFor(WavFormat format) {
  switch (format) {
    case WavFormat.pcm16:
      return 16;
    case WavFormat.pcm24:
      return 24;
    case WavFormat.float32:
      return 32;
  }
}

/// WAV format tag written into the `fmt ` chunk.
///
/// `1` is integer PCM, `3` is IEEE float; a reader needs the tag because two
/// files with the same bit depth decode differently.
int _formatTag(WavFormat format) => format == WavFormat.float32 ? 3 : 1;

/// Encodes interleaved `f32` samples as a WAV byte stream.
///
/// [samples] is interleaved (`L, R, L, R, …`); [channels] is the frame width;
/// [sampleRate] is in hertz. Non-finite samples are written as silence rather
/// than allowed to become `NaN`/`Inf` bytes that break a player.
Uint8List encodeWav(
  Float32List samples, {
  required int channels,
  required int sampleRate,
  WavFormat format = WavFormat.pcm16,
}) {
  final bits = wavBitsFor(format);
  final bytesPerSample = bits ~/ 8;
  final blockAlign = channels * bytesPerSample;
  final dataSize = samples.length * bytesPerSample;
  // 44 = 12 (RIFF header) + 24 (fmt chunk with no extension) + 8 (data header).
  final fileSize = 44 + dataSize;

  final out = Uint8List(fileSize);
  final view = ByteData.view(out.buffer);
  var offset = 0;

  void ascii(String s) {
    for (final code in s.codeUnits) {
      out[offset++] = code;
    }
  }

  void u32(int v) {
    view.setUint32(offset, v, Endian.little);
    offset += 4;
  }

  void u16(int v) {
    view.setUint16(offset, v, Endian.little);
    offset += 2;
  }

  // ── RIFF header ──
  ascii('RIFF');
  u32(fileSize - 8);
  ascii('WAVE');

  // ── fmt chunk ──
  ascii('fmt ');
  u32(16); // PCM fmt chunk size
  u16(_formatTag(format));
  u16(channels);
  u32(sampleRate);
  u32(sampleRate * blockAlign); // byte rate
  u16(blockAlign);
  u16(bits);

  // ── data chunk ──
  ascii('data');
  u32(dataSize);

  for (final sample in samples) {
    final value = sample.isFinite ? sample.clamp(-1.0, 1.0) : 0.0;
    switch (format) {
      case WavFormat.pcm16:
        final q = (value * 32767.0).round().clamp(-32768, 32767);
        view.setInt16(offset, q, Endian.little);
        offset += 2;
      case WavFormat.pcm24:
        // Scale to the 24-bit range, then write three little-endian bytes. The
        // value is kept as two's complement so negatives round-trip.
        final q = (value * 8388607.0).round().clamp(-8388608, 8388607);
        final unsigned = q & 0xFFFFFF;
        out[offset++] = unsigned & 0xFF;
        out[offset++] = (unsigned >> 8) & 0xFF;
        out[offset++] = (unsigned >> 16) & 0xFF;
      case WavFormat.float32:
        view.setFloat32(offset, value, Endian.little);
        offset += 4;
    }
  }
  return out;
}

/// Encodes interleaved `f64` samples by narrowing to `f32` first.
///
/// Provided because the existing Dart renderers produce `Float64List`; rather
/// than change them, this narrows at the boundary. Values outside `f32` range
/// would be `±Inf` and are written as silence by [encodeWav].
Uint8List encodeWavFromFloat64(
  Float64List samples, {
  required int channels,
  required int sampleRate,
  WavFormat format = WavFormat.pcm16,
}) {
  final narrowed = Float32List(samples.length);
  for (var i = 0; i < samples.length; i++) {
    narrowed[i] = samples[i];
  }
  return encodeWav(
    narrowed,
    channels: channels,
    sampleRate: sampleRate,
    format: format,
  );
}

/// Decodes the header of a WAV byte stream, for tests and for import.
///
/// Returns `null` when the bytes are not a RIFF/WAVE stream. Only the fields
/// this module writes are read; a full decoder is out of scope.
({int channels, int sampleRate, int bits, int dataBytes})? probeWavHeader(
  Uint8List bytes,
) {
  if (bytes.length < 44) return null;
  final view = ByteData.view(bytes.buffer, bytes.offsetInBytes);
  bool tag(int at, String s) {
    for (var i = 0; i < 4; i++) {
      if (bytes[at + i] != s.codeUnitAt(i)) return false;
    }
    return true;
  }

  if (!tag(0, 'RIFF') || !tag(8, 'WAVE') || !tag(12, 'fmt ')) return null;
  final channels = view.getUint16(22, Endian.little);
  final sampleRate = view.getUint32(24, Endian.little);
  final bits = view.getUint16(34, Endian.little);
  final dataBytes = view.getUint32(40, Endian.little);
  return (
    channels: channels,
    sampleRate: sampleRate,
    bits: bits,
    dataBytes: dataBytes,
  );
}

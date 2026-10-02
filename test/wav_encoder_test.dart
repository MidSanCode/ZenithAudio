import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/services/wav_encoder.dart';

/// S4: WAV encoding, pinned against the actual bytes a reader will see.
void main() {
  group('header', () {
    test('a PCM16 mono file has the documented layout', () {
      final samples = Float32List.fromList([0.0, 0.5, -0.5]);
      final wav = encodeWav(samples, channels: 1, sampleRate: 44100);

      final header = probeWavHeader(wav);
      expect(header, isNotNull);
      expect(header!.channels, 1);
      expect(header.sampleRate, 44100);
      expect(header.bits, 16);
      expect(header.dataBytes, 3 * 2, reason: 'one 16-bit sample per frame');
      expect(wav.length, 44 + 6);
    });

    test('the RIFF sizes are consistent', () {
      final wav = encodeWav(
        Float32List.fromList(List.filled(100, 0.0)),
        channels: 2,
        sampleRate: 48000,
      );
      final view = ByteData.view(wav.buffer);
      final riffSize = view.getUint32(4, Endian.little);
      final dataSize = view.getUint32(40, Endian.little);
      expect(riffSize, wav.length - 8);
      expect(dataSize, 100 * 2);
    });

    test('float32 uses format tag 3 and 32 bits', () {
      final wav = encodeWav(
        Float32List.fromList([0.25]),
        channels: 1,
        sampleRate: 48000,
        format: WavFormat.float32,
      );
      final view = ByteData.view(wav.buffer);
      expect(view.getUint16(20, Endian.little), 3, reason: 'IEEE float tag');
      expect(view.getUint16(34, Endian.little), 32);
      // The sample itself decodes exactly.
      expect(view.getFloat32(44, Endian.little), closeTo(0.25, 1e-7));
    });

    test('pcm24 writes three bytes per sample', () {
      final wav = encodeWav(
        Float32List.fromList([0.0, 1.0]),
        channels: 1,
        sampleRate: 48000,
        format: WavFormat.pcm24,
      );
      expect(probeWavHeader(wav)!.bits, 24);
      expect(wav.length, 44 + 2 * 3);
    });
  });

  group('sample quantisation', () {
    test('full-scale samples round-trip within one LSB', () {
      final wav = encodeWav(
        Float32List.fromList([1.0, -1.0, 0.0]),
        channels: 1,
        sampleRate: 48000,
      );
      final view = ByteData.view(wav.buffer);
      // The scale is symmetric at 32767, so -1.0 maps to -32767, not -32768;
      // that avoids the off-by-one asymmetry a 32768 scale produces.
      expect(view.getInt16(44, Endian.little), 32767);
      expect(view.getInt16(46, Endian.little), -32767);
      expect(view.getInt16(48, Endian.little), 0);
    });

    test('out-of-range samples are clamped, not wrapped', () {
      final wav = encodeWav(
        Float32List.fromList([2.0, -2.0]),
        channels: 1,
        sampleRate: 48000,
      );
      final view = ByteData.view(wav.buffer);
      expect(view.getInt16(44, Endian.little), 32767);
      expect(view.getInt16(46, Endian.little), -32767);
    });

    test('non-finite samples become silence rather than NaN bytes', () {
      final wav = encodeWav(
        Float32List.fromList([double.nan, double.infinity, -double.infinity]),
        channels: 1,
        sampleRate: 48000,
      );
      final view = ByteData.view(wav.buffer);
      expect(view.getInt16(44, Endian.little), 0);
      expect(view.getInt16(46, Endian.little), 0);
      expect(view.getInt16(48, Endian.little), 0);
    });

    test('a negative value survives the 24-bit two\'s-complement encoding', () {
      final wav = encodeWav(
        Float32List.fromList([-1.0]),
        channels: 1,
        sampleRate: 48000,
        format: WavFormat.pcm24,
      );
      // -8388607 in two's complement is 0x800001, little-endian 01 00 80.
      expect(wav[44], 0x01);
      expect(wav[45], 0x00);
      expect(wav[46], 0x80);
    });
  });

  group('float64 convenience', () {
    test('narrows to f32 and encodes', () {
      final wav = encodeWavFromFloat64(
        Float64List.fromList([0.5, -0.5]),
        channels: 2,
        sampleRate: 44100,
      );
      final header = probeWavHeader(wav);
      expect(header!.channels, 2);
      expect(header.dataBytes, 4);
    });
  });

  group('probe', () {
    test('rejects a non-WAV byte stream', () {
      expect(probeWavHeader(Uint8List(100)), isNull);
      expect(probeWavHeader(Uint8List.fromList(List.filled(10, 0))), isNull);
    });
  });
}

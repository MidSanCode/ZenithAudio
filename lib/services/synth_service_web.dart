import 'dart:html' as html;
import 'dart:typed_data';
import '../models/note.dart';
import '../models/instrument.dart';
import '../core/utils/logger.dart';
import 'synth_engine.dart';
import 'soundfont_service.dart';

class SynthService {
  static const _sampleRate = 44100;

  Future<({String path, double duration})> renderToFile({
    required List<Note> notes,
    required String instrumentName,
  }) async {
    if (notes.isEmpty) {
      AppLogger.w('SynthService: no notes to render');
      return (path: '', duration: 0.0);
    }

    final instrument = InstrumentPreset.fromId(instrumentName);
    final totalDuration = _computeDuration(notes);

    final buffer = renderNoteList(SynthRenderJob(
      notes: notes,
      instrument: instrument,
      totalDuration: totalDuration,
      sampleRate: _sampleRate,
      bank: SoundFontService.instance.bank,
    ));

    final wavBytes = _encodeWav(buffer, buffer.length);
    final blob = html.Blob([wavBytes], 'audio/wav');
    final url = html.Url.createObjectUrl(blob);

    AppLogger.d('SynthService: rendered ${notes.length} notes to blob (${totalDuration.toStringAsFixed(2)}s)');
    return (path: url, duration: totalDuration);
  }

  double _computeDuration(List<Note> notes) {
    double end = 0;
    for (final n in notes) {
      final e = n.startTime + n.duration;
      if (e > end) end = e;
    }
    return end + 0.5;
  }

  Float64List renderPreview(InstrumentPreset inst,
      {int pitch = 60, double duration = 1.0, int velocity = 100}) {
    final tail = inst.envCurve != null ? 0.5 : 0.3;
    final job = SynthRenderJob(
      notes: [
        Note(pitch: pitch, startTime: 0, duration: duration, velocity: velocity),
      ],
      instrument: inst,
      totalDuration: duration + tail,
      sampleRate: _sampleRate,
      bank: SoundFontService.instance.bank,
    );
    return renderNoteList(job);
  }

  Uint8List renderPreviewWav(InstrumentPreset inst,
      {int pitch = 60, double duration = 1.0, int velocity = 100}) {
    final buffer = renderPreview(inst, pitch: pitch, duration: duration, velocity: velocity);
    return _encodeWav(buffer, buffer.length);
  }

  Uint8List _encodeWav(Float64List buffer, int numSamples) {
    final bytesPerSample = 2;
    final dataSize = numSamples * bytesPerSample;
    final fileSize = 44 + dataSize;
    final result = DataWriter(fileSize);

    result.writeString('RIFF');
    result.writeInt32(fileSize - 8);
    result.writeString('WAVE');
    result.writeString('fmt ');
    result.writeInt32(16);
    result.writeInt16(1);
    result.writeInt16(1);
    result.writeInt32(_sampleRate);
    result.writeInt32(_sampleRate * bytesPerSample);
    result.writeInt16(bytesPerSample);
    result.writeInt16(16);
    result.writeString('data');
    result.writeInt32(dataSize);

    for (int i = 0; i < numSamples; i++) {
      final clamped = buffer[i].clamp(-1.0, 1.0);
      final sample = (clamped * 32767).round().clamp(-32768, 32767);
      result.writeInt16(sample);
    }

    return result.bytes;
  }
}

class DataWriter {
  final List<int> _data;
  int _offset = 0;

  DataWriter(int size) : _data = List.filled(size, 0);

  Uint8List get bytes => Uint8List.fromList(_data);

  void writeString(String s) {
    for (int i = 0; i < s.length; i++) {
      _data[_offset++] = s.codeUnitAt(i);
    }
  }

  void writeInt32(int value) {
    _data[_offset++] = value & 0xFF;
    _data[_offset++] = (value >> 8) & 0xFF;
    _data[_offset++] = (value >> 16) & 0xFF;
    _data[_offset++] = (value >> 24) & 0xFF;
  }

  void writeInt16(int value) {
    _data[_offset++] = value & 0xFF;
    _data[_offset++] = (value >> 8) & 0xFF;
  }
}

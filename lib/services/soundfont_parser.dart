import 'dart:math' as math;
import 'dart:typed_data';

/// Minimal SoundFont 2 (SF2) reader.
///
/// Supports the subset needed for sample playback:
///  - RIFF/sfbk container, LIST/INFO/SDTA/PDTA chunks
///  - phdr/pbag/pgen (preset zones: instrument link only)
///  - inst/ibag/igen (instrument zones: sampleID, sampleModes,
///    overridingRootKey, coarse/fineTune)
///  - shdr (sample headers), smpl (16-bit PCM)
class SoundFontSample {
  final Float64List data;
  final int sampleRate;
  final double baseFreq; // Hz of the root key
  final int loopMode; // 0 none, 1 forward loop
  final int loopStart;
  final int loopEnd;
  final int minKey;
  final int maxKey;
  final int rootKey;
  final double tuneCents;

  SoundFontSample({
    required this.data,
    required this.sampleRate,
    required this.baseFreq,
    required this.loopMode,
    required this.loopStart,
    required this.loopEnd,
    required this.minKey,
    required this.maxKey,
    required this.rootKey,
    this.tuneCents = 0,
  });
}

class SoundFontPreset {
  final String name;
  final int program;
  final int bank;
  final List<SoundFontSample> samples;

  SoundFontPreset({
    required this.name,
    required this.program,
    required this.bank,
    required this.samples,
  });
}

class SoundFontBank {
  final String name;
  final List<SoundFontPreset> presets;

  SoundFontBank({required this.name, required this.presets});

  /// Find the best sample for a MIDI [pitch] within a preset.
  SoundFontSample? sampleForPitch(SoundFontPreset preset, int pitch) {
    SoundFontSample? best;
    int bestDist = 1 << 30;
    for (final s in preset.samples) {
      if (pitch >= s.minKey && pitch <= s.maxKey) {
        final d = (pitch - s.rootKey).abs();
        if (d < bestDist) {
          bestDist = d;
          best = s;
        }
      }
    }
    best ??= preset.samples.isNotEmpty ? preset.samples.first : null;
    return best;
  }

  SoundFontPreset? findPreset(int program, {int bank = 0}) {
    for (final p in presets) {
      if (p.program == program && p.bank == bank) return p;
    }
    for (final p in presets) {
      if (p.program == program) return p;
    }
    SoundFontPreset? best;
    int bestDist = 1 << 30;
    for (final p in presets) {
      final d = (p.program - program).abs();
      if (d < bestDist) {
        bestDist = d;
        best = p;
      }
    }
    return best;
  }

  /// Load an SF2 file from bytes. Throws FormatException on invalid files.
  static SoundFontBank parse(Uint8List bytes) {
    final r = _Reader(bytes);

    if (r.readString(4) != 'RIFF') {
      throw const FormatException('Not a RIFF file');
    }
    final riffLen = r.readUint32();
    if (r.readString(4) != 'sfbk') {
      throw const FormatException('Not a SoundFont (sfbk) file');
    }
    final end = math.min(bytes.length, 8 + riffLen);

    String name = 'SoundFont';
    final samples16 = <int>[];
    _Phdr? phdr;
    _Pbag? pbag;
    List<_Gen> pgen = [];
    _Inst? inst;
    _Ibag? ibag;
    List<_Gen> igen = [];
    _Shdr? shdr;

    while (r.offset + 8 <= end) {
      final chunkId = r.readString(4);
      final chunkLen = r.readUint32();
      final chunkEnd = r.offset + chunkLen;

      if (chunkId == 'LIST') {
        final type = r.readString(4);
        if (type == 'INFO') {
          while (r.offset + 8 <= chunkEnd) {
            final id = r.readString(4);
            final len = r.readUint32();
            final dataEnd = r.offset + len;
            if (id == 'INAM') {
              name = r.readString(len).replaceAll('\x00', '').trim();
            }
            r.seek(dataEnd);
          }
        } else if (type == 'sdta') {
          while (r.offset + 8 <= chunkEnd) {
            final id = r.readString(4);
            final len = r.readUint32();
            final dataEnd = r.offset + len;
            if (id == 'smpl') {
              final count = len ~/ 2;
              for (int i = 0; i < count; i++) {
                samples16.add(r.readInt16());
              }
            }
            r.seek(dataEnd);
          }
        } else if (type == 'pdta') {
          while (r.offset + 8 <= chunkEnd) {
            final id = r.readString(4);
            final len = r.readUint32();
            final dataEnd = r.offset + len;
            switch (id) {
              case 'phdr': // 38 bytes per record
                final n = len ~/ 38;
                phdr = _Phdr(n);
                for (int i = 0; i < n; i++) {
                  phdr.names[i] = r.readString(20).replaceAll('\x00', '');
                  phdr.programs[i] = r.readUint16();
                  phdr.banks[i] = r.readUint16();
                  phdr.bagIdx[i] = r.readUint16();
                  r.skip(10); // library(4) genre(4) morph(2)
                }
                break;
              case 'pbag': // 4 bytes per record
                final n = len ~/ 4;
                pbag = _Pbag(n);
                for (int i = 0; i < n; i++) {
                  pbag.genIdx[i] = r.readUint16();
                  pbag.modIdx[i] = r.readUint16();
                }
                break;
              case 'pgen': // 4 bytes per record
                final n = len ~/ 4;
                pgen = List.generate(n, (_) => _Gen(r));
                break;
              case 'inst': // 22 bytes per record: name(20) + bagIdx(2)
                final n = len ~/ 22;
                inst = _Inst(n);
                for (int i = 0; i < n; i++) {
                  inst.names[i] = r.readString(20).replaceAll('\x00', '');
                  inst.bagIdx[i] = r.readUint16();
                }
                break;
              case 'ibag': // 4 bytes per record
                final n = len ~/ 4;
                ibag = _Ibag(n);
                for (int i = 0; i < n; i++) {
                  ibag.genIdx[i] = r.readUint16();
                  ibag.modIdx[i] = r.readUint16();
                }
                break;
              case 'igen': // 4 bytes per record
                final n = len ~/ 4;
                igen = List.generate(n, (_) => _Gen(r));
                break;
              case 'shdr': // 46 bytes per record
                final n = len ~/ 46;
                shdr = _Shdr(n);
                for (int i = 0; i < n; i++) {
                  shdr.names[i] = r.readString(20).replaceAll('\x00', '');
                  shdr.start[i] = r.readUint32();
                  shdr.end[i] = r.readUint32();
                  shdr.loopStart[i] = r.readUint32();
                  shdr.loopEnd[i] = r.readUint32();
                  shdr.sampleRate[i] = r.readUint32();
                  shdr.pitch[i] = r.readUint8();
                  shdr.pitchCor[i] = r.readInt8();
                  shdr.type[i] = r.readUint16();
                  r.skip(2); // sampleLink
                }
                break;
              default:
                break;
            }
            r.seek(dataEnd);
          }
        }
      }
      r.seek(chunkEnd);
    }

    if (phdr == null || inst == null || shdr == null) {
      throw const FormatException('SF2 missing required chunks');
    }

    final presets = _buildPresets(
      phdr: phdr,
      pbag: pbag,
      pgen: pgen,
      inst: inst,
      ibag: ibag,
      igen: igen,
      shdr: shdr,
      samples16: samples16,
    );
    return SoundFontBank(name: name, presets: presets);
  }

  static List<SoundFontPreset> _buildPresets({
    required _Phdr phdr,
    required _Pbag? pbag,
    required List<_Gen> pgen,
    required _Inst inst,
    required _Ibag? ibag,
    required List<_Gen> igen,
    required _Shdr shdr,
    required List<int> samples16,
  }) {
    // Group instrument zones per instrument.
    final instZones = <int, List<_Gen>>{};
    if (ibag != null) {
      for (int i = 0; i < inst.bagIdx.length; i++) {
        final start = inst.bagIdx[i];
        final end = i + 1 < inst.bagIdx.length
            ? inst.bagIdx[i + 1]
            : ibag.genIdx.length;
        final zones = <_Gen>[];
        for (int z = start; z < end && z < ibag.genIdx.length; z++) {
          final gs = ibag.genIdx[z];
          final ge = z + 1 < ibag.genIdx.length
              ? ibag.genIdx[z + 1]
              : igen.length;
          for (int g = gs; g < ge && g < igen.length; g++) {
            zones.add(igen[g]);
          }
        }
        instZones[i] = zones;
      }
    }

    final presets = <SoundFontPreset>[];
    final presetCount =
        phdr.programs.isNotEmpty ? phdr.programs.length - 1 : 0;
    for (int p = 0; p < presetCount; p++) {
      final program = phdr.programs[p];
      final bank = phdr.banks[p];
      final bagStart = phdr.bagIdx[p];
      final bagEnd = p + 1 < phdr.bagIdx.length
          ? phdr.bagIdx[p + 1]
          : (pbag?.genIdx.length ?? 0);

      final sampleList = <SoundFontSample>[];
      for (int b = bagStart;
          b < bagEnd && pbag != null && b < pbag.genIdx.length;
          b++) {
        final gs = pbag.genIdx[b];
        final ge =
            b + 1 < pbag.genIdx.length ? pbag.genIdx[b + 1] : pgen.length;
        int? instrumentIdx;
        for (int g = gs; g < ge && g < pgen.length; g++) {
          if (pgen[g].op == 41) instrumentIdx = pgen[g].amount; // instrument
        }
        if (instrumentIdx == null || instrumentIdx >= inst.names.length) {
          continue;
        }

        final zones = instZones[instrumentIdx] ?? const <_Gen>[];
        int? sampleId;
        int minKey = 0, maxKey = 127;
        int rootKey = -1;
        int loopMode = 0;
        int coarseTune = 0, fineTune = 0;
        for (final z in zones) {
          switch (z.op) {
            case 43: // sampleID
              sampleId = z.amount;
              break;
            case 44: // sampleModes: bit0 = loop
              loopMode = z.amount & 1;
              break;
            case 58: // overridingRootKey
              rootKey = z.amount;
              break;
            case 51: // coarseTune
              coarseTune = z.signed;
              break;
            case 52: // fineTune
              fineTune = z.signed;
              break;
            case 45: // keyRange (RangeRec: 2 bytes lo/hi packed in amount)
              minKey = z.amount & 0xFF;
              maxKey = (z.amount >> 8) & 0xFF;
              break;
            case 46: // velRange
              break;
            default:
              break;
          }
        }
        if (sampleId == null || sampleId >= shdr.start.length) continue;

        final s0 = shdr.start[sampleId];
        final s1 = shdr.end[sampleId];
        if (s1 <= s0 || s1 > samples16.length) continue;
        final sr = shdr.sampleRate[sampleId];
        if (sr == 0) continue;
        final midiPitch = shdr.pitch[sampleId];
        final corr = shdr.pitchCor[sampleId];
        final tuneSemis =
            (coarseTune + (fineTune + corr) / 100.0);
        final baseFreq = midiPitch < 128
            ? 440.0 * math.pow(2, (midiPitch - 69) / 12 + tuneSemis / 12)
            : 440.0;

        final n = s1 - s0;
        final data = Float64List(n);
        for (int i = 0; i < n; i++) {
          data[i] = samples16[s0 + i] / 32768.0;
        }
        final ls = shdr.loopStart[sampleId].clamp(s0, s1) - s0;
        final le = shdr.loopEnd[sampleId].clamp(s0, s1) - s0;

        sampleList.add(SoundFontSample(
          data: data,
          sampleRate: sr,
          baseFreq: baseFreq,
          loopMode: loopMode,
          loopStart: ls,
          loopEnd: le,
          minKey: minKey,
          maxKey: maxKey,
          rootKey: rootKey >= 0 && rootKey < 128
              ? rootKey
              : (midiPitch < 128 ? midiPitch : 60),
          tuneCents: (coarseTune * 100 + fineTune).toDouble(),
        ));
      }

      if (sampleList.isNotEmpty) {
        presets.add(SoundFontPreset(
          name: phdr.names[p],
          program: program,
          bank: bank,
          samples: sampleList,
        ));
      }
    }
    return presets;
  }
}

// ── internal chunk structs ──

class _Reader {
  final Uint8List b;
  final ByteData _bd;
  int offset;

  _Reader(this.b)
      : _bd = ByteData.sublistView(b),
        offset = 0;

  String readString(int n) {
    var len = n;
    if (offset + len > b.length) len = b.length - offset;
    final s = String.fromCharCodes(b, offset, offset + len);
    offset += len;
    return s;
  }

  int readUint8() {
    final v = _bd.getUint8(offset);
    offset += 1;
    return v;
  }

  int readInt8() {
    final v = _bd.getInt8(offset);
    offset += 1;
    return v;
  }

  int readInt16() {
    final v = _bd.getInt16(offset, Endian.little);
    offset += 2;
    return v;
  }

  int readUint16() {
    final v = _bd.getUint16(offset, Endian.little);
    offset += 2;
    return v;
  }

  int readUint32() {
    final v = _bd.getUint32(offset, Endian.little);
    offset += 4;
    return v;
  }

  void skip(int n) => offset += n;
  void seek(int o) => offset = o;
}

class _Phdr {
  final List<String> names;
  final List<int> programs;
  final List<int> banks;
  final List<int> bagIdx;
  _Phdr(int n)
      : names = List.filled(n, ''),
        programs = List.filled(n, 0),
        banks = List.filled(n, 0),
        bagIdx = List.filled(n, 0);
}

class _Pbag {
  final List<int> genIdx;
  final List<int> modIdx;
  _Pbag(int n)
      : genIdx = List.filled(n, 0),
        modIdx = List.filled(n, 0);
}

class _Gen {
  final int op;
  final int amount;
  final int signed;
  factory _Gen(_Reader r) {
    final op = r.readUint16();
    final amount = r.readUint16();
    return _Gen._(op, amount);
  }
  const _Gen._(this.op, this.amount)
      : signed = amount >= 0x8000 ? amount - 0x10000 : amount;
}

class _Inst {
  final List<String> names;
  final List<int> bagIdx;
  _Inst(int n)
      : names = List.filled(n, ''),
        bagIdx = List.filled(n, 0);
}

class _Ibag {
  final List<int> genIdx;
  final List<int> modIdx;
  _Ibag(int n)
      : genIdx = List.filled(n, 0),
        modIdx = List.filled(n, 0);
}

class _Shdr {
  final List<String> names;
  final List<int> start;
  final List<int> end;
  final List<int> loopStart;
  final List<int> loopEnd;
  final List<int> sampleRate;
  final List<int> pitch;
  final List<int> pitchCor;
  final List<int> type;
  _Shdr(int n)
      : names = List.filled(n, ''),
        start = List.filled(n, 0),
        end = List.filled(n, 0),
        loopStart = List.filled(n, 0),
        loopEnd = List.filled(n, 0),
        sampleRate = List.filled(n, 0),
        pitch = List.filled(n, 0),
        pitchCor = List.filled(n, 0),
        type = List.filled(n, 0);
}

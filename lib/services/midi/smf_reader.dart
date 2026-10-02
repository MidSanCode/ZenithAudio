import 'dart:typed_data';

import '../../models/musical_time.dart';
import '../../models/note.dart';
import '../../models/pattern.dart';
import 'smf_types.dart';

/// Parses a Standard MIDI File into an [SmfFile].
///
/// The parser is total: any malformed input raises an [SmfFormatException]
/// with an offset rather than a `RangeError` or a silent misread. That matters
/// because the input is a user-supplied file, not a trusted buffer.
class SmfReader {
  /// Parses [bytes] as a Standard MIDI File.
  ///
  /// Throws [SmfFormatException] when the bytes are not a valid SMF.
  static SmfFile parse(Uint8List bytes) {
    final cursor = ByteCursor(bytes);
    final header = _parseHeader(cursor);
    final tracks = <SmfTrack>[];
    for (var i = 0; i < header.trackCount; i++) {
      if (cursor.isAtEnd) {
        // A header may declare more tracks than the file contains; stop rather
        // than fail, since truncated-but-usable files are common.
        break;
      }
      tracks.add(_parseTrack(cursor));
    }
    return SmfFile(header: header, tracks: tracks);
  }

  static SmfHeader _parseHeader(ByteCursor cursor) {
    final chunkType = cursor.readBytes(4);
    if (chunkType[0] != 0x4D || chunkType[1] != 0x54 || chunkType[2] != 0x68 || chunkType[3] != 0x64) {
      throw const SmfFormatException('missing MThd header chunk');
    }
    final length = cursor.readUint32();
    if (length < 6) {
      throw SmfFormatException('MThd chunk too short: $length bytes');
    }
    final format = cursor.readUint16();
    final trackCount = cursor.readUint16();
    final division = cursor.readUint16();
    // The remaining header bytes, if any, are reserved.
    if (length > 6) cursor.skip(length - 6);

    int? ppq;
    double? smpteFps;
    if ((division & 0x8000) == 0) {
      ppq = division == 0 ? 960 : division;
    } else {
      // SMPTE: high byte is a negative frame count, low byte is ticks/frame.
      final negativeFps = (division >> 8) & 0xFF;
      final fps = 256 - negativeFps;
      final ticksPerFrame = division & 0xFF;
      smpteFps = fps.toDouble() == 29.0 ? 29.97 : fps.toDouble();
      // Kept for completeness; import rejects SMPTE (see `toProject`).
      ppq = null;
      if (ticksPerFrame == 0) {
        // A SMF with zero ticks per frame is malformed.
        throw const SmfFormatException('SMPTE division with zero ticks per frame');
      }
    }
    return SmfHeader(
      format: format,
      trackCount: trackCount,
      ppq: ppq,
      smpteFps: smpteFps,
    );
  }

  static SmfTrack _parseTrack(ByteCursor cursor) {
    final chunkType = cursor.readBytes(4);
    if (chunkType[0] != 0x4D || chunkType[1] != 0x54 || chunkType[2] != 0x72 || chunkType[3] != 0x6B) {
      throw SmfFormatException('missing MTrk chunk', offset: cursor.position - 4);
    }
    final length = cursor.readUint32();
    final end = cursor.position + length;
    if (end > cursor.position + cursor.remaining) {
      throw SmfFormatException('MTrk chunk runs past end of file',
          offset: cursor.position);
    }

    final notes = <SmfNote>[];
    final tempos = <SmfTempo>[];
    final timeSignatures = <SmfTimeSignature>[];
    String? name;

    var tick = 0;
    // Currently sounding notes, keyed by (channel << 8) | pitch. A stack per
    // key handles the (unusual but legal) case of the same pitch retriggered
    // before it is released.
    final sounding = <int, List<_Pending>>{};

    // Running status: a data byte pair may omit its status byte.
    int? runningStatus;

    while (cursor.position < end) {
      tick += cursor.readVarLen();

      // Running status: a data byte may stand in for a repeated status byte.
      // Peek rather than read-then-rewind, so a malformed run cannot rewind the
      // cursor past the start of a chunk.
      final peeked = cursor.peekUint8();
      if (peeked == null) break;
      int status;
      if (peeked < 0x80) {
        if (runningStatus == null) {
          throw SmfFormatException('running status with no prior status byte',
              offset: cursor.position);
        }
        status = runningStatus;
      } else {
        status = cursor.readUint8();
        if (status < 0xF0) {
          runningStatus = status;
        } else {
          // System and meta events cancel running status.
          runningStatus = null;
        }
      }

      if (status == 0xFF) {
        // Meta event.
        final type = cursor.readUint8();
        final metaLength = cursor.readVarLen();
        final data = cursor.readBytes(metaLength);
        switch (type) {
          case 0x51: // tempo
            if (metaLength >= 3) {
              final upq = (data[0] << 16) | (data[1] << 8) | data[2];
              if (upq > 0) tempos.add(SmfTempo(tick, upq));
            }
          case 0x58: // time signature
            if (metaLength >= 2) {
              final numerator = data[0];
              final denominator = 1 << data[1];
              timeSignatures.add(SmfTimeSignature(tick, numerator, denominator));
            }
          case 0x03: // track name
            name = _decodeText(data);
          case 0x2F: // end of track
            // Nothing to add; the loop bound already accounts for it.
            break;
          default:
            // Ignore other meta events (markers, lyrics, key signatures).
            break;
        }
        continue;
      }

      if (status == 0xF0 || status == 0xF7) {
        // SysEx: skip the payload.
        final sysexLength = cursor.readVarLen();
        cursor.skip(sysexLength);
        continue;
      }

      // A channel voice message: status byte + 1 or 2 data bytes.
      final eventType = status & 0xF0;
      final channel = status & 0x0F;
      final data1 = cursor.readUint8();
      // Program change and channel pressure have one data byte; the rest two.
      final hasSecond = eventType != 0xC0 && eventType != 0xD0;
      final data2 = hasSecond ? cursor.readUint8() : 0;

      if (eventType == 0x90 && data2 > 0) {
        final key = (channel << 8) | data1;
        (sounding[key] ??= []).add(_Pending(tick, data1, data2, channel));
      } else if (eventType == 0x80 || (eventType == 0x90 && data2 == 0)) {
        final key = (channel << 8) | data1;
        final stack = sounding[key];
        if (stack != null && stack.isNotEmpty) {
          final pending = stack.removeLast();
          final length = tick - pending.startTick;
          notes.add(SmfNote(
            pitch: pending.pitch,
            startTick: pending.startTick,
            lengthTicks: length < 1 ? 1 : length,
            velocity: pending.velocity,
            channel: pending.channel,
          ));
        }
      }
      // Other channel events (CC, pitch bend, aftertouch) do not change the
      // note set this importer consumes, so they are skipped.
    }

    // Any note left sounding at end-of-track gets a one-tick length so it is
    // not lost.
    for (final stack in sounding.values) {
      for (final pending in stack) {
        notes.add(SmfNote(
          pitch: pending.pitch,
          startTick: pending.startTick,
          lengthTicks: 1,
          velocity: pending.velocity,
          channel: pending.channel,
        ));
      }
    }

    notes.sort((a, b) => a.startTick == b.startTick
        ? a.pitch.compareTo(b.pitch)
        : a.startTick.compareTo(b.startTick));

    return SmfTrack(
      notes: notes,
      tempos: tempos,
      timeSignatures: timeSignatures,
      name: name,
    );
  }

  static String _decodeText(Uint8List data) {
    // SMF text is nominally ASCII; decode as UTF-8 with a Latin-1 fallback so a
    // stray high byte does not throw.
    try {
      return String.fromCharCodes(data);
    } catch (_) {
      return '';
    }
  }
}

/// A note-on awaiting its note-off.
class _Pending {
  final int startTick;
  final int pitch;
  final int velocity;
  final int channel;

  const _Pending(this.startTick, this.pitch, this.velocity, this.channel);
}

/// Converts a parsed [SmfFile] into the project's tick-based model.
///
/// One MIDI track becomes one [Pattern]; the project assigns them to tracks.
/// Ticks are rescaled from the file's PPQ to the project's [Ticks.ppq] so the
/// music lands on the same grid as the editor.
class SmfProjectData {
  /// One pattern per non-empty MIDI track, in file order.
  final List<Pattern> patterns;

  /// The initial tempo read from the file.
  final double bpm;

  /// The time signature read from the file, as `(numerator, denominator)`.
  final (int, int)? timeSignature;

  const SmfProjectData({
    required this.patterns,
    required this.bpm,
    this.timeSignature,
  });

  /// Whether the file yielded no playable notes.
  bool get isEmpty => patterns.isEmpty;
}

/// Converts [file] to project patterns at the project's PPQ.
///
/// SMPTE-timed files are rejected with an [SmfFormatException]: their ticks are
/// frame-based, and silently treating them as PPQ would place every note at the
/// wrong time.
SmfProjectData smfToPatterns(SmfFile file, {String Function(int index)? nameFor}) {
  if (!file.header.isMetrical) {
    throw const SmfFormatException(
      'SMPTE-timed MIDI files are not supported; export the file with a '
      'metrical (PPQ) division',
    );
  }
  final sourcePpq = file.effectivePpq;
  final scale = Ticks.ppq / sourcePpq;

  final patterns = <Pattern>[];
  for (var i = 0; i < file.tracks.length; i++) {
    final track = file.tracks[i];
    if (track.notes.isEmpty && track.name == null) {
      // An empty metadata-only track (common in format 1) is not a pattern.
      continue;
    }
    final notes = track.notes
        .map((n) => Note(
              pitch: n.pitch,
              startTicks: (n.startTick * scale).round(),
              lengthTicks: (n.lengthTicks * scale).round().clamp(1, 1 << 30),
              velocity: n.velocity,
            ))
        .toList();
    if (notes.isEmpty) continue;

    // The pattern loops to the next bar past its content, so a playlist block
    // built from it repeats cleanly.
    final contentEnd = notes
        .map((n) => n.startTicks + n.lengthTicks)
        .reduce((a, b) => a > b ? a : b);
    final bar = Ticks.barTicks(4);
    final lengthTicks = ((contentEnd + bar - 1) ~/ bar) * bar;

    patterns.add(Pattern(
      id: 'midi_${i}_${DateTime.now().microsecondsSinceEpoch}',
      name: track.name ?? (nameFor?.call(i) ?? 'MIDI ${i + 1}'),
      notes: notes,
      lengthTicks: lengthTicks <= 0 ? Ticks.ppq * 4 : lengthTicks,
    ));
  }

  (int, int)? timeSignature;
  for (final track in file.tracks) {
    if (track.timeSignatures.isNotEmpty) {
      final ts = track.timeSignatures.first;
      timeSignature = (ts.numerator, ts.denominator);
      break;
    }
  }

  return SmfProjectData(
    patterns: patterns,
    bpm: file.initialBpm,
    timeSignature: timeSignature,
  );
}

import 'dart:typed_data';

/// Shared value types for the SMF reader and writer.
///
/// These are plain, immutable records of what a Standard MIDI File contains,
/// independent of the project model. The reader produces them; the project
/// conversion happens in a separate step so the parser can be tested against
/// raw bytes without a `Pattern` in sight.

/// The SMF header fields.
class SmfHeader {
  /// Format 0 (one track), 1 (simultaneous tracks) or 2 (independent).
  final int format;

  /// Number of track chunks declared in the header.
  final int trackCount;

  /// Ticks per quarter note, when the division is metrical.
  ///
  /// `null` when the division is SMPTE, in which case [smpteFps] carries the
  /// frame rate. The two are mutually exclusive.
  final int? ppq;

  /// SMPTE frames per second, when the division is timecode-based.
  final double? smpteFps;

  const SmfHeader({
    required this.format,
    required this.trackCount,
    this.ppq,
    this.smpteFps,
  });

  /// Whether the division is metrical (PPQ) rather than SMPTE.
  bool get isMetrical => ppq != null;

  @override
  String toString() => 'SmfHeader(format: $format, tracks: $trackCount, '
      'ppq: $ppq, smpteFps: $smpteFps)';
}

/// A tempo change read from a `FF 51` meta event.
class SmfTempo {
  /// Absolute tick position.
  final int tick;

  /// Microseconds per quarter note; 500000 is 120 BPM.
  final int microsecondsPerQuarter;

  const SmfTempo(this.tick, this.microsecondsPerQuarter);

  /// The tempo in beats per minute.
  double get bpm => 60000000 / microsecondsPerQuarter;
}

/// A time-signature change read from a `FF 58` meta event.
class SmfTimeSignature {
  /// Absolute tick position.
  final int tick;

  /// Beats per bar.
  final int numerator;

  /// Note value that gets the beat (denominator, e.g. 4 for a quarter).
  final int denominator;

  const SmfTimeSignature(this.tick, this.numerator, this.denominator);
}

/// A note event, already paired into an on/off span.
///
/// The reader emits these rather than raw note-on/off pairs so callers do not
/// each reimplement the pairing (and get velocity-0 note-offs wrong).
class SmfNote {
  /// MIDI pitch, 0..127.
  final int pitch;

  /// Start position, in ticks from the start of the track.
  final int startTick;

  /// Length in ticks. Always at least 1, so a zero-length note is still visible.
  final int lengthTicks;

  /// Note-on velocity, 1..127.
  final int velocity;

  /// The MIDI channel the note was on, 0..15.
  final int channel;

  const SmfNote({
    required this.pitch,
    required this.startTick,
    required this.lengthTicks,
    required this.velocity,
    required this.channel,
  });

  /// End position, in ticks.
  int get endTick => startTick + lengthTicks;

  @override
  String toString() => 'SmfNote(pitch: $pitch, start: $startTick, '
      'len: $lengthTicks, vel: $velocity, ch: $channel)';
}

/// One parsed MIDI track: its notes and the tempo/time-signature events it
/// carried. Tempo events are usually only in the first track (format 1).
class SmfTrack {
  /// Notes, sorted by start tick.
  final List<SmfNote> notes;

  /// Tempo changes in this track.
  final List<SmfTempo> tempos;

  /// Time-signature changes in this track.
  final List<SmfTimeSignature> timeSignatures;

  /// The track's name from a `FF 03` meta event, if present.
  final String? name;

  const SmfTrack({
    this.notes = const [],
    this.tempos = const [],
    this.timeSignatures = const [],
    this.name,
  });

  /// The last note end, or 0 when empty.
  int get contentEndTick =>
      notes.isEmpty ? 0 : notes.map((n) => n.endTick).reduce((a, b) => a > b ? a : b);
}

/// A whole parsed MIDI file.
class SmfFile {
  /// Header fields.
  final SmfHeader header;

  /// The tracks, in file order.
  final List<SmfTrack> tracks;

  const SmfFile({required this.header, required this.tracks});

  /// The tempo at the start of the file, defaulting to 120 BPM when absent.
  double get initialBpm {
    for (final track in tracks) {
      if (track.tempos.isNotEmpty) return track.tempos.first.bpm;
    }
    return 120.0;
  }

  /// The PPQ to interpret note ticks with, defaulting to 960 when the file is
  /// SMPTE-timed (import refuses SMPTE, so this is a defensive default).
  int get effectivePpq => header.ppq ?? 960;
}

/// Raised when a byte stream is not a valid Standard MIDI File.
class SmfFormatException implements Exception {
  /// Human-readable reason.
  final String message;

  /// Byte offset the problem was detected at, when known.
  final int? offset;

  const SmfFormatException(this.message, {this.offset});

  @override
  String toString() =>
      'SmfFormatException: $message${offset == null ? '' : ' (at byte $offset)'}';
}

/// A cursor over a byte buffer with bounds-checked reads.
///
/// Kept internal to the MIDI services: reading a malformed file must not throw
/// `RangeError` from a hundred call sites, so every primitive checks its bound
/// and raises an [SmfFormatException] with the offending offset.
class ByteCursor {
  final Uint8List _bytes;
  int _position = 0;

  ByteCursor(this._bytes);

  /// The current byte offset.
  int get position => _position;

  /// Whether the cursor has reached the end.
  bool get isAtEnd => _position >= _bytes.length;

  /// The number of bytes left.
  int get remaining => _bytes.length - _position;

  /// Reads one byte, or throws when at the end.
  int readUint8() {
    if (_position >= _bytes.length) {
      throw SmfFormatException('unexpected end of data', offset: _position);
    }
    return _bytes[_position++];
  }

  /// Reads a big-endian 16-bit value.
  int readUint16() {
    final hi = readUint8();
    final lo = readUint8();
    return (hi << 8) | lo;
  }

  /// Reads a big-endian 24-bit value.
  int readUint24() {
    final a = readUint8();
    final b = readUint8();
    final c = readUint8();
    return (a << 16) | (b << 8) | c;
  }

  /// Reads a big-endian 32-bit value.
  int readUint32() {
    final a = readUint8();
    final b = readUint8();
    final c = readUint8();
    final d = readUint8();
    return (a << 24) | (b << 16) | (c << 8) | d;
  }

  /// Reads a variable-length quantity (SMF's delta-time encoding).
  ///
  /// Up to four bytes, as allowed by the specification. A longer run is a
  /// malformed file and is rejected rather than silently truncated.
  int readVarLen() {
    var value = 0;
    for (var i = 0; i < 4; i++) {
      final byte = readUint8();
      value = (value << 7) | (byte & 0x7F);
      if ((byte & 0x80) == 0) return value;
    }
    throw SmfFormatException('variable-length quantity exceeds 4 bytes',
        offset: _position);
  }

  /// Reads exactly `length` bytes.
  Uint8List readBytes(int length) {
    if (length < 0 || _position + length > _bytes.length) {
      throw SmfFormatException('truncated read of $length bytes',
          offset: _position);
    }
    final slice = Uint8List.sublistView(_bytes, _position, _position + length);
    _position += length;
    return slice;
  }

  /// Skips `length` bytes.
  void skip(int length) {
    if (length < 0 || _position + length > _bytes.length) {
      throw SmfFormatException('truncated skip of $length bytes',
          offset: _position);
    }
    _position += length;
  }

  /// Peeks at the next byte without consuming it, or `null` at the end.
  int? peekUint8() => _position < _bytes.length ? _bytes[_position] : null;
}

/// Encodes `value` as a variable-length quantity.
///
/// Used by the writer; exposed here so reader/writer tests can share a single
/// definition of the encoding.
List<int> encodeVarLen(int value) {
  if (value < 0) {
    throw ArgumentError.value(value, 'value', 'a delta time cannot be negative');
  }
  // A single byte covers 0..0x7F; longer values collect seven bits at a time.
  final buffer = <int>[value & 0x7F];
  var remaining = value >> 7;
  while (remaining > 0) {
    buffer.add((remaining & 0x7F) | 0x80);
    remaining >>= 7;
  }
  return buffer.reversed.toList();
}

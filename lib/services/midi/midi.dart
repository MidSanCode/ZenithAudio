/// Standard MIDI File (SMF) reading and writing.
///
/// ## Scope
///
/// This is a pure Dart implementation of the SMF 0 (single track) and SMF 1
/// (multiple simultaneous tracks) formats. It converts between an SMF byte
/// stream and the project's tick-based [`Pattern`] model, so a MIDI file can be
/// imported (one pattern per MIDI track) and exported (one MIDI track per
/// pattern).
///
/// It is deliberately free of Flutter and of the audio engine: it depends only
/// on `models/`, so it can be unit-tested without a device and reused by a
/// future Rust importer.
///
/// ## What is handled
///
/// * MThd header (format 0/1, division in PPQ ticks per quarter note)
/// * MTrk chunks, delta-time encoded (variable-length quantities)
/// * Tempo and time-signature meta events
/// * Note-on / note-off, including note-on with velocity 0 (a note-off)
/// * Running status
/// * SMPTE division is rejected, not silently misread
///
/// ## What is intentionally not handled
///
/// The plan's S6c requires SMF 0/1 import and export; it does not require
/// playback of every controller. Non-note channel events are preserved on a
/// re-export only as they affect notes (they are otherwise skipped on import),
/// and format 2 (independent sequences) is unsupported because no DAW-style
/// arrangement maps cleanly onto it.
library;

export 'smf_reader.dart';
export 'smf_writer.dart';
export 'smf_types.dart';

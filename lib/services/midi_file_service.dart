import 'dart:typed_data';

import 'package:file_picker/file_picker.dart';

/// A MIDI file chosen by the user, with its bytes already read.
///
/// The bytes are read here rather than exposing a path because the import is
/// pure byte parsing, and reading the file at the call site would make the
/// importer depend on `dart:io` (breaking a web build).
class PickedMidiFile {
  /// The file's base name, used to label imported patterns.
  final String name;

  /// The file's bytes.
  final Uint8List bytes;

  const PickedMidiFile({required this.name, required this.bytes});
}

/// Picks `.mid` / `.midi` files and hands back their bytes.
class MidiFileService {
  /// Opens a file picker filtered to MIDI files, returning `null` on cancel.
  Future<PickedMidiFile?> pickMidiFile() async {
    final result = await FilePicker.platform.pickFiles(
      type: FileType.custom,
      allowedExtensions: ['mid', 'midi'],
      // The importer needs the bytes, and `withData` is the only cross-platform
      // way to get them (web has no path).
      withData: true,
    );
    if (result == null || result.files.isEmpty) return null;
    final file = result.files.single;
    final bytes = file.bytes;
    if (bytes == null) return null;
    return PickedMidiFile(name: file.name, bytes: bytes);
  }

  /// Saves `bytes` as a MIDI file, returning the chosen path or `null` on
  /// cancel.
  Future<String?> saveMidiFile(Uint8List bytes, {String? suggestedName}) async {
    return FilePicker.platform.saveFile(
      dialogTitle: 'Export MIDI',
      fileName: suggestedName ?? 'export.mid',
      type: FileType.custom,
      allowedExtensions: ['mid'],
      bytes: bytes,
    );
  }
}

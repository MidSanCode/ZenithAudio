import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:archive/archive.dart';
import 'package:path_provider/path_provider.dart';
import '../core/constants/app_constants.dart';
import '../models/project.dart';
import '../models/track.dart';
import '../models/note.dart';
import '../core/utils/logger.dart';
import 'synth_engine.dart' show TrackCompressorParams;

/// Result returned after deserializing a project archive.
class SerializedProject {
  final Project project;
  final Map<String, String> trackAudioFiles;

  const SerializedProject({
    required this.project,
    required this.trackAudioFiles,
  });
}

/// Handles saving/loading project files in .zap format on desktop platforms.
///
/// .zap file internal structure:
///   info.json                  — project descriptor (JSON)
///   assets/                    — audio files
///   instruments/ (reserved)    — instrument definitions
///   effects/     (reserved)    — effect presets
class ProjectSerializer {
  /// Serialize a [Project] into .zap archive bytes.
  ///
  /// [audioFileBytes] maps trackId → raw audio bytes for each track that has an audio file.
  /// Provide empty map when reading from disk is handled inside this method,
  /// or pass pre-loaded bytes when working with in-memory data.
  Future<Uint8List> serialize(
    Project project, {
    Map<String, Uint8List> audioFileBytes = const {},
  }) async {
    final archive = Archive();

    // --- info.json ---
    final info = _buildInfoJson(project);
    final infoBytes = utf8.encode(jsonEncode(info));
    archive.addFile(ArchiveFile('info.json', infoBytes.length, infoBytes));

    // --- assets/ (audio files) ---
    for (final track in project.tracks) {
      if (track.audioFilePath == null) continue;
      Uint8List bytes;
      if (audioFileBytes.containsKey(track.id)) {
        bytes = audioFileBytes[track.id]!;
      } else {
        final file = File(track.audioFilePath!);
        if (!await file.exists()) {
          AppLogger.w('Audio file not found for track ${track.id}: ${track.audioFilePath}');
          continue;
        }
        bytes = await file.readAsBytes();
      }
      final ext = _extensionFromPath(track.audioFilePath!) ?? '.wav';
      final assetName = 'audio_${track.id}$ext';
      final assetPath = '${AppConstants.projectAssetsDir}$assetName';
      archive.addFile(ArchiveFile(assetPath, bytes.length, bytes));
    }

    // Placeholder directory markers for future reserved directories.
    archive.addFile(ArchiveFile('${AppConstants.projectInstrumentsDir}.gitkeep', 0, Uint8List(0)));
    archive.addFile(ArchiveFile('${AppConstants.projectEffectsDir}.gitkeep', 0, Uint8List(0)));

    final encoded = ZipEncoder().encode(archive);
    return Uint8List.fromList(encoded);
  }

  /// Trigger a browser file download (stub for desktop; actual impl in web variant).
  void downloadArchive(Uint8List bytes, String filename) {
    // Desktop platforms use FilePicker.saveFile() instead.
    throw UnsupportedError('downloadArchive is only supported on web');
  }

  /// Deserialize .zap archive bytes into a [SerializedProject].
  ///
  /// Extracts audio files to a temporary directory so the AudioService
  /// can reference them via local file paths.
  Future<SerializedProject?> deserialize(Uint8List bytes) async {
    try {
      final archive = ZipDecoder().decodeBytes(bytes.toList());

      // --- Parse info.json ---
      final infoFile = archive.files.firstWhere(
        (f) => f.name == 'info.json',
        orElse: () => throw Exception('Missing info.json in project file'),
      );
      final infoStr = utf8.decode(infoFile.content.toList());
      final info = jsonDecode(infoStr) as Map<String, dynamic>;

      final project = _parseInfoJson(info);
      final trackAudioFiles = <String, String>{};

      // --- Extract all files to a temporary directory ---
      final tempDir = await getTemporaryDirectory();
      final extractDir = Directory('${tempDir.path}/zap_extract_${project.id}');
      if (await extractDir.exists()) {
        await extractDir.delete(recursive: true);
      }
      await extractDir.create(recursive: true);

      for (final file in archive.files) {
        if (file.isFile) {
          final dest = File('${extractDir.path}/${file.name}');
          await dest.create(recursive: true);
          await dest.writeAsBytes(file.content.toList());
        }
      }

      // Map each track's audio file reference to the extracted path.
      for (final track in project.tracks) {
        if (track.audioFilePath == null) continue;
        // Use forward slash separator; dart:io File normalizes on all platforms.
        final fullPath = '${extractDir.path}/${track.audioFilePath}';
        if (await File(fullPath).exists()) {
          trackAudioFiles[track.id] = fullPath;
        } else {
          AppLogger.w('Extracted audio not found: $fullPath');
        }
      }

      return SerializedProject(
        project: project,
        trackAudioFiles: trackAudioFiles,
      );
    } catch (e) {
      AppLogger.e('Failed to deserialize project', e);
      return null;
    }
  }

  // --- JSON helpers ---

  Map<String, dynamic> _buildInfoJson(Project project) {
    final tracksJson = project.tracks.map((t) {
      String? audioFile;
      if (t.type == TrackType.audio && t.audioFilePath != null) {
        final ext = _extensionFromPath(t.audioFilePath!) ?? '.wav';
        audioFile = '${AppConstants.projectAssetsDir}audio_${t.id}$ext';
      }
      return {
        'id': t.id,
        'name': t.name,
        'type': t.type.name,
        'volume': t.volume,
        'isMuted': t.isMuted,
        'isSolo': t.isSolo,
        if (t.type == TrackType.audio) 'audioFile': audioFile,
        if (t.isInstrument) 'instrumentName': t.instrumentName,
        if (t.isInstrument && t.notes.isNotEmpty)
          'notes': t.notes.map((n) => n.toJson()).toList(),
        if (t.compressor != null) 'compressor': t.compressor!.toJson(),
        'color': '#${t.color.toARGB32().toRadixString(16).padLeft(8, '0')}',
        'duration': t.duration,
      };
    }).toList();

    return {
      'version': AppConstants.projectFormatVersion,
      'name': project.name,
      'sampleRate': project.sampleRate,
      'timeSignatureNumerator': project.timeSignatureNumerator,
      'timeSignatureDenominator': project.timeSignatureDenominator,
      'keySignature': project.keySignature,
      'bpm': project.bpm,
      'playbackSpeed': project.playbackSpeed,
      'tracks': tracksJson,
    };
  }

  Project _parseInfoJson(Map<String, dynamic> info) {
    final version = info['version'] as int? ?? 1;
    // Future: handle migration for older versions.
    if (version > AppConstants.projectFormatVersion) {
      throw Exception('Project requires a newer version of the app');
    }

    final name = info['name'] as String? ?? 'Untitled';
    final sampleRate = (info['sampleRate'] as num?)?.toDouble() ?? 44100;
    final tracksJson = info['tracks'] as List<dynamic>? ?? [];

    final tracks = tracksJson.map((j) {
      final t = j as Map<String, dynamic>;
      final typeStr = t['type'] as String? ?? 'audio';
      final type = TrackType.values.firstWhere(
        (e) => e.name == typeStr,
        orElse: () => TrackType.audio,
      );
      return Track(
        id: t['id'] as String? ?? '',
        name: t['name'] as String? ?? 'Track',
        type: type,
        instrumentName: t['instrumentName'] as String?,
        notes: (type == TrackType.instrument || type == TrackType.synth)
            ? ((t['notes'] as List<dynamic>?)?.map((n) =>
                Note.fromJson(n as Map<String, dynamic>)).toList() ?? [])
            : const [],
        volume: (t['volume'] as num?)?.toDouble() ?? 0.8,
        isMuted: t['isMuted'] as bool? ?? false,
        isSolo: t['isSolo'] as bool? ?? false,
        audioFilePath: t['audioFile'] as String?,
        color: _parseColor(t['color'] as String?),
        duration: (t['duration'] as num?)?.toDouble() ?? 0,
        compressor: t['compressor'] != null
            ? TrackCompressorParams.fromJson(
                t['compressor'] as Map<String, dynamic>)
            : null,
      );
    }).toList();

    return Project(
      id: '',
      name: name,
      tracks: tracks,
      sampleRate: sampleRate,
      timeSignatureNumerator: info['timeSignatureNumerator'] as int? ?? 4,
      timeSignatureDenominator: info['timeSignatureDenominator'] as int? ?? 4,
      keySignature: info['keySignature'] as String? ?? 'C',
      bpm: (info['bpm'] as num?)?.toDouble() ?? 120,
      playbackSpeed: (info['playbackSpeed'] as num?)?.toDouble() ?? 1.0,
    );
  }

  Color _parseColor(String? hex) {
    if (hex == null) return const Color(0xFF40C4FF);
    try {
      final val = int.parse(hex.replaceFirst('#', ''), radix: 16);
      return Color(val);
    } catch (_) {
      return const Color(0xFF40C4FF);
    }
  }

  String? _extensionFromPath(String path) {
    final dot = path.lastIndexOf('.');
    return dot >= 0 ? path.substring(dot).toLowerCase() : null;
  }
}

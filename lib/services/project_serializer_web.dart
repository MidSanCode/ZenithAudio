import 'dart:convert';
import 'dart:html' as html;
import 'dart:typed_data';

import 'package:archive/archive.dart';

import '../core/utils/logger.dart';
import '../models/project.dart';
import 'lgdf_format.dart';
import 'lgdf_project_codec.dart';
import 'project_serializer_models.dart';

export 'project_serializer_models.dart' show SerializedProject, LgdfProjectInfo;

/// LGDF v2.0 project serialization on web.
///
/// The browser has no writable project directory, so a project always exists
/// as an in-memory LGDF archive: `assets/audios/*` carry the audio bytes and
/// `spec/project.json` carries the DAW document. Reading an archive materializes
/// its resources as blob/object URLs for the audio pipeline.
class ProjectSerializer {
  const ProjectSerializer();

  static const _codec = LgdfProjectCodec();

  /// Builds LGDF archive bytes for [project].
  ///
  /// [audioFileBytes] maps trackId → raw audio bytes (blob URLs cannot be read
  /// back on web, so callers must supply the bytes).
  Future<Uint8List> serialize(
    Project project, {
    Map<String, Uint8List> audioFileBytes = const {},
  }) async {
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final slug = Lgdf.slugify(
      project.name,
      fallback: 'project-${Lgdf.shortId(project.id)}',
    );
    final archive = Archive();

    void addJson(String path, Object? value) {
      final bytes = Lgdf.encodeJsonBytes(value);
      archive.addFile(ArchiveFile(path, bytes.length, bytes));
    }

    // ── Resources ──
    final registered = <String>[];
    final trackAssetPaths = <String, String>{};

    for (final track in project.tracks) {
      final bytes = audioFileBytes[track.id];
      if (bytes == null) {
        if (track.audioFilePath != null) {
          AppLogger.w('Missing audio bytes for track ${track.id} on web');
        }
        continue;
      }
      final ext = Lgdf.extensionOf(track.audioFilePath ?? '.wav');
      final assetName = 'audio_${track.id}$ext';
      final relPath = '${Lgdf.audioAssetsDir}/$assetName';

      archive.addFile(ArchiveFile(relPath, bytes.length, bytes));
      addJson(
        '${Lgdf.audioMetadataDir}/$assetName.json',
        Lgdf.buildAssetMetadata(
          path: relPath,
          size: bytes.length,
          sha256Hex: Lgdf.sha256Hex(bytes),
          createdTime: now,
          lastUpdateTime: now,
          extra: {'track_id': track.id, 'track_name': track.name},
        ),
      );

      registered.add(relPath);
      trackAssetPaths[track.id] = relPath;
    }

    // ── Description layer ──
    addJson(
      Lgdf.projectSpecFile,
      _codec.buildProjectDocument(project, trackAssetPaths),
    );
    addJson(Lgdf.configSpecFile, _codec.buildSpecConfig(project, slug));
    addJson(Lgdf.schemaSpecFile, _codec.buildSpecConfigSchema());

    final overview = utf8.encode(_codec.buildOverview(project, trackAssetPaths));
    archive.addFile(ArchiveFile(Lgdf.overviewSpecFile, overview.length, overview));

    // ── Registry + info ──
    addJson(Lgdf.registryFile, Lgdf.buildRegistry(registered));
    addJson(
      Lgdf.infoFile,
      _codec.buildInfo(project: project, createdTime: now, lastUpdateTime: now),
    );

    final encoded = ZipEncoder().encode(archive);
    return Uint8List.fromList(encoded);
  }

  /// Triggers a browser download of [bytes] as [filename].
  void downloadArchive(Uint8List bytes, String filename) {
    final blob = html.Blob([bytes], 'application/zip');
    final url = html.Url.createObjectUrlFromBlob(blob);
    final anchor = html.AnchorElement(href: url)
      ..download = filename
      ..style.display = 'none';
    html.document.body?.children.add(anchor);
    anchor.click();
    anchor.remove();
    html.Url.revokeObjectUrl(url);
  }

  /// Reads an LGDF archive (or a legacy `.zap`) into a [SerializedProject].
  Future<SerializedProject?> deserialize(Uint8List bytes) async {
    try {
      final archive = ZipDecoder().decodeBytes(bytes.toList());

      final infoFile = archive.files.firstWhere(
        (f) => Lgdf.normalizePath(f.name) == Lgdf.infoFile,
        orElse: () => throw Exception('Missing info.json in project archive'),
      );
      final info =
          jsonDecode(utf8.decode(infoFile.content.toList())) as Map<String, dynamic>;

      final Project project;
      LgdfProjectInfo? lgdfInfo;

      if (info['format'] == Lgdf.format) {
        final docFile = archive.files.firstWhere(
          (f) => Lgdf.normalizePath(f.name) == Lgdf.projectSpecFile,
          orElse: () =>
              throw Exception('Missing ${Lgdf.projectSpecFile} in LGDF archive'),
        );
        project = _codec.parseProjectDocument(
          jsonDecode(utf8.decode(docFile.content.toList())) as Map<String, dynamic>,
        );
        lgdfInfo = LgdfProjectInfo(
          name: info['name'] as String? ?? Lgdf.slugify(project.name),
          displayName: info['display_name'] as String?,
          createdTime: (info['created_time'] as num?)?.toInt() ??
              DateTime.now().millisecondsSinceEpoch ~/ 1000,
          version: (info['version'] as num?)?.toInt() ?? 1,
        );
      } else {
        project = _codec.parseLegacyInfo(info);
        AppLogger.i('Legacy .zap project migrated on read: ${project.name}');
      }

      // Materialize resources as object URLs the audio engine can play.
      final trackAudioFiles = <String, String>{};
      final urls = <String, String>{};
      for (final file in archive.files) {
        if (!file.isFile) continue;
        final rel = Lgdf.normalizePath(file.name);
        if (!rel.startsWith('${Lgdf.audioAssetsDir}/')) continue;
        final data = Uint8List.fromList(file.content.toList());
        final blob = html.Blob([data], 'audio/*');
        urls[rel] = html.Url.createObjectUrlFromBlob(blob);
      }
      for (final track in project.tracks) {
        final rel = track.audioFilePath;
        if (rel == null) continue;
        final url = urls[Lgdf.normalizePath(rel)];
        if (url != null) trackAudioFiles[track.id] = url;
      }

      return SerializedProject(
        project: project,
        trackAudioFiles: trackAudioFiles,
        lgdfInfo: lgdfInfo,
      );
    } catch (e) {
      AppLogger.e('Failed to deserialize project', e);
      return null;
    }
  }
}

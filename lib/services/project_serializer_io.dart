import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:archive/archive.dart';
import 'package:path_provider/path_provider.dart';

import '../core/utils/logger.dart';
import '../models/project.dart';
import 'lgdf_format.dart';
import 'lgdf_project_codec.dart';
import 'project_serializer_models.dart';

export 'project_serializer_models.dart' show SerializedProject, LgdfProjectInfo;

/// Writes and reads LGDF v2.0 directory-mode projects on desktop.
///
/// ```
/// my-song/
/// ├── info.json             核心配置
/// ├── registry.json         注册资源清单
/// ├── assets/audios/        音频资源
/// ├── metadata/audios/      资源元数据（一一镜像）
/// ├── spec/                 描述层（project.json / config.json / overview.md）
/// ├── work/                 临时文件（永不导出）
/// └── dist/                 导出的 .lgdf 包
/// ```
class ProjectSerializer {
  const ProjectSerializer();

  static const _codec = LgdfProjectCodec();

  // ────────────────────────────────────────────────────────────
  // Directory mode
  // ────────────────────────────────────────────────────────────

  /// Writes a directory-mode LGDF project at [projectDir].
  ///
  /// Audio files referenced by tracks are copied into `assets/audios/` and
  /// registered with mirrored metadata. Returns the project document size.
  Future<int> writeProjectDirectory(
    Project project,
    Directory projectDir, {
    LgdfProjectInfo? existingInfo,
  }) async {
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final created = existingInfo?.createdTime ?? now;

    await _ensureDir(projectDir);
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.audioAssetsDir}'));
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.audioMetadataDir}'));
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.specDir}'));
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.workDir}'));
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.distDir}'));

    // ── 1. Resources ──
    final registered = <String>[];
    final trackAssetPaths = <String, String>{};

    for (final track in project.tracks) {
      final source = track.audioFilePath;
      if (source == null) continue;

      final file = File(source);
      if (!await file.exists()) {
        AppLogger.w('Audio file missing for track ${track.id}: $source');
        continue;
      }

      final ext = Lgdf.extensionOf(source);
      final assetName = 'audio_${track.id}$ext';
      final relPath = '${Lgdf.audioAssetsDir}/$assetName';
      final dest = File('${projectDir.path}/$relPath');

      final bytes = await file.readAsBytes();
      await dest.create(recursive: true);
      await dest.writeAsBytes(bytes);

      await _writeJson(
        File('${projectDir.path}/${Lgdf.audioMetadataDir}/$assetName.json'),
        Lgdf.buildAssetMetadata(
          path: relPath,
          size: bytes.length,
          sha256Hex: Lgdf.sha256Hex(bytes),
          createdTime: now,
          lastUpdateTime: now,
          extra: {
            'track_id': track.id,
            'track_name': track.name,
            'original_file_name': source.replaceAll('\\', '/').split('/').last,
          },
        ),
      );

      registered.add(relPath);
      trackAssetPaths[track.id] = relPath;
    }

    // ── 2. Description layer ──
    final documentBytes =
        Lgdf.encodeJsonBytes(_codec.buildProjectDocument(project, trackAssetPaths));
    await File('${projectDir.path}/${Lgdf.projectSpecFile}')
        .writeAsBytes(documentBytes);

    final slug = Lgdf.slugify(
      project.name,
      fallback: 'project-${project.id.isEmpty ? 'new' : project.id.substring(0, 8)}',
    );
    await _writeJson(
      File('${projectDir.path}/${Lgdf.configSpecFile}'),
      _codec.buildSpecConfig(project, slug),
    );
    await _writeJson(
      File('${projectDir.path}/${Lgdf.schemaSpecFile}'),
      _codec.buildSpecConfigSchema(),
    );
    await File('${projectDir.path}/${Lgdf.overviewSpecFile}')
        .writeAsString(_codec.buildOverview(project, trackAssetPaths));

    // ── 3. registry.json + info.json ──
    await _writeJson(
      File('${projectDir.path}/${Lgdf.registryFile}'),
      Lgdf.buildRegistry(registered),
    );
    await _writeJson(
      File('${projectDir.path}/${Lgdf.infoFile}'),
      _codec.buildInfo(
        project: project,
        createdTime: created,
        lastUpdateTime: now,
        version: existingInfo?.version ?? 1,
      ),
    );

    // ── 4. .lgdfignore ──
    final ignore = File('${projectDir.path}/${Lgdf.ignoreFile}');
    if (!await ignore.exists()) {
      await ignore.writeAsString(Lgdf.defaultIgnore);
    }

    return documentBytes.length;
  }

  /// Reads a directory-mode LGDF project, or null when [projectDir] is not one.
  Future<SerializedProject?> readProjectDirectory(Directory projectDir) async {
    try {
      final infoFile = File('${projectDir.path}/${Lgdf.infoFile}');
      if (!await infoFile.exists()) return null;
      final info =
          jsonDecode(await infoFile.readAsString()) as Map<String, dynamic>;
      if (info['format'] != Lgdf.format) return null;

      final docFile = File('${projectDir.path}/${Lgdf.projectSpecFile}');
      if (!await docFile.exists()) {
        AppLogger.e('LGDF project missing ${Lgdf.projectSpecFile}: ${projectDir.path}');
        return null;
      }
      final document =
          jsonDecode(await docFile.readAsString()) as Map<String, dynamic>;
      final project = _codec.parseProjectDocument(document);

      final trackAudioFiles = <String, String>{};
      for (final track in project.tracks) {
        final rel = track.audioFilePath;
        if (rel == null) continue;
        final full = File('${projectDir.path}/${Lgdf.normalizePath(rel)}');
        if (await full.exists()) {
          trackAudioFiles[track.id] = full.path;
        } else {
          AppLogger.w('LGDF asset not found: ${full.path}');
        }
      }

      return SerializedProject(
        project: project,
        trackAudioFiles: trackAudioFiles,
        lgdfInfo: LgdfProjectInfo(
          name: info['name'] as String? ?? Lgdf.slugify(project.name),
          displayName: info['display_name'] as String?,
          createdTime: (info['created_time'] as num?)?.toInt() ??
              DateTime.now().millisecondsSinceEpoch ~/ 1000,
          version: (info['version'] as num?)?.toInt() ?? 1,
        ),
      );
    } catch (e) {
      AppLogger.e('Failed to read LGDF project directory', e);
      return null;
    }
  }

  // ────────────────────────────────────────────────────────────
  // Archive mode
  // ────────────────────────────────────────────────────────────

  /// Packs a directory-mode project into archive bytes.
  ///
  /// `work/`, `dist/`, `.tmp/` and dotfiles are excluded; entry paths use `/`.
  Future<Uint8List> packProjectDirectory(Directory projectDir) async {
    final archive = Archive();
    final rootPath = projectDir.path.replaceAll('\\', '/');

    await for (final entity
        in projectDir.list(recursive: true, followLinks: false)) {
      if (entity is! File) continue;
      final full = entity.path.replaceAll('\\', '/');
      final rel =
          full.startsWith('$rootPath/') ? full.substring(rootPath.length + 1) : full;
      final normalized = Lgdf.normalizePath(rel);

      final top = normalized.split('/').first;
      if (Lgdf.excludedFromPack.contains(top)) continue;
      if (normalized.startsWith('.')) continue;

      final bytes = await entity.readAsBytes();
      archive.addFile(ArchiveFile(normalized, bytes.length, bytes));
    }

    final encoded = ZipEncoder().encode(archive);
    return Uint8List.fromList(encoded);
  }

  /// Serializes a [Project] into `.lgdf` archive bytes via a temporary
  /// directory-mode project.
  Future<Uint8List> serialize(
    Project project, {
    Map<String, Uint8List> audioFileBytes = const {},
  }) async {
    final temp = await Directory.systemTemp.createTemp('lgdf_pack_');
    try {
      if (audioFileBytes.isEmpty) {
        await writeProjectDirectory(project, temp);
      } else {
        await writeProjectDirectoryFromBytes(project, temp, audioFileBytes);
      }
      return await packProjectDirectory(temp);
    } finally {
      try {
        if (await temp.exists()) await temp.delete(recursive: true);
      } catch (_) {}
    }
  }

  /// Writes a directory-mode project where audio comes from memory
  /// (used on web, where files live behind blob URLs).
  Future<void> writeProjectDirectoryFromBytes(
    Project project,
    Directory projectDir,
    Map<String, Uint8List> audioFileBytes,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final slug = Lgdf.slugify(
      project.name,
      fallback: 'project-${project.id.isEmpty ? 'new' : project.id.substring(0, 8)}',
    );

    await _ensureDir(Directory('${projectDir.path}/${Lgdf.audioAssetsDir}'));
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.audioMetadataDir}'));
    await _ensureDir(Directory('${projectDir.path}/${Lgdf.specDir}'));

    final registered = <String>[];
    final trackAssetPaths = <String, String>{};

    for (final track in project.tracks) {
      final bytes = audioFileBytes[track.id];
      if (bytes == null) continue;
      final ext = Lgdf.extensionOf(track.audioFilePath ?? '.wav');
      final assetName = 'audio_${track.id}$ext';
      final relPath = '${Lgdf.audioAssetsDir}/$assetName';
      await File('${projectDir.path}/$relPath').writeAsBytes(bytes);

      await _writeJson(
        File('${projectDir.path}/${Lgdf.audioMetadataDir}/$assetName.json'),
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

    await File('${projectDir.path}/${Lgdf.projectSpecFile}')
        .writeAsBytes(
            Lgdf.encodeJsonBytes(_codec.buildProjectDocument(project, trackAssetPaths)));
    await _writeJson(
      File('${projectDir.path}/${Lgdf.configSpecFile}'),
      _codec.buildSpecConfig(project, slug),
    );
    await _writeJson(
      File('${projectDir.path}/${Lgdf.schemaSpecFile}'),
      _codec.buildSpecConfigSchema(),
    );
    await File('${projectDir.path}/${Lgdf.overviewSpecFile}')
        .writeAsString(_codec.buildOverview(project, trackAssetPaths));
    await _writeJson(
      File('${projectDir.path}/${Lgdf.registryFile}'),
      Lgdf.buildRegistry(registered),
    );
    await _writeJson(
      File('${projectDir.path}/${Lgdf.infoFile}'),
      _codec.buildInfo(project: project, createdTime: now, lastUpdateTime: now),
    );
  }

  /// Desktop stub — the web implementation performs the download.
  void downloadArchive(Uint8List bytes, String filename) {
    throw UnsupportedError('downloadArchive is only supported on web');
  }

  /// Reads a project archive (LGDF `.lgdf` or legacy `.zap`).
  ///
  /// Extracted resources land in a temp directory so [AudioService] can
  /// reference them by path.
  Future<SerializedProject?> deserialize(Uint8List bytes) async {
    try {
      final archive = ZipDecoder().decodeBytes(bytes.toList());

      final infoFile = archive.files.firstWhere(
        (f) => Lgdf.normalizePath(f.name) == Lgdf.infoFile,
        orElse: () => throw Exception('Missing info.json in project archive'),
      );
      final info =
          jsonDecode(utf8.decode(infoFile.content.toList())) as Map<String, dynamic>;

      if (info['format'] == Lgdf.format) {
        return _readLgdfArchive(archive, info);
      }
      return _readLegacyArchive(archive, info);
    } catch (e) {
      AppLogger.e('Failed to deserialize project', e);
      return null;
    }
  }

  Future<SerializedProject?> _readLgdfArchive(
    Archive archive,
    Map<String, dynamic> info,
  ) async {
    final docFile = archive.files.firstWhere(
      (f) => Lgdf.normalizePath(f.name) == Lgdf.projectSpecFile,
      orElse: () =>
          throw Exception('Missing ${Lgdf.projectSpecFile} in LGDF archive'),
    );
    final document =
        jsonDecode(utf8.decode(docFile.content.toList())) as Map<String, dynamic>;
    final project = _codec.parseProjectDocument(document);

    final extractDir = await _extract(archive, project.id);
    final trackAudioFiles = _resolveAssets(project, extractDir);

    return SerializedProject(
      project: project,
      trackAudioFiles: trackAudioFiles,
      lgdfInfo: LgdfProjectInfo(
        name: info['name'] as String? ?? Lgdf.slugify(project.name),
        displayName: info['display_name'] as String?,
        createdTime: (info['created_time'] as num?)?.toInt() ??
            DateTime.now().millisecondsSinceEpoch ~/ 1000,
        version: (info['version'] as num?)?.toInt() ?? 1,
      ),
    );
  }

  /// Migrates a legacy `.zap` archive (info.json held every field).
  Future<SerializedProject?> _readLegacyArchive(
    Archive archive,
    Map<String, dynamic> info,
  ) async {
    final project = _codec.parseLegacyInfo(info);
    final extractDir = await _extract(archive, project.id);
    AppLogger.i('Legacy .zap project migrated on read: ${project.name}');
    return SerializedProject(
      project: project,
      trackAudioFiles: _resolveAssets(project, extractDir),
    );
  }

  Future<Directory> _extract(Archive archive, String projectId) async {
    final tempDir = await getTemporaryDirectory();
    final safeId = projectId.isEmpty ? 'unknown' : projectId;
    final extractDir = Directory('${tempDir.path}/lgdf_extract_$safeId');
    if (await extractDir.exists()) await extractDir.delete(recursive: true);
    await extractDir.create(recursive: true);

    for (final file in archive.files) {
      if (!file.isFile) continue;
      final rel = Lgdf.normalizePath(file.name);
      if (rel.startsWith('..')) continue; // zip-slip guard
      final dest = File('${extractDir.path}/$rel');
      await dest.create(recursive: true);
      await dest.writeAsBytes(file.content.toList());
    }
    return extractDir;
  }

  Map<String, String> _resolveAssets(Project project, Directory extractDir) {
    final files = <String, String>{};
    for (final track in project.tracks) {
      final rel = track.audioFilePath;
      if (rel == null) continue;
      final full = '${extractDir.path}/${Lgdf.normalizePath(rel)}';
      if (File(full).existsSync()) files[track.id] = full;
    }
    return files;
  }

  // ────────────────────────────────────────────────────────────
  // Helpers
  // ────────────────────────────────────────────────────────────

  Future<void> _writeJson(File file, Object? value) async {
    await file.create(recursive: true);
    await file.writeAsBytes(Lgdf.encodeJsonBytes(value));
  }

  Future<void> _ensureDir(Directory dir) async {
    if (!await dir.exists()) await dir.create(recursive: true);
  }
}

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:path_provider/path_provider.dart';

import '../core/utils/logger.dart';
import 'lgdf_format.dart';
import 'workspace_service.dart';

/// Local home for all projects created by Zenith Audio.
///
/// - iOS: the app's Documents directory itself. `UIFileSharingEnabled` +
///   `LSSupportsOpeningDocumentsInPlace` make it appear in the Files app as a
///   folder named after the app — that folder *is* the workspace.
/// - Other platforms: `<Documents>/Zenith Audio/Workspace`.
class WorkspaceService {
  static const _folderName = 'Zenith Audio/Workspace';

  /// True when running on iOS (the Files-app-visible Documents root).
  static bool get _usesDocumentsRoot {
    if (kIsWeb) return false;
    try {
      return Platform.isIOS;
    } catch (_) {
      return false;
    }
  }

  /// The workspace directory, created on demand.
  Future<Directory> directory() async {
    final documents = await getApplicationDocumentsDirectory();
    final folder = _usesDocumentsRoot
        ? documents
        : Directory('${documents.path}/$_folderName');
    if (!await folder.exists()) await folder.create(recursive: true);
    return folder;
  }

  /// Absolute path of the workspace folder (created on demand).
  Future<String> directoryPath() async => (await directory()).path;

  /// Lists workspace entries: LGDF project directories first, then legacy
  /// `.zap` files and exported `.lgdf` archives. Newest first.
  Future<List<WorkspaceProjectFile>> projects() async {
    final folder = await directory();
    final entries = <WorkspaceProjectFile>[];

    await for (final entity in folder.list(followLinks: false)) {
      if (entity is Directory) {
        final info = File('${entity.path}/${Lgdf.infoFile}');
        if (!await info.exists()) continue;
        final stat = await entity.stat();
        final summary = await _readInfoSummary(info);
        entries.add(WorkspaceProjectFile(
          path: entity.path,
          name: entity.uri.pathSegments
              .lastWhere((s) => s.isNotEmpty, orElse: () => entity.path),
          displayName: summary.name,
          modified: summary.modified ?? stat.modified,
          size: await _directorySize(entity),
          isDirectory: true,
        ));
      } else if (entity is File) {
        final lower = entity.path.toLowerCase();
        final isLgdf = lower.endsWith(Lgdf.extension);
        final isLegacy = lower.endsWith('.zap');
        if (!isLgdf && !isLegacy) continue;
        final stat = await entity.stat();
        entries.add(WorkspaceProjectFile(
          path: entity.path,
          name: entity.uri.pathSegments.last,
          modified: stat.modified,
          size: stat.size,
          isDirectory: false,
        ));
      }
    }

    entries.sort((a, b) => b.modified.compareTo(a.modified));
    return entries;
  }

  /// Directory to create for a new project with [projectName].
  ///
  /// The folder name is an LGDF slug; `untitled` becomes a generated name.
  Future<String> defaultProjectDirectory(
    String projectName,
    String projectId,
  ) async {
    final folder = await directory();
    final trimmed = projectName.trim();
    final slug = (trimmed.isEmpty || trimmed.toLowerCase() == 'untitled')
        ? 'project-${projectId.isEmpty ? 'new' : projectId.substring(0, 8)}'
        : Lgdf.slugify(trimmed, fallback: 'project');
    return '${folder.path}/$slug';
  }

  /// Legacy alias kept for callers that still ask for a file path.
  Future<String> defaultProjectPath(String projectName, String projectId) async {
    final dir = await defaultProjectDirectory(projectName, projectId);
    return '$dir${Lgdf.extension}';
  }

  /// Deletes a workspace entry (directory or file), recursively.
  Future<void> deleteProject(String path) async {
    final type = await FileSystemEntity.type(path);
    if (type == FileSystemEntityType.directory) {
      await Directory(path).delete(recursive: true);
    } else if (type == FileSystemEntityType.file) {
      await File(path).delete();
    }
  }

  /// Reads the LGDF display name and `last_update_time` from `info.json`.
  Future<({String? name, DateTime? modified})> _readInfoSummary(File info) async {
    try {
      final json = jsonDecode(await info.readAsString()) as Map<String, dynamic>;
      final display = json['display_name'] as String?;
      final updated = (json['last_update_time'] as num?)?.toInt();
      return (
        name: (display != null && display.trim().isNotEmpty) ? display : null,
        modified: updated != null
            ? DateTime.fromMillisecondsSinceEpoch(updated * 1000)
            : null,
      );
    } catch (e) {
      AppLogger.w('Unreadable ${Lgdf.infoFile} at ${info.path}: $e');
      return (name: null, modified: null);
    }
  }

  /// Recursive size of a project directory (bounded walk).
  Future<int> _directorySize(Directory dir) async {
    var total = 0;
    try {
      await for (final entity in dir.list(recursive: true, followLinks: false)) {
        if (entity is File) {
          try {
            total += await entity.length();
          } catch (_) {}
        }
      }
    } catch (_) {}
    return total;
  }
}

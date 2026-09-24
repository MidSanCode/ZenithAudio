import 'dart:io';

import 'package:path_provider/path_provider.dart';

import '../core/constants/app_constants.dart';
import 'workspace_service.dart';

/// Local home for all projects created by Zenith Audio.
class WorkspaceService {
  static const _folderName = 'Zenith Audio/Workspace';

  Future<Directory> directory() async {
    final documents = await getApplicationDocumentsDirectory();
    final folder = Directory('${documents.path}/$_folderName');
    if (!await folder.exists()) await folder.create(recursive: true);
    return folder;
  }

  /// Absolute path of the workspace folder (created on demand).
  Future<String> directoryPath() async => (await directory()).path;

  Future<List<WorkspaceProjectFile>> projects() async {
    final folder = await directory();
    final files = <WorkspaceProjectFile>[];
    await for (final entity in folder.list()) {
      if (entity is! File ||
          !entity.path.toLowerCase().endsWith(AppConstants.projectExtension)) {
        continue;
      }
      final stat = await entity.stat();
      files.add(WorkspaceProjectFile(
        path: entity.path,
        name: entity.uri.pathSegments.last,
        modified: stat.modified,
        size: stat.size,
      ));
    }
    files.sort((a, b) => b.modified.compareTo(a.modified));
    return files;
  }

  Future<String> defaultProjectPath(String projectName, String projectId) async {
    final folder = await directory();
    final safeName = projectName.trim().replaceAll(RegExp(r'[\\/:*?"<>|]'), '_');
    final name = safeName.isEmpty || safeName.toLowerCase() == 'untitled'
        ? 'Project_${projectId.substring(0, 8)}'
        : safeName;
    return '${folder.path}/$name${AppConstants.projectExtension}';
  }

  Future<void> deleteProject(String path) async {
    final file = File(path);
    if (await file.exists()) await file.delete();
  }
}
import 'workspace_service.dart';

/// Web has no writable local workspace — projects are saved through browser
/// downloads, so the workspace list is always empty here.
class WorkspaceService {
  Future<String> directory() async => '/Zenith Audio/Workspace';

  Future<String> directoryPath() async => '/Zenith Audio/Workspace';

  Future<List<WorkspaceProjectFile>> projects() async => const [];

  Future<String> defaultProjectDirectory(String projectName, String projectId) async {
    return '/Zenith Audio/Workspace/$projectName';
  }

  Future<String> defaultProjectPath(String projectName, String projectId) async {
    return '$projectName.lgdf';
  }

  Future<void> deleteProject(String path) async {}
}

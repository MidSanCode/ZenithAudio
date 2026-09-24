import 'workspace_service.dart';

/// Web has no writable local workspace directory — projects are saved via
/// browser download, so the workspace list is always empty here.
class WorkspaceService {
  Future<String> directoryPath() async => '/Zenith Audio/Workspace';

  Future<List<WorkspaceProjectFile>> projects() async => const [];

  Future<String> defaultProjectPath(String projectName, String projectId) async {
    return '$projectName.zap';
  }

  Future<void> deleteProject(String path) async {}
}
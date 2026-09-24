// Project workspace abstraction.
// Desktop: lists projects saved into Documents/Zenith Audio/Workspace.
// Web: no local file system — the workspace is empty and projects are
// downloaded through the browser instead.
export 'workspace_service_io.dart'
    if (dart.library.html) 'workspace_service_web.dart';

/// Metadata for a single project file inside the workspace.
class WorkspaceProjectFile {
  final String path;
  final String name;
  final DateTime modified;
  final int size;

  const WorkspaceProjectFile({
    required this.path,
    required this.name,
    required this.modified,
    required this.size,
  });
}
// Project workspace abstraction.
//
// Desktop: lists LGDF directory-mode projects stored in
// `Documents/Zenith Audio/Workspace`.
// iOS: the workspace *is* the app's Documents directory, which the Files app
// exposes as a folder named after the app (see Info.plist
// `UIFileSharingEnabled` / `LSSupportsOpeningDocumentsInPlace`).
// Web: no writable local file system — the workspace is empty and projects are
// downloaded through the browser instead.
export 'workspace_service_io.dart'
    if (dart.library.html) 'workspace_service_web.dart';

import 'lgdf_format.dart';

/// One entry inside the workspace.
///
/// LGDF projects are directories (`isDirectory == true`); legacy single-file
/// `.zaproj` archives (and legacy `.lgdf` / `.zap`) are files.
class WorkspaceProjectFile {
  final String path;
  final String name;

  /// Display name from LGDF `info.json` (`display_name`), when available.
  final String? displayName;

  final DateTime modified;
  final int size;
  final bool isDirectory;

  const WorkspaceProjectFile({
    required this.path,
    required this.name,
    required this.modified,
    required this.size,
    this.displayName,
    this.isDirectory = true,
  });

  /// Name shown in the UI: the LGDF display name wins when present.
  String get label {
    final display = displayName?.trim();
    if (display != null && display.isNotEmpty) return display;
    // Directory-mode projects are named by their folder; archives have their
    // container extension removed so `my-song.zaproj` reads as `my-song`.
    if (isDirectory) return name;
    return Lgdf.stripArchiveExtension(name);
  }
}

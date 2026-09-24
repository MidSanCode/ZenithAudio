import '../models/project.dart';

/// Result returned after reading a project (directory mode or archive).
///
/// Shared by the desktop and web serializers so both platforms expose an
/// identical API surface.
class SerializedProject {
  final Project project;

  /// Track id → absolute path (desktop) or object URL (web) of its audio file.
  final Map<String, String> trackAudioFiles;

  /// LGDF `info.json` identity, preserved so a re-save keeps name/version.
  final LgdfProjectInfo? lgdfInfo;

  const SerializedProject({
    required this.project,
    required this.trackAudioFiles,
    this.lgdfInfo,
  });
}

/// LGDF `info.json` values that Zenith Audio preserves across saves.
class LgdfProjectInfo {
  final String name;
  final String? displayName;
  final int createdTime;
  final int version;

  const LgdfProjectInfo({
    required this.name,
    this.displayName,
    required this.createdTime,
    this.version = 1,
  });
}

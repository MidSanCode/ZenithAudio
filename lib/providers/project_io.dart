part of 'project_provider.dart';

/// Serialization, auto-save recovery and workspace IO for [ProjectNotifier],
/// split out of the main provider in S0 (the provider exceeded the 800-line
/// rule). Declared as a `part` so the file path / LGDF identity stay private.
extension ProjectIo on ProjectNotifier {
  /// Check if an auto-save cache exists for recovery.
  static Future<String?> findAutoSaveCache(String projectId) async {
    try {
      final dir = await getApplicationDocumentsDirectory();
      final path = '${dir.path}/.autosave/$projectId${Lgdf.extension}';
      if (await File(path).exists()) return path;
    } catch (_) {}
    return null;
  }

  /// Loads an already-deserialized project (used by the launch flow).
  ///
  /// The project is not attached to a workspace directory until it is saved.
  Future<bool> loadSerializedProject(SerializedProject serialized,
      {bool markDirty = true}) async {
    try {
      _pushUndo();
      stopAutoSave();
      await _loadSerialized(serialized);
      if (markDirty) {
        _currentFilePath = null;
        _isDirty = true;
      }
      startAutoSave();
      AppLogger.i('Project loaded from external archive: ${state.name}');
      return true;
    } catch (e) {
      AppLogger.e('Failed to load external project', e);
      return false;
    }
  }

  Future<void> _loadSerialized(SerializedProject serialized) async {
    await ref.read(audioEngineProvider).unloadAll();
    _lgdfInfo = serialized.lgdfInfo;
    final updatedTracks = serialized.project.tracks.map((t) {
      if (t.type == TrackType.audio) {
        final audioPath = serialized.trackAudioFiles[t.id];
        return audioPath != null ? t.copyWith(audioFilePath: audioPath) : t;
      }
      return t;
    }).toList();
    state = serialized.project.copyWith(tracks: updatedTracks);
    _isDirty = true;
    for (final track in state.tracks) {
      if (track.type == TrackType.audio && track.audioFilePath != null) {
        ref.read(audioEngineProvider).loadTrack(track).then((dur) {
          final updated = track.copyWith(duration: dur);
          state = state.copyWith(
            tracks:
                state.tracks.map((t) => t.id == track.id ? updated : t).toList(),
          );
        });
      }
    }
  }

  // ──── Save / Open ────

  /// Saves the project into the app workspace as an LGDF directory-mode
  /// project. Returns true on success.
  Future<bool> saveProject() async {
    try {
      AppLogger.i('Saving project...');
      final serializer = const ProjectSerializer();

      if (kIsWeb) {
        // The browser cannot hold a project directory — emit an .lgdf archive.
        final bytes = await serializer.serialize(state);
        serializer.downloadArchive(
          bytes,
          '${Lgdf.slugify(state.name)}${Lgdf.extension}',
        );
        _isDirty = false;
        AppLogger.i('Project saved via browser download');
        return true;
      }

      // Resolve the project directory inside the workspace.
      final Directory projectDir;
      if (_currentFilePath != null &&
          await Directory(_currentFilePath!).exists()) {
        projectDir = Directory(_currentFilePath!);
      } else {
        final path = await WorkspaceService()
            .defaultProjectDirectory(state.name, state.id);
        projectDir = Directory(path);
      }

      await serializer.writeProjectDirectory(
        state,
        projectDir,
        existingInfo: _lgdfInfo,
      );

      _currentFilePath = projectDir.path;
      _isDirty = false;
      clearAutoSaveCache();
      ref.invalidate(workspaceProjectsProvider);
      AppLogger.i('Project saved to: ${projectDir.path}');
      return true;
    } catch (e) {
      AppLogger.e('Failed to save project', e);
      return false;
    }
  }

  /// Returns the project directory when the current project has one.
  Future<Directory?> _currentProjectDir() async {
    final path = _currentFilePath;
    if (path == null) return null;
    final dir = Directory(path);
    return await dir.exists() ? dir : null;
  }

  /// Exports the current project as a portable `.lgdf` archive (plus a
  /// `.sha256` companion) without changing the workspace project.
  ///
  /// Returns true on success; false when the user cancels or export fails.
  Future<bool> exportProject() async {
    try {
      final serializer = const ProjectSerializer();

      if (kIsWeb) {
        final bytes = await serializer.serialize(state);
        serializer.downloadArchive(
          bytes,
          '${Lgdf.slugify(state.name)}${Lgdf.extension}',
        );
        return true;
      }

      // Prefer packing the on-disk project so `work/`-style exclusions apply;
      // fall back to an in-memory conversion for never-saved projects.
      final dir = await _currentProjectDir() ?? await _stageTempProject();
      if (dir == null) return false;

      final bytes = await serializer.packProjectDirectory(dir);
      final fileName = '${Lgdf.slugify(state.name)}${Lgdf.extension}';
      final outputPath = await FilePicker.platform.saveFile(
        dialogTitle: 'menu.file.exportProject'.tr(),
        fileName: fileName,
        type: FileType.custom,
        allowedExtensions: AppConstants.projectOpenExtensions,
      );
      if (outputPath == null) return false;

      await File(outputPath).writeAsBytes(bytes);
      await _writeChecksumCompanion(outputPath, bytes);

      AppLogger.i('Project exported to: $outputPath');
      return true;
    } catch (e) {
      AppLogger.e('Project export failed', e);
      return false;
    }
  }

  /// Writes the `<package>.sha256` companion required by the standard.
  Future<void> _writeChecksumCompanion(
      String archivePath, Uint8List bytes) async {
    try {
      final digest = Lgdf.sha256Hex(bytes);
      final name = archivePath.replaceAll('\\', '/').split('/').last;
      await File('$archivePath.sha256').writeAsString('$digest  $name\n');
    } catch (e) {
      // A missing companion must not fail an otherwise good export.
      AppLogger.w('Could not write .sha256 companion: $e');
    }
  }

  /// Materializes the in-memory project into a temp directory for packing.
  Future<Directory?> _stageTempProject() async {
    try {
      final temp = await Directory.systemTemp.createTemp('zenith_export_');
      await const ProjectSerializer()
          .writeProjectDirectory(state, temp, existingInfo: _lgdfInfo);
      return temp;
    } catch (e) {
      AppLogger.e('Could not stage project for export', e);
      return null;
    }
  }

  /// Exports a workspace entry (LGDF project directory or legacy archive) to a
  /// user-chosen `.lgdf` path without opening it. Returns true on success.
  Future<bool> exportWorkspaceFile(String path) async {
    try {
      final serializer = const ProjectSerializer();
      final type = await FileSystemEntity.type(path);

      Uint8List bytes;
      String baseName;
      if (type == FileSystemEntityType.directory) {
        final dir = Directory(path);
        bytes = await serializer.packProjectDirectory(dir);
        baseName = dir.uri.pathSegments.lastWhere(
          (s) => s.isNotEmpty,
          orElse: () => 'project',
        );
      } else if (type == FileSystemEntityType.file) {
        final file = File(path);
        final name = file.uri.pathSegments.last;
        // Already an archive — copy it through unchanged, but re-extension it
        // to the current container so old `.lgdf`/`.zap` exports come out as
        // `.zaproj`.
        bytes = await file.readAsBytes();
        baseName = Lgdf.stripArchiveExtension(name);
      } else {
        return false;
      }

      final outputPath = await FilePicker.platform.saveFile(
        dialogTitle: 'menu.file.exportProject'.tr(),
        fileName: '$baseName${Lgdf.extension}',
        type: FileType.custom,
        allowedExtensions: AppConstants.projectOpenExtensions,
      );
      if (outputPath == null) return false;

      await File(outputPath).writeAsBytes(bytes);
      await _writeChecksumCompanion(outputPath, bytes);
      AppLogger.i('Workspace entry exported to: $outputPath');
      return true;
    } catch (e) {
      AppLogger.e('Workspace export failed', e);
      return false;
    }
  }

  /// Deletes a project from the workspace. Returns true on success.
  Future<bool> deleteWorkspaceFile(String path) async {
    try {
      await WorkspaceService().deleteProject(path);
      ref.invalidate(workspaceProjectsProvider);
      AppLogger.i('Workspace entry deleted: $path');
      return true;
    } catch (e) {
      AppLogger.e('Workspace delete failed', e);
      return false;
    }
  }

  /// Loads a workspace entry (LGDF project directory or legacy archive).
  /// Returns true on success.
  Future<bool> openWorkspaceProject(String path) async {
    try {
      final type = await FileSystemEntity.type(path);

      if (type == FileSystemEntityType.directory) {
        final serialized = await const ProjectSerializer()
            .readProjectDirectory(Directory(path));
        if (serialized == null) return false;
        await _loadSerialized(serialized);
        _currentFilePath = path;
        _isDirty = false;
        AppLogger.i('Workspace project loaded: $path');
        return true;
      }

      if (type == FileSystemEntityType.file) {
        final bytes = await File(path).readAsBytes();
        final serialized = await const ProjectSerializer().deserialize(bytes);
        if (serialized == null) return false;
        await _loadSerialized(serialized);
        // A legacy single-file project stays read-only until saved, which
        // converts it into an LGDF project directory in the workspace.
        _currentFilePath = null;
        _isDirty = true;
        AppLogger.i('Archive project loaded: $path');
        return true;
      }

      return false;
    } catch (e) {
      AppLogger.e('Failed to load workspace project', e);
      return false;
    }
  }

  /// Picks an `.lgdf` (or legacy `.zap`) project archive and loads it.
  /// Returns true when a project was loaded.
  Future<bool> openProject({
    Future<bool> Function(ProjectProbe probe)? confirmMigration,
  }) async {
    _pushUndo();
    _isDirty = false;
    stopAutoSave();
    try {
      AppLogger.i('Opening project...');

      final result = await FilePicker.platform.pickFiles(
        type: FileType.custom,
        allowedExtensions: AppConstants.projectOpenExtensions,
      );

      if (result == null || result.files.isEmpty) return false;

      final file = result.files.single;

      Uint8List bytes;
      if (kIsWeb) {
        final webBytes = file.bytes;
        if (webBytes == null) return false;
        bytes = webBytes;
      } else if (file.path != null) {
        bytes = await File(file.path!).readAsBytes();
      } else {
        return false;
      }

      // Probe the archive before opening so a migration wizard can tell the
      // user what they are about to load. Detection is by content, not the
      // extension, so a renamed file still opens correctly.
      final probe = ProjectMigrationService.inspect(bytes, fileName: file.name);
      if (!probe.isOpenable) {
        AppLogger.e('Not a Zenith project archive: ${file.name} '
            '(${probe.formatLabel})');
        return false;
      }
      if ((probe.needsMigration || probe.documentVersion != null) &&
          confirmMigration != null) {
        final proceed = await confirmMigration(probe);
        if (!proceed) return false;
      }

      final serialized = await const ProjectSerializer().deserialize(bytes);
      if (serialized == null) {
        AppLogger.e('Failed to deserialize project');
        return false;
      }

      await _loadSerialized(serialized);
      // An opened archive is not yet a workspace project: saving will create
      // the LGDF directory for it.
      _currentFilePath = null;
      _isDirty = true;
      AppLogger.i('Project loaded: ${state.name} '
          '(format ${probe.formatLabel}'
          '${probe.needsMigration ? ', migrated' : ''})');
      return true;
    } catch (e) {
      AppLogger.e('Failed to open project', e);
      return false;
    }
  }

  /// Picks an existing LGDF project *folder* and loads it.
  /// Returns true when a project was loaded.
  Future<bool> openProjectFolder() async {
    _pushUndo();
    _isDirty = false;
    stopAutoSave();
    try {
      final path = await FilePicker.platform.getDirectoryPath(
        dialogTitle: 'workspace.openFolder'.tr(),
      );
      if (path == null) return false;

      final serialized =
          await const ProjectSerializer().readProjectDirectory(Directory(path));
      if (serialized == null) {
        AppLogger.e('Selected folder is not an LGDF project: $path');
        return false;
      }

      await _loadSerialized(serialized);
      _currentFilePath = path;
      _isDirty = false;
      AppLogger.i('Project folder loaded: $path');
      return true;
    } catch (e) {
      AppLogger.e('Failed to open project folder', e);
      return false;
    }
  }
}

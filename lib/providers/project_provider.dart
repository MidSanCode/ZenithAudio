import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:file_picker/file_picker.dart';
import 'package:uuid/uuid.dart';
import 'package:path_provider/path_provider.dart';
import '../models/project.dart';
import '../models/track.dart';
import '../models/note.dart';
import '../models/instrument.dart';
import '../core/constants/app_constants.dart';
import '../core/utils/logger.dart';
import '../services/audio_service.dart';
import '../services/lgdf_format.dart';
import '../services/project_serializer.dart';
import '../services/workspace_service.dart';
import 'workspace_provider.dart';
import '../services/synth_engine.dart' show TrackCompressorParams;
import 'settings_provider.dart';

final projectProvider = NotifierProvider<ProjectNotifier, Project>(
  ProjectNotifier.new,
);

class ProjectNotifier extends Notifier<Project> {
  static const _uuid = Uuid();
  static const int _maxUndo = 50;

  /// Current project directory inside the workspace (set after first save or
  /// when an LGDF directory project is opened).
  String? _currentFilePath;

  /// LGDF identity of the loaded project, so a re-save keeps its created time
  /// and version.
  LgdfProjectInfo? _lgdfInfo;

  /// Tracks whether there are unsaved changes.
  bool _isDirty = false;

  final List<Project> _undoStack = [];
  final List<Project> _redoStack = [];

  /// Auto-save timer.
  Timer? _autoSaveTimer;

  bool get isDirty => _isDirty;

  /// Confirm discard if dirty. Returns true if user confirms discard/cancel.
  Future<bool> confirmDiscard(BuildContext context) async {
    if (!_isDirty) return true;
    final result = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('proj.unsavedTitle'.tr()),
        content: Text('proj.unsavedMessage'.tr()),
        actions: [
          TextButton(onPressed: () => Navigator.of(ctx).pop('cancel'), child: Text('proj.cancel'.tr())),
          TextButton(onPressed: () => Navigator.of(ctx).pop('discard'), child: Text('proj.discard'.tr())),
          FilledButton(onPressed: () => Navigator.of(ctx).pop('save'), child: Text('proj.save'.tr())),
        ],
      ),
    );
    if (result == 'save') {
      await saveProject();
      // If the user cancelled the save dialog the project is still dirty —
      // treat that as "not confirmed" so the caller keeps the app open
      // instead of losing the unsaved changes.
      return !_isDirty;
    }
    return result == 'discard';
  }

  /// Leaves the editor and returns to the workspace.
  ///
  /// Prompts to save when there are unsaved changes — cancelling the prompt
  /// keeps the user in the editor. Returns true when navigation happened.
  Future<bool> leaveEditor(BuildContext context) async {
    // Capture the navigator before the await so we never touch a disposed
    // BuildContext.
    final navigator = Navigator.of(context);
    if (!navigator.canPop()) return false;

    final confirmed = await confirmDiscard(context);
    if (!confirmed) return false;
    navigator.pop();
    return true;
  }

  @override
  Project build() {
    ref.onDispose(() {
      ref.read(audioServiceProvider).dispose();
      _autoSaveTimer?.cancel();
    });
    WidgetsBinding.instance.addPostFrameCallback((_) => startAutoSave());
    return Project(id: _uuid.v4(), name: 'untitled');
  }

  bool get canUndo => _undoStack.isNotEmpty;
  bool get canRedo => _redoStack.isNotEmpty;

  Project _deepClone(Project p) {
    return Project(
      id: p.id,
      name: p.name,
      tracks: p.tracks.map((t) => t.copyWith(
        notes: t.notes.map((n) => n.copyWith()).toList(),
      )).toList(),
      sampleRate: p.sampleRate,
      timeSignatureNumerator: p.timeSignatureNumerator,
      timeSignatureDenominator: p.timeSignatureDenominator,
      keySignature: p.keySignature,
      bpm: p.bpm,
      playbackSpeed: p.playbackSpeed,
    );
  }

  void _pushUndo() {
    _undoStack.add(_deepClone(state));
    if (_undoStack.length > _maxUndo) _undoStack.removeAt(0);
    _redoStack.clear();
  }

  void undo() {
    if (_undoStack.isEmpty) return;
    _redoStack.add(_deepClone(state));
    state = _undoStack.removeLast();
    AppLogger.i('Undo');
  }

  void redo() {
    if (_redoStack.isEmpty) return;
    _undoStack.add(_deepClone(state));
    state = _redoStack.removeLast();
    AppLogger.i('Redo');
  }

  void _markDirty() => _isDirty = true;

  // ──── Auto-Save ────

  void startAutoSave() {
    _autoSaveTimer?.cancel();
    final settings = ref.read(settingsProvider);
    if (!settings.autoSaveEnabled) return;
    final interval = Duration(minutes: settings.autoSaveIntervalMinutes);
    _autoSaveTimer = Timer.periodic(interval, (_) => _autoSave());
  }

  void stopAutoSave() {
    _autoSaveTimer?.cancel();
    _autoSaveTimer = null;
  }

  Future<void> _autoSave() async {
    if (!_isDirty) return;
    try {
      final bytes = await const ProjectSerializer().serialize(state);
      final dir = await getApplicationDocumentsDirectory();
      final autoDir = Directory('${dir.path}/.autosave');
      if (!await autoDir.exists()) await autoDir.create(recursive: true);
      final path = '${autoDir.path}/${state.id}${Lgdf.extension}';
      await File(path).writeAsBytes(bytes);
      AppLogger.d('Auto-saved to $path');
    } catch (e) {
      AppLogger.e('Auto-save failed', e);
    }
  }

  /// Check if an auto-save cache exists for recovery.
  static Future<String?> findAutoSaveCache(String projectId) async {
    try {
      final dir = await getApplicationDocumentsDirectory();
      final path = '${dir.path}/.autosave/$projectId${Lgdf.extension}';
      if (await File(path).exists()) return path;
    } catch (_) {}
    return null;
  }

  /// Check for auto-save cache on startup and offer recovery.
  static Future<void> checkForAutoSaveRecovery(BuildContext context, WidgetRef ref) async {
    try {
      final dir = await getApplicationDocumentsDirectory();
      final autoDir = Directory('${dir.path}/.autosave');
      if (!await autoDir.exists()) return;
      final files = await autoDir
          .list()
          .where((e) =>
              e.path.endsWith(Lgdf.extension) || e.path.endsWith('.zap'))
          .toList();
      if (files.isEmpty) return;
      if (!context.mounted) return;
      final recover = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: Text('proj.recoveryTitle'.tr()),
          content: Text('proj.recoveryMessage'.tr(namedArgs: {'n': '${files.length}'})),
          actions: [
            TextButton(onPressed: () => Navigator.of(ctx).pop(false), child: Text('proj.noRestore'.tr())),
            FilledButton(onPressed: () => Navigator.of(ctx).pop(true), child: Text('proj.restore'.tr())),
          ],
        ),
      );
      if (recover == true && files.isNotEmpty && context.mounted) {
        // Open the most recent auto-save
        final newest = files.reduce((a, b) =>
          File(a.path).statSync().modified.isAfter(File(b.path).statSync().modified) ? a : b);
        final bytes = await File(newest.path).readAsBytes();
        final serialized = await const ProjectSerializer().deserialize(bytes);
        if (serialized != null && context.mounted) {
          final notifier = ref.read(projectProvider.notifier);
          await notifier._loadSerialized(serialized);
        }
      }
    } catch (_) {}
  }

  /// Loads an already-deserialized project (used by the launch flow).
  ///
  /// The project is not attached to a workspace directory until it is saved.
  Future<bool> loadSerializedProject(SerializedProject serialized) async {
    try {
      _pushUndo();
      stopAutoSave();
      await _loadSerialized(serialized);
      _currentFilePath = null;
      _isDirty = true;
      startAutoSave();
      AppLogger.i('Project loaded from external archive: ${state.name}');
      return true;
    } catch (e) {
      AppLogger.e('Failed to load external project', e);
      return false;
    }
  }

  Future<void> _loadSerialized(SerializedProject serialized) async {
    await ref.read(audioServiceProvider).unloadAll();
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
        ref.read(audioServiceProvider).loadTrack(track).then((dur) {
          final updated = track.copyWith(duration: dur);
          state = state.copyWith(
            tracks: state.tracks.map((t) => t.id == track.id ? updated : t).toList(),
          );
        });
      }
    }
  }

  /// Clear auto-save cache after manual save.
  Future<void> clearAutoSaveCache() async {
    try {
      final dir = await getApplicationDocumentsDirectory();
      for (final ext in [Lgdf.extension, '.zap']) {
        final file = File('${dir.path}/.autosave/${state.id}$ext');
        if (await file.exists()) await file.delete();
      }
    } catch (_) {}
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
        final path =
            await WorkspaceService().defaultProjectDirectory(state.name, state.id);
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
  Future<void> _writeChecksumCompanion(String archivePath, Uint8List bytes) async {
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
  Future<bool> openProject() async {
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
      AppLogger.i('Project loaded: ${state.name}');
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

  // ──── Track management ────

  void addAudioTrack({String? name, String? audioFilePath}) {
    _pushUndo();
    _markDirty();
    final trackColors = _trackColors();
    final track = Track(
      id: _uuid.v4(),
      name: name ?? 'Track ${state.tracks.length + 1}',
      type: TrackType.audio,
      volume: 0.8,
      audioFilePath: audioFilePath,
      color: trackColors[state.tracks.length % trackColors.length],
    );

    state = state.copyWith(tracks: [...state.tracks, track]);
    if (audioFilePath != null) {
      ref.read(audioServiceProvider).loadTrack(track).then((dur) {
        final updated = track.copyWith(duration: dur);
        state = state.copyWith(
          tracks: state.tracks.map((t) => t.id == track.id ? updated : t).toList(),
        );
        AppLogger.d('Track "${track.name}" duration: ${dur.toStringAsFixed(1)}s');
      });
    }
    AppLogger.i('Added audio track: ${track.name}');
  }

  void addInstrumentTrack({String? name, String? instrumentName}) {
    _pushUndo();
    _markDirty();
    final trackColors = _trackColors();
    // Synth-family presets get their own track type so the UI can
    // distinguish them; they still share the note/render pipeline.
    final presetId = instrumentName ?? 'piano';
    final preset = InstrumentPreset.fromIdOrNull(presetId);
    final isSynthPreset = preset != null &&
        (preset.category == InstrumentCategory.synth ||
            preset.id.startsWith('syn_'));
    final track = Track(
      id: _uuid.v4(),
      name: name ?? 'Track ${state.tracks.length + 1}',
      type: isSynthPreset ? TrackType.synth : TrackType.instrument,
      instrumentName: instrumentName ?? 'piano',
      volume: 0.8,
      color: trackColors[state.tracks.length % trackColors.length],
      stepPattern: List.generate(16, (_) => false),
    );

    state = state.copyWith(tracks: [...state.tracks, track]);
    AppLogger.i('Added instrument track: ${track.name}');
  }

  void addTrack({String? name, String? audioFilePath}) {
    addAudioTrack(name: name, audioFilePath: audioFilePath);
  }

  Future<void> removeTrack(String trackId) async {
    _pushUndo();
    _markDirty();
    await ref.read(audioServiceProvider).unloadTrack(trackId);
    final removedName = state.tracks.firstWhere((t) => t.id == trackId).name;
    state = state.copyWith(
      tracks: state.tracks.where((t) => t.id != trackId).toList(),
    );
    AppLogger.i('Deleted track: $removedName');
  }

  void updateTrackVolume(String trackId, double volume) {
    _markDirty();
    final trackName = state.tracks.firstWhere((t) => t.id == trackId).name;
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(volume: volume);
        return t;
      }).toList(),
    );
    ref.read(audioServiceProvider).updateTrackVolume(trackId, volume);
    AppLogger.d('Track "$trackName" volume: ${(volume * 100).toInt()}%');
  }

  void updateTrackPan(String trackId, double pan) {
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(pan: pan.clamp(-1.0, 1.0));
        return t;
      }).toList(),
    );
  }

  void toggleTrackStep(String trackId, int step) {
    _markDirty();
    final track = state.tracks.firstWhere((t) => t.id == trackId);
    final pattern = List<bool>.from(track.stepPattern);
    if (step >= 0 && step < pattern.length) {
      pattern[step] = !pattern[step];
    }
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(stepPattern: pattern);
        return t;
      }).toList(),
    );
  }

  void setTrackStepPattern(String trackId, List<bool> pattern) {
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(stepPattern: pattern);
        return t;
      }).toList(),
    );
  }

  void toggleTrackMute(String trackId) {
    _markDirty();
    final track = state.tracks.firstWhere((t) => t.id == trackId);
    final newMuted = !track.isMuted;
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(isMuted: newMuted);
        return t;
      }).toList(),
    );
    _syncVolumes();
    AppLogger.i('Track "${track.name}" ${newMuted ? "muted" : "unmuted"}');
  }

  void toggleTrackSolo(String trackId) {
    _markDirty();
    final track = state.tracks.firstWhere((t) => t.id == trackId);
    final newSolo = !track.isSolo;
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(isSolo: newSolo);
        return t;
      }).toList(),
    );
    _syncVolumes();
    AppLogger.i('Track "${track.name}" ${newSolo ? "solo" : "unsolo"}');
  }

  void _syncVolumes() {
    final audio = ref.read(audioServiceProvider);
    final hasSolo = state.hasSoloTrack;
    for (final t in state.tracks) {
      final effectiveVol = hasSolo
          ? (t.isSolo ? t.volume : 0.0)
          : (t.isMuted ? 0.0 : t.volume);
      audio.updateTrackVolume(t.id, effectiveVol);
    }
  }

  void setTrackAudioFile(String trackId, String filePath) {
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(audioFilePath: filePath);
        return t;
      }).toList(),
    );
  }

  void renameTrack(String trackId, String newName) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(name: newName);
        return t;
      }).toList(),
    );
  }

  void updateTrackNotes(String trackId, List<Note> notes) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(notes: notes);
        return t;
      }).toList(),
    );
  }

  void setTrackInstrument(String trackId, String instrumentName) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) {
          // Keep the track type in sync with the preset family.
          final preset = InstrumentPreset.fromIdOrNull(instrumentName);
          final isSynthPreset = preset != null &&
              (preset.category == InstrumentCategory.synth ||
                  preset.id.startsWith('syn_'));
          return t.copyWith(
            instrumentName: instrumentName,
            type: t.type == TrackType.audio
                ? t.type
                : (isSynthPreset ? TrackType.synth : TrackType.instrument),
          );
        }
        return t;
      }).toList(),
    );
  }

  /// null removes the compressor; a params instance sets/replaces it.
  void setTrackCompressor(String trackId, TrackCompressorParams? params) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(compressor: params);
        return t;
      }).toList(),
    );
  }

  void setTimeSignature(int numerator, int denominator) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(
      timeSignatureNumerator: numerator,
      timeSignatureDenominator: denominator,
    );
    AppLogger.i('Time signature: $numerator/$denominator');
  }

  void setKeySignature(String key) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(keySignature: key);
    AppLogger.i('Key signature: $key');
  }

  static const double referenceBpm = 120.0;

  void setBpm(double bpm) {
    _pushUndo();
    _markDirty();
    bpm = bpm.clamp(20, 300);
    final speed = bpm / referenceBpm;
    state = state.copyWith(bpm: bpm, playbackSpeed: speed);
    ref.read(audioServiceProvider).setPlaybackSpeed(speed);
    AppLogger.i('BPM: ${bpm.toStringAsFixed(1)} (speed: ${speed.toStringAsFixed(3)}x)');
  }

  void setPlaybackSpeed(double speed) {
    _pushUndo();
    _markDirty();
    speed = speed.clamp(0.25, 4.0);
    final bpm = speed * referenceBpm;
    state = state.copyWith(playbackSpeed: speed, bpm: bpm);
    ref.read(audioServiceProvider).setPlaybackSpeed(speed);
    AppLogger.i('Playback speed: ${speed.toStringAsFixed(2)}x (BPM: ${bpm.toStringAsFixed(1)})');
  }

  Future<void> forceNewProject() async {
    _pushUndo();
    _isDirty = false;
    stopAutoSave();
    await ref.read(audioServiceProvider).unloadAll();
    _currentFilePath = null;
    _lgdfInfo = null;
    state = Project(id: _uuid.v4(), name: 'Untitled');
    AppLogger.i('New project created');
  }

  Future<void> tryNewProject(BuildContext context) async {
    if (_isDirty) {
      final result = await showDialog<String>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: Text('proj.unsavedTitle'.tr()),
          content: Text('proj.unsavedMessage'.tr()),
          actions: [
            TextButton(onPressed: () => Navigator.of(ctx).pop('cancel'), child: Text('proj.cancel'.tr())),
            TextButton(onPressed: () => Navigator.of(ctx).pop('discard'), child: Text('proj.discard'.tr())),
            FilledButton(onPressed: () => Navigator.of(ctx).pop('save'), child: Text('proj.save'.tr())),
          ],
        ),
      );
      if (result == 'save') {
        await saveProject();
      } else if (result != 'discard') {
        return; // cancelled
      }
    }
    await forceNewProject();
  }

  List<Color> _trackColors() => [
    const Color(0xFF40C4FF),
    const Color(0xFF69F0AE),
    const Color(0xFFFFD740),
    const Color(0xFFFF8A65),
    const Color(0xFFCE93D8),
    const Color(0xFF4DB6AC),
    const Color(0xFFF06292),
    const Color(0xFFAED581),
  ];
}

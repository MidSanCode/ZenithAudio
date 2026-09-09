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
import '../services/project_serializer.dart';
import '../services/synth_engine.dart' show TrackCompressorParams;
import 'settings_provider.dart';

final projectProvider = NotifierProvider<ProjectNotifier, Project>(
  ProjectNotifier.new,
);

class ProjectNotifier extends Notifier<Project> {
  static const _uuid = Uuid();
  static const int _maxUndo = 50;

  /// Current save path for the project (set after first save or open).
  String? _currentFilePath;

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
      return true;
    }
    return result == 'discard';
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
      final audioBytes = <String, Uint8List>{};
      final serializer = ProjectSerializer();
      final bytes = await serializer.serialize(state, audioFileBytes: audioBytes);
      final dir = await getApplicationDocumentsDirectory();
      final autoDir = Directory('${dir.path}/.autosave');
      if (!await autoDir.exists()) await autoDir.create(recursive: true);
      final path = '${autoDir.path}/${state.id}.zap';
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
      final path = '${dir.path}/.autosave/$projectId.zap';
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
      final files = await autoDir.list().where((e) => e.path.endsWith('.zap')).toList();
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
        final serializer = ProjectSerializer();
        final serialized = await serializer.deserialize(bytes);
        if (serialized != null && context.mounted) {
          final notifier = ref.read(projectProvider.notifier);
          await notifier._loadSerialized(serialized);
        }
      }
    } catch (_) {}
  }

  Future<void> _loadSerialized(SerializedProject serialized) async {
    await ref.read(audioServiceProvider).unloadAll();
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
      final path = '${dir.path}/.autosave/${state.id}.zap';
      final file = File(path);
      if (await file.exists()) await file.delete();
    } catch (_) {}
  }

  // ──── Save / Open ────

  Future<void> saveProject() async {
    try {
      AppLogger.i('Saving project...');

      // Serialize FIRST (before file picker on mobile)
      final audioBytes = <String, Uint8List>{};
      final serializer = ProjectSerializer();
      final bytes = await serializer.serialize(state, audioFileBytes: audioBytes);

      if (kIsWeb) {
        final webSerializer = ProjectSerializer();
        webSerializer.downloadArchive(bytes, '${state.name}${AppConstants.projectExtension}');
        AppLogger.i('Project saved via browser download');
        return;
      }

      // Resolve output path
      String? outputPath;
      if (_currentFilePath != null && await File(_currentFilePath!).exists()) {
        outputPath = _currentFilePath;
      } else {
        try {
          if (Platform.isAndroid || Platform.isIOS) {
            // On Android/iOS, pass bytes directly so FilePicker writes via ContentResolver
            outputPath = await FilePicker.platform.saveFile(
              dialogTitle: 'menu.file.saveProject'.tr(),
              fileName: '${state.name}${AppConstants.projectExtension}',
              type: FileType.custom,
              allowedExtensions: ['zap'],
              bytes: bytes,
            );
          } else {
            outputPath = await FilePicker.platform.saveFile(
              dialogTitle: 'menu.file.saveProject'.tr(),
              fileName: '${state.name}${AppConstants.projectExtension}',
              type: FileType.custom,
              allowedExtensions: ['zap'],
            );
          }
        } catch (e) {
          AppLogger.e('File picker error', e);
          return;
        }
      }

      if (outputPath == null) return; // User cancelled

      if (Platform.isIOS) {
        // On iOS with bytes param, file is already written by the picker.
        // If the path is somehow unwritable, fall back to app docs.
        if (!await File(outputPath).exists()) {
          try {
            await File(outputPath).writeAsBytes(bytes);
          } catch (e) {
            AppLogger.w('iOS save to picker path failed, using app docs: $e');
            final dir = await getApplicationDocumentsDirectory();
            outputPath = '${dir.path}/${state.name}${AppConstants.projectExtension}';
            await File(outputPath).writeAsBytes(bytes);
          }
        }
      } else if (!Platform.isAndroid) {
        // Desktop: write directly
        await File(outputPath).writeAsBytes(bytes);
      }
      // On Android/iOS with bytes param, file is already written by the picker

      _currentFilePath = outputPath;
      _isDirty = false;
      clearAutoSaveCache();
      AppLogger.i('Project saved to: $outputPath');
    } catch (e) {
      AppLogger.e('Failed to save project', e);
    }
  }

  Future<void> openProject() async {
    _pushUndo();
    _isDirty = false;
    stopAutoSave();
    try {
      AppLogger.i('Opening project...');

      final result = await FilePicker.platform.pickFiles(
        type: FileType.custom,
        allowedExtensions: ['zap'],
      );

      if (result == null || result.files.isEmpty) return;

      final file = result.files.single;

      Uint8List bytes;
      if (kIsWeb) {
        bytes = file.bytes!;
      } else if (file.path != null) {
        bytes = await File(file.path!).readAsBytes();
        _currentFilePath = file.path;
      } else {
        return;
      }

      final serializer = ProjectSerializer();
      final serialized = await serializer.deserialize(bytes);
      if (serialized == null) {
        AppLogger.e('Failed to deserialize project');
        return;
      }

      final audioService = ref.read(audioServiceProvider);
      await audioService.unloadAll();

      final updatedTracks = serialized.project.tracks.map((t) {
        if (t.type == TrackType.audio) {
          final audioPath = serialized.trackAudioFiles[t.id];
          return audioPath != null ? t.copyWith(audioFilePath: audioPath) : t;
        }
        return t;
      }).toList();
      state = serialized.project.copyWith(tracks: updatedTracks);

      for (final track in state.tracks) {
        if (track.type == TrackType.audio && track.audioFilePath != null) {
          audioService.loadTrack(track).then((dur) {
            final updated = track.copyWith(duration: dur);
            state = state.copyWith(
              tracks: state.tracks.map((t) => t.id == track.id ? updated : t).toList(),
            );
          });
        }
      }

      AppLogger.i('Project loaded: ${state.name}');
    } catch (e) {
      AppLogger.e('Failed to open project', e);
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

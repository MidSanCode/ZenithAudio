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
import '../engine/audio_engine_adapter.dart';
import '../services/lgdf_format.dart';
import '../services/project_serializer.dart';
import '../services/workspace_service.dart';
import 'workspace_provider.dart';
import '../services/synth_engine.dart' show TrackCompressorParams;
import 'settings_provider.dart';

part 'project_undo.dart';
part 'project_io.dart';

final projectProvider = NotifierProvider<ProjectNotifier, Project>(
  ProjectNotifier.new,
);

class ProjectNotifier extends Notifier<Project> with _ProjectHistoryMixin {
  static const _uuid = Uuid();

  /// Current project directory inside the workspace (set after first save or
  /// when an LGDF directory project is opened).
  String? _currentFilePath;

  /// LGDF identity of the loaded project, so a re-save keeps its created time
  /// and version.
  LgdfProjectInfo? _lgdfInfo;

  /// Tracks whether there are unsaved changes.
  bool _isDirty = false;

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
      ref.read(audioEngineProvider).disposeService();
      _autoSaveTimer?.cancel();
    });
    WidgetsBinding.instance.addPostFrameCallback((_) => startAutoSave());
    return Project(id: _uuid.v4(), name: 'untitled');
  }

  void _markDirty() => _isDirty = true;

  /// Check for an auto-save cache on startup and offer recovery.
  ///
  /// Lives on the class (not the IO part) because callers reach it as
  /// `ProjectNotifier.checkForAutoSaveRecovery(...)`.
  static Future<void> checkForAutoSaveRecovery(
      BuildContext context, WidgetRef ref) async {
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
          content: Text(
              'proj.recoveryMessage'.tr(namedArgs: {'n': '${files.length}'})),
          actions: [
            TextButton(
                onPressed: () => Navigator.of(ctx).pop(false),
                child: Text('proj.noRestore'.tr())),
            FilledButton(
                onPressed: () => Navigator.of(ctx).pop(true),
                child: Text('proj.restore'.tr())),
          ],
        ),
      );
      if (recover == true && files.isNotEmpty && context.mounted) {
        // Open the most recent auto-save.
        final newest = files.reduce((a, b) => File(a.path)
                .statSync()
                .modified
                .isAfter(File(b.path).statSync().modified)
            ? a
            : b);
        final bytes = await File(newest.path).readAsBytes();
        final serialized = await const ProjectSerializer().deserialize(bytes);
        if (serialized != null && context.mounted) {
          await ref
              .read(projectProvider.notifier)
              .loadSerializedProject(serialized);
        }
      }
    } catch (_) {}
  }


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
      ref.read(audioEngineProvider).loadTrack(track).then((dur) {
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
    await ref.read(audioEngineProvider).unloadTrack(trackId);
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
    ref.read(audioEngineProvider).updateTrackVolume(trackId, volume);
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
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(isMuted: !t.isMuted);
        return t;
      }).toList(),
    );
    ref.read(audioEngineProvider).setMute(trackId, !track.isMuted);
  }

  void toggleTrackSolo(String trackId) {
    _markDirty();
    final track = state.tracks.firstWhere((t) => t.id == trackId);
    final soloing = !track.isSolo;
    final updatedTracks = state.tracks.map((t) {
      if (t.id == trackId) return t.copyWith(isSolo: soloing);
      // Exclusive solo keeps the mix predictable.
      return soloing ? t.copyWith(isSolo: false) : t;
    }).toList();
    state = state.copyWith(tracks: updatedTracks);
    _syncVolumes();
  }

  void _syncVolumes() {
    final audio = ref.read(audioEngineProvider);
    for (final t in state.tracks) {
      audio.setMute(t.id, !state.shouldTrackPlay(t));
    }
  }

  void setTrackAudioFile(String trackId, String filePath) {
    _pushUndo();
    _markDirty();
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id == trackId) return t.copyWith(audioFilePath: filePath);
        return t;
      }).toList(),
    );
    AppLogger.i('Track $trackId audio file set');
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
    // The track type follows the preset family so synth presets keep their
    // own type and are classified correctly in the mixer/UI.
    final preset = InstrumentPreset.fromIdOrNull(instrumentName);
    final isSynthPreset = preset != null &&
        (preset.category == InstrumentCategory.synth ||
            preset.id.startsWith('syn_'));
    state = state.copyWith(
      tracks: state.tracks.map((t) {
        if (t.id != trackId) return t;
        return t.copyWith(
          instrumentName: instrumentName,
          type: isSynthPreset ? TrackType.synth : TrackType.instrument,
        );
      }).toList(),
    );
    AppLogger.i('Track $trackId instrument: $instrumentName');
  }

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
    _markDirty();
    state = state.copyWith(
      timeSignatureNumerator: numerator,
      timeSignatureDenominator: denominator,
    );
  }

  void setKeySignature(String key) {
    _markDirty();
    state = state.copyWith(keySignature: key);
  }

  /// Tempo used as the reference when comparing/restoring the project BPM.
  static const double referenceBpm = 120.0;

  void setBpm(double bpm) {
    _markDirty();
    final clamped = bpm.clamp(20.0, 300.0);
    // The seconds view of every note was derived at the old tempo; re-derive
    // it so the score keeps its musical position.
    final retimed = state.tracks.map((t) {
      if (!t.isInstrument || t.notes.isEmpty) return t;
      return t.copyWith(
        notes: t.notes.map((n) => n.withTempo(clamped)).toList(),
      );
    }).toList();
    state = state.copyWith(bpm: clamped, tracks: retimed);
  }

  void setPlaybackSpeed(double speed) {
    _markDirty();
    state = state.copyWith(playbackSpeed: speed.clamp(0.25, 4.0));
    ref.read(audioEngineProvider).setPlaybackSpeed(state.playbackSpeed);
  }

  Future<void> forceNewProject() async {
    stopAutoSave();
    _currentFilePath = null;
    _lgdfInfo = null;
    _clearHistory();
    await ref.read(audioEngineProvider).unloadAll();
    state = Project(id: _uuid.v4(), name: 'untitled');
    _isDirty = false;
    startAutoSave();
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

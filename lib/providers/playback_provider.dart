import 'dart:async';
import 'dart:io';
import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../models/track.dart';
import '../services/audio_service.dart';
import 'project_provider.dart';
import 'settings_provider.dart';

enum PlaybackState { stopped, playing, paused }

final playbackProvider = NotifierProvider<PlaybackNotifier, PlaybackState>(
  PlaybackNotifier.new,
);

final playheadPositionProvider = StateProvider<double>((ref) => 0);

final currentStepProvider = StateProvider<int>((ref) => -1);

final masterVolumeProvider = StateProvider<double>((ref) => 0.8);

final pixelsPerSecondProvider = StateProvider<double>((ref) => 50.0);

/// WAV generation progress (0.0 – 1.0). Resets to 0 on each play().
final wavGenerationProgressProvider = StateProvider<double>((ref) => 0.0);

class PlaybackNotifier extends Notifier<PlaybackState> {
  @override
  PlaybackState build() {
    final audio = ref.read(audioServiceProvider);
    audio.onPositionChanged = (pos) {
      ref.read(playheadPositionProvider.notifier).state = pos;
      final project = ref.read(projectProvider);
      final bpm = project.bpm;
      final stepsPerBeat = 4;
      final secondsPerStep = 60.0 / bpm / stepsPerBeat;
      final step = (pos / secondsPerStep).floor() % 16;
      ref.read(currentStepProvider.notifier).state = step;
    };
    audio.onCompleted = () {
      final settings = ref.read(settingsProvider);
      if (settings.autoLoop) {
        _restart();
      } else {
        state = PlaybackState.stopped;
        ref.read(playheadPositionProvider.notifier).state = 0;
      }
    };

    // Live editing: while the transport is rolling, re-render any instrument
    // track whose notes/params changed and hot-swap its WAV so the newly
    // drawn notes are heard immediately.
    List<Track> lastTracks = ref.read(projectProvider).tracks;
    ref.listen(projectProvider, (_, next) {      final prev = lastTracks;
      lastTracks = next.tracks;
      final wasPlaying = state == PlaybackState.playing && audio.isPlaying;
      if (!wasPlaying || prev == null) return;
      for (final t in next.tracks) {
        if (!t.isInstrument) continue;
        final before = prev.where((p) => p.id == t.id).firstOrNull;
        if (before == null) continue; // newly added during playback
        final changed = before.instrumentName != t.instrumentName ||
            !listEquals(before.notes, t.notes) ||
            before.compressor != t.compressor;
        if (!changed) continue;
        if (t.notes.isEmpty) {
          // All notes removed: silence this track right away.
          _swapTimers[t.id]?.cancel();
          _swapTimers.remove(t.id);
          _pendingSwap.remove(t.id);
          ref.read(audioServiceProvider).stopAndUnloadTrack(t.id);
          continue;
        }
        // Debounce rapid drag edits; the last state after the window wins.
        _pendingSwap[t.id] = t;
        _scheduleHotSwap(t.id);
      }
    });

    // Cancel pending hot-swap timers when the notifier goes away.
    ref.onDispose(() {
      for (final t in _swapTimers.values) {
        t.cancel();
      }
      _swapTimers.clear();
      _pendingSwap.clear();
    });

    return PlaybackState.stopped;
  }

  final Map<String, Timer> _swapTimers = {};
  final Map<String, Track> _pendingSwap = {};

  void _scheduleHotSwap(String trackId) {
    _swapTimers[trackId]?.cancel();
    _swapTimers[trackId] = Timer(const Duration(milliseconds: 250), () async {
      _swapTimers.remove(trackId);
      final track = _pendingSwap.remove(trackId);
      if (track == null) return;
      if (state != PlaybackState.playing) return;
      try {
        await ref.read(audioServiceProvider).hotSwapTrackWav(track);
      } catch (_) {}
    });
  }

  Future<void> _restart() async {
    await ref.read(audioServiceProvider).seekTo(0);
    ref.read(playheadPositionProvider.notifier).state = 0;
    await ref.read(audioServiceProvider).play();
    state = PlaybackState.playing;
  }

  /// Start playback of all tracks.
  /// [editingTrackId] — if set, the editing instrument track's WAV is
  /// generated on a background isolate (non-blocking). Other instrument
  /// tracks show progress while generating.
  Future<void> play({String? editingTrackId}) async {
    final audio = ref.read(audioServiceProvider);
    final project = ref.read(projectProvider);

    await audio.unloadAll();
    ref.read(wavGenerationProgressProvider.notifier).state = 0.0;

    // 1. Load audio tracks immediately (no WAV gen needed)
    for (final track in project.tracks) {
      if (track.type == TrackType.audio) {
        if (track.audioFilePath != null && File(track.audioFilePath!).existsSync()) {
          await audio.loadTrackFromPath(
            track.id, track.audioFilePath!,
            volume: track.volume,
            muted: track.isMuted,
          );
        }
      }
    }

    // 2. Determine effective volume for each track (solo/mute)
    double effectiveVolume(Track t) {
      final hasSolo = project.hasSoloTrack;
      if (hasSolo) return t.isSolo ? t.volume : 0.0;
      return t.isMuted ? 0.0 : t.volume;
    }

    // 3. Prepare instrument tracks
    final instTracks = project.tracks
        .where((t) => t.isInstrument &&
            t.instrumentName != null && t.notes.isNotEmpty)
        .toList();

    // Separate editing track from others
    final editingTrack = editingTrackId != null
        ? instTracks.where((t) => t.id == editingTrackId).firstOrNull
        : null;
    final otherTracks = instTracks.where((t) => t.id != editingTrackId).toList();

    // Prepare editing track WAV on background isolate (non-blocking)
    final Future<String?> editingFuture;
    if (editingTrack != null) {
      editingFuture = audio.prepareInstrumentTrack(editingTrack, useIsolate: true);
    } else {
      editingFuture = Future.value(null);
    }

    // Prepare other tracks with progress
    int done = 0;
    final total = otherTracks.length;
    for (final track in otherTracks) {
      final path = await audio.prepareInstrumentTrack(track);
      if (path != null) {
        final vol = effectiveVolume(track);
        await audio.loadTrackFromPath(track.id, path, volume: vol, muted: vol == 0);
      }
      done++;
      ref.read(wavGenerationProgressProvider.notifier).state =
          total > 0 ? done / total : 1.0;
    }

    // Wait for editing track's WAV
    final editingPath = await editingFuture;
    if (editingPath != null && editingTrack != null) {
      final vol = effectiveVolume(editingTrack);
      await audio.loadTrackFromPath(editingTrack.id, editingPath, volume: vol, muted: vol == 0);
    }

    // 4. Ensure all loaded tracks respect solo/mute
    for (final t in project.tracks) {
      final vol = effectiveVolume(t);
      audio.updateTrackVolume(t.id, vol);
    }

    ref.read(wavGenerationProgressProvider.notifier).state = 1.0;
    audio.setPlaybackSpeed(project.playbackSpeed);
    await audio.play();
    state = PlaybackState.playing;
  }

  Future<void> pause() async {
    await ref.read(audioServiceProvider).pause();
    state = PlaybackState.paused;
  }

  Future<void> stop() async {
    await ref.read(audioServiceProvider).stop();
    ref.read(playheadPositionProvider.notifier).state = 0;
    state = PlaybackState.stopped;
  }

  Future<void> toggle({String? editingTrackId}) async {
    if (state == PlaybackState.playing) {
      await pause();
    } else {
      await play(editingTrackId: editingTrackId);
    }
  }

  Future<void> seekTo(double seconds) async {
    await ref.read(audioServiceProvider).seekTo(seconds);
    ref.read(playheadPositionProvider.notifier).state = seconds;
  }
}

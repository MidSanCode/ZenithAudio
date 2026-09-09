import 'dart:async';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';
import 'package:media_kit/media_kit.dart' hide Track;
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:path_provider/path_provider.dart';
import '../models/track.dart';
import '../models/instrument.dart';
import '../models/envelope.dart';
import '../models/note.dart';
import '../core/utils/logger.dart';
import 'synth_engine.dart';
import 'soundfont_service.dart';
import 'soundfont_parser.dart' show SoundFontBank;

final audioServiceProvider = Provider<AudioService>((ref) {
  final service = AudioService();
  ref.onDispose(() => service.dispose());
  return service;
});

/// Snapshot of the live audio pipeline for the song-info panel.
class AudioOutputInfo {
  final int? sampleRate;
  final int? channels;
  final String? sampleFormat;
  final double? bitrateKbps;
  final String deviceName;
  final String deviceDescription;
  final List<(String, String)> availableDevices;

  const AudioOutputInfo({
    this.sampleRate,
    this.channels,
    this.sampleFormat,
    this.bitrateKbps,
    required this.deviceName,
    required this.deviceDescription,
    this.availableDevices = const [],
  });
}

class _TrackPlayer {
  final Player player;
  StreamSubscription? completedSub;
  StreamSubscription? positionSub;
  StreamSubscription? durationSub;
  bool _disposed = false;
  bool reachedEnd = false;
  double trackVolume = 1.0;

  _TrackPlayer(this.player);

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    completedSub?.cancel();
    positionSub?.cancel();
    durationSub?.cancel();
    completedSub = null;
    positionSub = null;
    durationSub = null;
    player.stop();
    player.dispose();
  }
}

/// Tracks which instrument WAVs are cached and their note hash.
class _WavCache {
  final String path;
  final int noteHash;
  _WavCache(this.path, this.noteHash);
}

class AudioService {
  final Map<String, _TrackPlayer> _players = {};
  final Map<String, _WavCache> _wavCache = {};
  final Set<String> _pendingDelete = {};
  bool _isPlaying = false;
  double _masterVolume = 1.0;
  double _playbackSpeed = 1.0;

  void Function(double position)? onPositionChanged;
  void Function()? onCompleted;

  bool get isPlaying => _isPlaying;
  double get masterVolume => _masterVolume;

  set masterVolume(double v) {
    _masterVolume = v.clamp(0.0, 1.0);
    for (final tp in _players.values) {
      tp.player.setVolume((tp.trackVolume * _masterVolume * 100).roundToDouble());
    }
  }

  DateTime _lastPositionUpdate = DateTime.now();
  static const Duration _positionThrottle = Duration(milliseconds: 33);

  Future<double> loadTrack(Track track) async {
    if (track.audioFilePath == null) return 0;

    final player = Player();
    final tp = _TrackPlayer(player);
    try {
      final uri = Uri.file(track.audioFilePath!);
      await player.open(Media(uri.toString()), play: false);

      final vol = (track.volume * _masterVolume * 100).roundToDouble();
      await player.setVolume(vol);
      await player.setRate(_playbackSpeed);

      _players[track.id] = tp;
      tp.trackVolume = track.volume;

      tp.completedSub = player.stream.completed.listen((completed) {
        if (tp._disposed) return;
        if (completed) {
          tp.reachedEnd = true;
          if (_players.values.every((p) => p._disposed || p.reachedEnd)) {
            _isPlaying = false;
            onCompleted?.call();
          }
        }
      });

      tp.positionSub = player.stream.position.listen((position) {
        if (tp._disposed) return;
        final now = DateTime.now();
        if (now.difference(_lastPositionUpdate) < _positionThrottle) return;
        _lastPositionUpdate = now;
        onPositionChanged?.call(position.inMilliseconds / 1000.0);
      });

      double dur = player.state.duration.inMilliseconds / 1000.0;
      if (dur <= 0) {
        try {
          dur = await player.stream.duration
              .firstWhere((d) => d > Duration.zero,
                  orElse: () => Duration.zero)
              .timeout(const Duration(seconds: 5),
                  onTimeout: () => Duration.zero)
              .then((d) => d.inMilliseconds / 1000.0);
        } catch (_) {
          dur = 0;
        }
      }
      AppLogger.d('loadTrack: ${dur.toStringAsFixed(2)}s');
      return dur;
    } catch (e) {
      tp.dispose();
      _players.remove(track.id);
      return 0;
    }
  }

  /// Get or generate a WAV for an instrument track. Returns the file path.
  /// Returns null if track has no instrument or no notes.
  /// If [useIsolate] is true, synthesis runs on a background isolate.
  Future<String?> prepareInstrumentTrack(Track track,
      {bool useIsolate = false}) async {
    if (track.type == TrackType.audio) return track.audioFilePath;
    if (track.instrumentName == null || track.notes.isEmpty) return null;

    final compKey = track.compressor == null
        ? 'off'
        : track.compressor!.toJson().entries
            .map((e) => '${e.key}=${e.value}')
            .join(',');
    final noteHash = Object.hash(track.instrumentName, Object.hashAll(track.notes), compKey);
    final cached = _wavCache[track.id];
    if (cached != null && cached.noteHash == noteHash) {
      return cached.path;
    }

    const maxDur = 120.0;
    final dur = track.computedDuration > 0
        ? (track.computedDuration + 0.5).clamp(0.0, maxDur).toDouble()
        : 2.0;
    final sampleRate = 44100;

    final Uint8List wav;
    if (useIsolate) {
      final params = _jobParams(track, dur, sampleRate);
      final bank = SoundFontService.instance.bank;
      wav = await Isolate.run(() => _synthAndEncodeWav(params, bank));
    } else {
      wav = _synthAndEncodeWav(_jobParams(track, dur, sampleRate),
          SoundFontService.instance.bank);
    }

    final dir = await getTemporaryDirectory();
    // Unique-per-hash file name: the previous WAV may still be open by the
    // player while a hot-swap re-render happens (Windows sharing violation).
    final filePath = '${dir.path}/synth_${track.id}_${noteHash.abs()}.wav';
    await File(filePath).writeAsBytes(wav);

    final oldPath = _wavCache[track.id]?.path;
    _wavCache[track.id] = _WavCache(filePath, noteHash);
    if (oldPath != null && oldPath != filePath) {
      if (_players.containsKey(track.id)) {
        // Still playing the old file — delete it after the hot swap.
        _pendingDelete.add(oldPath);
      } else {
        try {
          final f = File(oldPath);
          if (f.existsSync()) f.deleteSync();
        } catch (_) {}
      }
    }
    return filePath;
  }

  /// Serialize a track into the parameter map consumed by
  /// [_synthAndEncodeWav] (isolate-safe: plain JSON types only).
  Map<String, dynamic> _jobParams(Track track, double dur, int sampleRate) {
    final inst = InstrumentPreset.fromId(track.instrumentName!);
    return <String, dynamic>{
      'notes': track.notes.map((n) => {
        'startTime': n.startTime,
        'duration': n.duration,
        'pitch': n.pitch,
        'velocity': n.velocity,
      }).toList(),
      'instrument': _presetToMap(inst),
      'duration': dur,
      'sampleRate': sampleRate,
      'compressor': track.compressor?.enabled == true
          ? track.compressor!.toJson()
          : null,
    };
  }

  /// Flatten an InstrumentPreset into a plain map so the isolate does not
  /// depend on Flutter icon constants.
  static Map<String, dynamic> _presetToMap(InstrumentPreset p) => <String, dynamic>{
    'id': p.id,
    'programNumber': p.programNumber,
    'harmonics': p.harmonics,
    'attack': p.attack, 'decay': p.decay, 'sustain': p.sustain,
    'release': p.release, 'detuneCents': p.detuneCents,
    'noiseAttack': p.noiseAttack, 'brightnessFactor': p.brightnessFactor,
    'synthEngine': p.synthEngine,
    'envCurve': p.envCurve?.toJson(),
    'filterType': p.filterType, 'filterCutoff': p.filterCutoff,
    'filterResonance': p.filterResonance, 'filterEnvAmount': p.filterEnvAmount,
    'filterAttack': p.filterAttack, 'filterDecay': p.filterDecay,
    'filterSustain': p.filterSustain,
    'fmRatio': p.fmRatio, 'fmIndex': p.fmIndex, 'fmDecay': p.fmDecay,
    'fmFeedback': p.fmFeedback, 'morphRate': p.morphRate,
  };

  /// Prepare all given tracks (generate WAVs for instrument tracks if needed).
  /// Returns a stream of progress (0.0 – 1.0).
  Stream<double> prepareTracks(List<Track> tracks,
      {String? skipTrackId, bool useIsolate = false}) async* {
    final targets = tracks.where((t) =>
        t.isInstrument &&
        t.id != skipTrackId &&
        t.instrumentName != null &&
        t.notes.isNotEmpty);

    int done = 0;
    final total = targets.length;
    if (total == 0) {
      yield 1.0;
      return;
    }

    for (final track in targets) {
      await prepareInstrumentTrack(track, useIsolate: useIsolate);
      done++;
      yield done / total;
    }
  }

  /// Load all given tracks into players and volume them according to
  /// their settings and master volume.
  Future<void> loadTracks(List<Track> tracks, {String? skipTrackId}) async {
    await unloadAll();
    for (final track in tracks) {
      if (track.id == skipTrackId) continue;
      final path = track.type == TrackType.audio
          ? track.audioFilePath
          : _wavCache[track.id]?.path;
      if (path == null || path.isEmpty) continue;
      if (!File(path).existsSync()) continue;

      final player = Player();
      final tp = _TrackPlayer(player);
      try {
        await player.open(Media(Uri.file(path).toString()), play: false);
        final vol = (track.volume * _masterVolume * 100).roundToDouble();
        await player.setVolume(track.isMuted ? 0 : vol);
        await player.setRate(_playbackSpeed);
        _players[track.id] = tp;
        tp.trackVolume = track.volume;
      } catch (e) {
        tp.dispose();
      }
    }
  }

  /// Load a single track from a file path with given volume/mute.
  Future<void> loadTrackFromPath(String trackId, String path,
      {double volume = 1.0, bool muted = false}) async {
    await unloadTrack(trackId);
    final player = Player();
    final tp = _TrackPlayer(player);
    try {
      await player.open(Media(Uri.file(path).toString()), play: false);
      await player.setVolume(muted ? 0 : (volume * _masterVolume * 100).roundToDouble());
      await player.setRate(_playbackSpeed);
      _players[trackId] = tp;
      tp.trackVolume = volume;

      tp.completedSub = player.stream.completed.listen((completed) {
        if (tp._disposed) return;
        if (completed) {
          tp.reachedEnd = true;
          if (_players.values.every((p) => p._disposed || p.reachedEnd)) {
            _isPlaying = false;
            onCompleted?.call();
          }
        }
      });

      tp.positionSub = player.stream.position.listen((position) {
        if (tp._disposed) return;
        final now = DateTime.now();
        if (now.difference(_lastPositionUpdate) < _positionThrottle) return;
        _lastPositionUpdate = now;
        onPositionChanged?.call(position.inMilliseconds / 1000.0);
      });
    } catch (e) {
      tp.dispose();
    }
  }

  /// Returns the cached WAV path for a track, or null if not cached.
  String? getCachedTrackPath(String trackId) => _wavCache[trackId]?.path;

  /// Snapshot of the active audio output (device, params, bitrate) from the
  /// first live player. Returns null when nothing is loaded yet.
  AudioOutputInfo? getOutputInfo() {
    final tp = _players.values
        .where((p) => !p._disposed && p.player.state.playlist.medias.isNotEmpty)
        .firstOrNull;
    if (tp == null) return null;
    final st = tp.player.state;
    final params = st.audioParams;
    final dev = st.audioDevice;
    final desc = dev.name == 'auto'
        ? st.audioDevices
                .where((d) => d.name == 'auto')
                .map((d) => d.description)
                .firstOrNull ??
            dev.description
        : st.audioDevices
                .where((d) => d.name == dev.name)
                .map((d) => d.description)
                .firstOrNull ??
            dev.description;
    return AudioOutputInfo(
      sampleRate: params.sampleRate,
      channels: params.channelCount,
      sampleFormat: params.format,
      bitrateKbps: st.audioBitrate,
      deviceName: dev.name,
      deviceDescription: desc.isEmpty ? dev.name : desc,
      availableDevices: st.audioDevices
          .map((d) => (d.name, d.description.isEmpty ? d.name : d.description))
          .toList(),
    );
  }

  /// Check if track WAV is cached with current notes.
  bool isTrackCached(Track track) {
    if (track.type == TrackType.audio) return track.audioFilePath != null;
    final cached = _wavCache[track.id];
    if (cached == null) return false;
    final compKey = track.compressor == null
        ? 'off'
        : track.compressor!.toJson().entries
            .map((e) => '${e.key}=${e.value}')
            .join(',');
    final noteHash = Object.hash(track.instrumentName, Object.hashAll(track.notes), compKey);
    return cached.noteHash == noteHash;
  }

  /// Hot-swap a playing track's WAV without touching other tracks.
  ///
  /// Used when the user edits notes while the transport is rolling: the new
  /// WAV replaces the old one at the current playhead so freshly drawn notes
  /// are heard immediately. No-op when paused/stopped.
  Future<void> hotSwapTrackWav(Track track) async {
    if (!_isPlaying) return;
    if (track.isInstrument == false || track.instrumentName == null ||
        track.notes.isEmpty) {
      return;
    }

    // 1. Render the new WAV (cache entry updated by prepareInstrumentTrack).
    final newPath = await prepareInstrumentTrack(track);
    if (newPath == null) return;

    // 2. Capture the old player's position, volume and solo/mute before
    //    tearing it down, then open the new media at that offset.
    final tp = _players[track.id];
    final pos = tp?.player.state.position ?? Duration.zero;
    final volume = tp?.trackVolume ?? track.volume;
    final muted = track.isMuted || volume <= 0;

    // 3. Reuse loadTrackFromPath (handles subscriptions/counters), then seek
    //    the fresh player to the previous position and resume.
    await loadTrackFromPath(track.id, newPath, volume: volume, muted: muted);
    final fresh = _players[track.id];
    if (fresh != null && !fresh._disposed) {
      if (pos > Duration.zero) await fresh.player.seek(pos);
      fresh.player.play();
    }
  }

  void updateTrackVolume(String trackId, double volume) {
    final tp = _players[trackId];
    if (tp != null) {
      tp.trackVolume = volume;
      tp.player.setVolume((volume * _masterVolume * 100).roundToDouble());
    }
  }

  void setMute(String trackId, bool muted) {
    final tp = _players[trackId];
    if (tp != null) {
      tp.player.setVolume(muted ? 0 : (tp.trackVolume * _masterVolume * 100).roundToDouble());
    }
  }

  void setPlaybackSpeed(double speed) {
    _playbackSpeed = speed;
    for (final tp in _players.values) {
      tp.player.setRate(speed);
    }
  }

  void updateMasterVolume(double volume) {
    _masterVolume = volume.clamp(0.0, 1.0);
    for (final tp in _players.values) {
      tp.player.setVolume((tp.trackVolume * _masterVolume * 100).roundToDouble());
    }
  }

  Future<void> play() async {
    if (_players.isEmpty) return;
    await _cleanupPendingDeletes();
    _isPlaying = true;
    for (final tp in _players.values) {
      tp.reachedEnd = false;
      if (!tp._disposed) tp.player.play();
    }
  }

  /// Play a single track without affecting other tracks' state.
  Future<void> playSingleTrack(String trackId) async {
    final tp = _players[trackId];
    if (tp != null && !tp._disposed) {
      tp.player.play();
    }
  }

  /// Stop and unload a single track.
  Future<void> stopAndUnloadTrack(String trackId) async {
    final tp = _players[trackId];
    if (tp != null && !tp._disposed) {
      tp.player.stop();
    }
    await unloadTrack(trackId);
  }

  Future<void> pause() async {
    _isPlaying = false;
    for (final tp in _players.values) {
      if (!tp._disposed) tp.player.pause();
    }
    await _cleanupPendingDeletes();
  }

  Future<void> stop() async {
    _isPlaying = false;
    for (final tp in _players.values) {
      if (!tp._disposed) tp.player.stop();
    }
    await _cleanupPendingDeletes();
  }

  Future<void> seekTo(double seconds) async {
    final duration = Duration(milliseconds: (seconds * 1000).round());
    for (final tp in _players.values) {
      if (!tp._disposed) tp.player.seek(duration);
    }
  }

  Future<void> unloadTrack(String trackId) async {
    final tp = _players.remove(trackId);
    tp?.dispose();
  }

  Future<void> unloadAll() async {
    for (final tp in _players.values) {
      tp.dispose();
    }
    _players.clear();
    _isPlaying = false;
  }

  /// Delete temp WAVs queued while a playing player still held them open.
  Future<void> _cleanupPendingDeletes() async {
    if (_pendingDelete.isEmpty) return;
    final doomed = List<String>.from(_pendingDelete);
    _pendingDelete.clear();
    for (final path in doomed) {
      try {
        final f = File(path);
        if (f.existsSync()) f.deleteSync();
      } catch (_) {}
    }
  }

  Future<void> dispose() async {
    await unloadAll();
    _wavCache.clear();
  }
}

// ── Top-level WAV synthesis (usable with Flutter.compute) ──

Uint8List _encodeWav(Float64List buffer, int numSamples, int sampleRate) {
  final bytesPerSample = 2;
  final dataSize = numSamples * bytesPerSample;
  final fileSize = 44 + dataSize;
  final result = _DataWriter(fileSize);
  result.writeString('RIFF');
  result.writeInt32(fileSize - 8);
  result.writeString('WAVE');
  result.writeString('fmt ');
  result.writeInt32(16);
  result.writeInt16(1);
  result.writeInt16(1);
  result.writeInt32(sampleRate);
  result.writeInt32(sampleRate * bytesPerSample);
  result.writeInt16(bytesPerSample);
  result.writeInt16(16);
  result.writeString('data');
  result.writeInt32(dataSize);
  for (int i = 0; i < numSamples; i++) {
    final clamped = buffer[i].clamp(-1.0, 1.0);
    final sample = (clamped * 32767).round().clamp(-32768, 32767);
    result.writeInt16(sample);
  }
  return result.bytes;
}

/// Top-level synth + encode function for use with [Isolate.run].
/// [params] comes from [_jobParams]: plain-JSON instrument map.
/// [bank] is the (sendable) SoundFont bank, or null.
Uint8List _synthAndEncodeWav(Map<String, dynamic> params,
    [SoundFontBank? bank]) {
  final notesData = params['notes'] as List<dynamic>;
  final instMap = params['instrument'] as Map<String, dynamic>;
  final duration = (params['duration'] as num).toDouble();
  final sampleRate = params['sampleRate'] as int;
  final compJson = params['compressor'] as Map<String, dynamic>?;

  final inst = _presetFromMap(instMap);
  final notes = notesData.map((nd) {
    final m = nd as Map<String, dynamic>;
    return Note(
      startTime: (m['startTime'] as num).toDouble(),
      duration: (m['duration'] as num).toDouble(),
      pitch: m['pitch'] as int,
      velocity: m['velocity'] as int,
    );
  }).toList();

  final buffer = renderNoteList(SynthRenderJob(
    notes: notes,
    instrument: inst,
    totalDuration: duration,
    sampleRate: sampleRate,
    bank: SoundFontService.instance.bank,
    compressor: compJson != null
        ? TrackCompressorParams.fromJson(compJson)
        : null,
  ));

  return _encodeWav(buffer, buffer.length, sampleRate);
}

/// Rebuild an InstrumentPreset from the flattened map (no icon dependency).
InstrumentPreset _presetFromMap(Map<String, dynamic> m) => InstrumentPreset(
  id: m['id'] as String? ?? 'track',
  name: m['id'] as String? ?? 'track',
  programNumber: m['programNumber'] as int? ?? 0,
  harmonics: ((m['harmonics'] as List?) ?? const [1.0])
      .map((e) => (e as num).toDouble()).toList(),
  attack: (m['attack'] as num?)?.toDouble() ?? 0.01,
  decay: (m['decay'] as num?)?.toDouble() ?? 0.2,
  sustain: (m['sustain'] as num?)?.toDouble() ?? 0.7,
  release: (m['release'] as num?)?.toDouble() ?? 0.1,
  detuneCents: (m['detuneCents'] as num?)?.toDouble() ?? 0,
  noiseAttack: (m['noiseAttack'] as num?)?.toDouble() ?? 0,
  brightnessFactor: (m['brightnessFactor'] as num?)?.toDouble() ?? 0.3,
  synthEngine: m['synthEngine'] as String?,
  envCurve: m['envCurve'] != null
      ? EnvelopeCurve.fromJson(m['envCurve'] as Map<String, dynamic>)
      : null,
  filterType: m['filterType'] as String? ?? 'lowPass',
  filterCutoff: (m['filterCutoff'] as num?)?.toDouble() ?? 1200,
  filterResonance: (m['filterResonance'] as num?)?.toDouble() ?? 1.2,
  filterEnvAmount: (m['filterEnvAmount'] as num?)?.toDouble() ?? 2.0,
  filterAttack: (m['filterAttack'] as num?)?.toDouble() ?? 0.005,
  filterDecay: (m['filterDecay'] as num?)?.toDouble() ?? 0.3,
  filterSustain: (m['filterSustain'] as num?)?.toDouble() ?? 0.3,
  fmRatio: (m['fmRatio'] as num?)?.toDouble() ?? 2.0,
  fmIndex: (m['fmIndex'] as num?)?.toDouble() ?? 3.0,
  fmDecay: (m['fmDecay'] as num?)?.toDouble() ?? 0.8,
  fmFeedback: (m['fmFeedback'] as num?)?.toDouble() ?? 0.15,
  morphRate: (m['morphRate'] as num?)?.toDouble() ?? 0,
);

class _DataWriter {
  final List<int> _data;
  int _offset = 0;
  _DataWriter(int size) : _data = List.filled(size, 0);
  Uint8List get bytes => Uint8List.fromList(_data);
  void writeString(String s) {
    for (int i = 0; i < s.length; i++) {
      _data[_offset++] = s.codeUnitAt(i);
    }
  }
  void writeInt32(int value) {
    _data[_offset++] = value & 0xFF;
    _data[_offset++] = (value >> 8) & 0xFF;
    _data[_offset++] = (value >> 16) & 0xFF;
    _data[_offset++] = (value >> 24) & 0xFF;
  }
  void writeInt16(int value) {
    _data[_offset++] = value & 0xFF;
    _data[_offset++] = (value >> 8) & 0xFF;
  }
}

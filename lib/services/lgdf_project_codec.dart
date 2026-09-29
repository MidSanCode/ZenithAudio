import 'package:flutter/material.dart';

import '../core/constants/app_config.dart';
import '../core/constants/app_constants.dart';
import '../models/note.dart';
import '../models/pattern.dart';
import '../models/playlist.dart';
import '../models/project.dart';
import '../models/track.dart';
import 'lgdf_format.dart';
import 'synth_engine.dart' show TrackCompressorParams;

/// Platform-neutral LGDF document codec shared by the desktop and web
/// project serializers.
///
/// It converts a [Project] to/from the LGDF description layer
/// (`spec/project.json`, `spec/config.json`, `spec/config.schema.json`,
/// `spec/overview.md`) and migrates legacy `.zap` documents.
class LgdfProjectCodec {
  const LgdfProjectCodec();

  /// LGDF `info.json` values preserved across saves.
  static const String defaultDescription = '卓声 ZENITH AUDIO 工程（LGDF 目录模式）';
  static const List<String> defaultTags = ['zenith-audio', 'daw', 'music'];

  // ────────────────────────────────────────────────────────────
  // Serialization
  // ────────────────────────────────────────────────────────────

  /// The DAW document stored at `spec/project.json` (snake_case keys).
  Map<String, dynamic> buildProjectDocument(
    Project project,
    Map<String, String> trackAssetPaths,
  ) {
    return {
      'document_version': AppConstants.projectFormatVersion,
      'project_id': project.id,
      'name': project.name,
      'sample_rate': _num(project.sampleRate),
      'time_signature': {
        'numerator': project.timeSignatureNumerator,
        'denominator': project.timeSignatureDenominator,
      },
      'key_signature': project.keySignature,
      'bpm': _num(project.bpm),
      'playback_speed': _num(project.playbackSpeed),
      'tracks': [
        for (final t in project.tracks)
          buildTrack(t, trackAssetPaths, bpm: project.bpm),
      ],
    };
  }

  /// Whole doubles are emitted as integers, matching the reference documents.
  static num _num(double value) =>
      value == value.roundToDouble() ? value.toInt() : value;

  Map<String, dynamic> buildTrack(
    Track t,
    Map<String, String> trackAssetPaths, {
    double bpm = 120.0,
  }) {
    return {
      'id': t.id,
      'name': t.name,
      'type': t.type.name,
      'volume': _num(t.volume),
      'pan': _num(t.pan),
      'is_muted': t.isMuted,
      'is_solo': t.isSolo,
      'color': '#${t.color.toARGB32().toRadixString(16).padLeft(8, '0')}',
      'duration': _num(t.duration),
      if (t.type == TrackType.audio) 'audio_file': trackAssetPaths[t.id],
      if (t.isInstrument) 'instrument_name': t.instrumentName,
      if (t.isInstrument && t.notes.isNotEmpty)
        'notes': t.notes.map(buildNote).toList(),
      if (t.stepPattern.isNotEmpty) 'step_pattern': t.stepPattern,
      if (t.compressor != null) 'compressor': t.compressor!.toJson(),
      // S0 optional extension slots (S3/S6 fill these in).
      if (t.mixerChannelId != null) 'mixer_channel_id': t.mixerChannelId,
      if (t.sends != null) 'sends': t.sends,
      if (t.automation != null) 'automation': t.automation,
    };
  }

  Map<String, dynamic> buildNote(Note n) => {
        'pitch': n.pitch,
        // Ticks are authoritative; the seconds shadow field is written for one
        // version cycle so older readers keep working unchanged.
        'start_ticks': n.startTicks,
        'length_ticks': n.lengthTicks,
        'start_time': _num(n.startTime),
        'duration': _num(n.duration),
        'velocity': n.velocity,
      };

  /// `spec/config.json` — runtime configuration (mirrors the reference shape).
  Map<String, dynamic> buildSpecConfig(Project project, String slug) => {
        'project': {
          'name': slug,
          'display_name': project.name,
        },
        'engine': {
          'name': 'zenith-audio',
          'version': AppConfig.appVersion,
        },
        'settings': {
          'bpm': _num(project.bpm),
          'key_signature': project.keySignature,
          'sample_rate': _num(project.sampleRate),
          'playback_speed': _num(project.playbackSpeed),
          'track_count': project.tracks.length,
          'language': 'zh-CN',
        },
      };

  /// `spec/config.schema.json` — JSON Schema validating `spec/config.json`.
  Map<String, dynamic> buildSpecConfigSchema() => {
        '\$schema': 'http://json-schema.org/draft-07/schema#',
        '\$id': 'lgdf://zenith-audio/config.schema.json',
        'title': '卓声工程运行配置',
        'type': 'object',
        'additionalProperties': false,
        'required': ['project', 'engine', 'settings'],
        'properties': {
          'project': {
            'type': 'object',
            'required': ['name', 'display_name'],
            'properties': {
              'name': {'type': 'string', 'pattern': r'^[a-z0-9_-]+$'},
              'display_name': {'type': 'string'},
            },
          },
          'engine': {
            'type': 'object',
            'required': ['name', 'version'],
            'properties': {
              'name': {'type': 'string'},
              'version': {'type': 'string'},
            },
          },
          'settings': {
            'type': 'object',
            'required': ['bpm', 'key_signature', 'sample_rate'],
            'properties': {
              'bpm': {'type': 'number', 'minimum': 20, 'maximum': 300},
              'key_signature': {'type': 'string'},
              'sample_rate': {'type': 'number'},
              'playback_speed': {'type': 'number'},
              'track_count': {'type': 'integer', 'minimum': 0},
              'language': {'type': 'string'},
            },
          },
        },
      };

  /// `spec/overview.md` — human-readable project summary.
  String buildOverview(Project project, Map<String, String> trackAssetPaths) {
    final buffer = StringBuffer()
      ..writeln('# ${project.name}')
      ..writeln()
      ..writeln('卓声 ZENITH AUDIO 工程 · LGDF 目录模式')
      ..writeln()
      ..writeln('## 工程信息')
      ..writeln()
      ..writeln('| 项目 | 值 |')
      ..writeln('| --- | --- |')
      ..writeln('| 速度 | ${project.bpm.toStringAsFixed(1)} BPM |')
      ..writeln(
          '| 拍号 | ${project.timeSignatureNumerator}/${project.timeSignatureDenominator} |')
      ..writeln('| 调号 | ${project.keySignature} |')
      ..writeln('| 采样率 | ${project.sampleRate.toInt()} Hz |')
      ..writeln('| 时长 | ${project.duration.toStringAsFixed(2)} s |')
      ..writeln('| 音轨数 | ${project.tracks.length} |')
      ..writeln()
      ..writeln('## 音轨清单')
      ..writeln();

    for (final t in project.tracks) {
      buffer.writeln('- **${t.name}** (${t.type.name})');
      if (t.isInstrument && t.notes.isNotEmpty) {
        buffer.writeln('  - 音符数：${t.notes.length}');
      }
      final asset = trackAssetPaths[t.id];
      if (asset != null) buffer.writeln('  - 资源：`$asset`');
    }

    buffer
      ..writeln()
      ..writeln('## 目录结构')
      ..writeln()
      ..writeln('```')
      ..writeln('info.json          核心配置')
      ..writeln('registry.json      注册资源清单')
      ..writeln('assets/audios/     音频资源')
      ..writeln('metadata/audios/   资源元数据（一一镜像）')
      ..writeln('spec/              描述层（本目录）')
      ..writeln('```');

    return buffer.toString();
  }

  /// `info.json` content for a project.
  Map<String, dynamic> buildInfo({
    required Project project,
    required int createdTime,
    required int lastUpdateTime,
    int version = 1,
  }) {
    final slug = Lgdf.projectSlug(project.name, project.id);
    return Lgdf.buildInfo(
      name: slug,
      displayName: project.name,
      description: defaultDescription,
      tags: defaultTags,
      createdTime: createdTime,
      lastUpdateTime: lastUpdateTime,
      version: version,
      extra: {
        'project_id': project.id,
        'app': AppConstants.appNameEn,
        'app_format_version': AppConstants.projectFormatVersion,
      },
    );
  }

  // ────────────────────────────────────────────────────────────
  // Parsing
  // ────────────────────────────────────────────────────────────

  Project parseProjectDocument(Map<String, dynamic> doc) {
    final version = (doc['document_version'] as num?)?.toInt() ?? 1;
    if (version > AppConstants.projectFormatVersion) {
      throw Exception('Project requires a newer version of the app');
    }

    final timeSig = doc['time_signature'] as Map<String, dynamic>?;
    final tracksJson = doc['tracks'] as List<dynamic>? ?? [];
    final bpm = (doc['bpm'] as num?)?.toDouble() ?? 120;

    return Project(
      id: doc['project_id'] as String? ?? '',
      name: doc['name'] as String? ?? AppConstants.untitledProjectName,
      tracks: tracksJson
          .map((j) => parseTrack(j as Map<String, dynamic>, bpm: bpm))
          .toList(),
      sampleRate: (doc['sample_rate'] as num?)?.toDouble() ?? 44100,
      timeSignatureNumerator: (timeSig?['numerator'] as num?)?.toInt() ?? 4,
      timeSignatureDenominator: (timeSig?['denominator'] as num?)?.toInt() ?? 4,
      keySignature: doc['key_signature'] as String? ?? 'C',
      bpm: bpm,
      playbackSpeed: (doc['playback_speed'] as num?)?.toDouble() ?? 1.0,
      patterns: (doc['patterns'] as List<dynamic>?)
              ?.map((e) => Pattern.fromJson(e as Map<String, dynamic>))
              .toList() ??
          const [],
      playlist: doc['playlist'] == null
          ? null
          : Playlist.fromJson(doc['playlist'] as Map<String, dynamic>),
    );
  }

  Track parseTrack(Map<String, dynamic> t, {double bpm = 120}) {
    final typeStr = t['type'] as String? ?? 'audio';
    final type = TrackType.values.firstWhere(
      (e) => e.name == typeStr,
      orElse: () => TrackType.audio,
    );
    return Track(
      id: t['id'] as String? ?? '',
      name: t['name'] as String? ?? 'Track',
      type: type,
      instrumentName: t['instrument_name'] as String?,
      notes: t['notes'] != null
          ? (t['notes'] as List<dynamic>)
              .map((n) => parseNote(n as Map<String, dynamic>, bpm: bpm))
              .toList()
          : const [],
      volume: (t['volume'] as num?)?.toDouble() ?? 0.8,
      pan: (t['pan'] as num?)?.toDouble() ?? 0.0,
      isMuted: t['is_muted'] as bool? ?? false,
      isSolo: t['is_solo'] as bool? ?? false,
      audioFilePath: t['audio_file'] as String?,
      color: parseColor(t['color'] as String?),
      duration: (t['duration'] as num?)?.toDouble() ?? 0,
      stepPattern: (t['step_pattern'] as List<dynamic>?)
              ?.map((e) => e as bool)
              .toList() ??
          const [],
      compressor: t['compressor'] != null
          ? TrackCompressorParams.fromJson(
              t['compressor'] as Map<String, dynamic>)
          : null,
      mixerChannelId: t['mixer_channel_id'] as String?,
      sends: (t['sends'] as List<dynamic>?)?.cast<Map<String, dynamic>>(),
      automation: (t['automation'] as List<dynamic>?)
          ?.cast<Map<String, dynamic>>(),
    );
  }

  Note parseNote(Map<String, dynamic> n, {double bpm = 120.0}) {
    // New documents carry `start_ticks` / `length_ticks`; older ones only have
    // seconds, which are converted through the project tempo.
    final startTicks = (n['start_ticks'] as num?)?.toInt();
    final lengthTicks = (n['length_ticks'] as num?)?.toInt();
    if (startTicks != null && lengthTicks != null) {
      return Note(
        pitch: (n['pitch'] as num?)?.toInt() ?? 60,
        startTicks: startTicks,
        lengthTicks: lengthTicks,
        velocity: (n['velocity'] as num?)?.toInt() ?? 100,
        bpm: bpm,
      );
    }
    return Note.fromSeconds(
      pitch: (n['pitch'] as num?)?.toInt() ?? 60,
      startTime: (n['start_time'] as num?)?.toDouble() ?? 0,
      duration: (n['duration'] as num?)?.toDouble() ?? 1,
      velocity: (n['velocity'] as num?)?.toInt() ?? 100,
      bpm: bpm,
    );
  }

  /// Parses a legacy `.zap` info.json (camelCase keys, tracks inline).
  Project parseLegacyInfo(Map<String, dynamic> info) {
    final version = info['version'] as int? ?? 1;
    if (version > AppConstants.projectFormatVersion) {
      throw Exception('Project requires a newer version of the app');
    }

    final tracksJson = info['tracks'] as List<dynamic>? ?? [];
    final legacyBpm = (info['bpm'] as num?)?.toDouble() ?? 120;
    final tracks = tracksJson.map((j) {
      final t = j as Map<String, dynamic>;
      final typeStr = t['type'] as String? ?? 'audio';
      final type = TrackType.values.firstWhere(
        (e) => e.name == typeStr,
        orElse: () => TrackType.audio,
      );
      return Track(
        id: t['id'] as String? ?? '',
        name: t['name'] as String? ?? 'Track',
        type: type,
        instrumentName: t['instrumentName'] as String?,
        notes: (type == TrackType.instrument || type == TrackType.synth)
            ? ((t['notes'] as List<dynamic>?)
                    ?.map((n) =>
                        parseNote(n as Map<String, dynamic>, bpm: legacyBpm))
                    .toList() ??
                [])
            : const [],
        volume: (t['volume'] as num?)?.toDouble() ?? 0.8,
        isMuted: t['isMuted'] as bool? ?? false,
        isSolo: t['isSolo'] as bool? ?? false,
        audioFilePath: t['audioFile'] as String?,
        color: parseColor(t['color'] as String?),
        duration: (t['duration'] as num?)?.toDouble() ?? 0,
        compressor: t['compressor'] != null
            ? TrackCompressorParams.fromJson(
                t['compressor'] as Map<String, dynamic>)
            : null,
      );
    }).toList();

    return Project(
      id: info['id'] as String? ?? '',
      name: info['name'] as String? ?? 'Untitled',
      tracks: tracks,
      sampleRate: (info['sampleRate'] as num?)?.toDouble() ?? 44100,
      timeSignatureNumerator: info['timeSignatureNumerator'] as int? ?? 4,
      timeSignatureDenominator: info['timeSignatureDenominator'] as int? ?? 4,
      keySignature: info['keySignature'] as String? ?? 'C',
      bpm: (info['bpm'] as num?)?.toDouble() ?? 120,
      playbackSpeed: (info['playbackSpeed'] as num?)?.toDouble() ?? 1.0,
    );
  }

  Color parseColor(String? hex) {
    if (hex == null) return const Color(0xFF40C4FF);
    try {
      final val = int.parse(hex.replaceFirst('#', ''), radix: 16);
      return Color(val);
    } catch (_) {
      return const Color(0xFF40C4FF);
    }
  }
}

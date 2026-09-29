import 'package:flutter/material.dart';
import '../core/constants/app_constants.dart';
import '../models/note.dart';
import '../services/synth_engine.dart' show TrackCompressorParams;

enum TrackType { audio, instrument, synth }

class Track {
  final String id;
  final String name;
  final TrackType type;
  final String? instrumentName;
  final List<Note> notes;
  final double volume;
  final double pan;
  final bool isMuted;
  final bool isSolo;
  final String? audioFilePath;
  final Color color;
  final double duration;
  final List<bool> stepPattern;

  /// Per-track compressor (instrument tracks; null = disabled).
  final TrackCompressorParams? compressor;

  // ── S0 optional extension slots ──
  // Reserved for later stages; absent in older projects and never required.
  // All are plain JSON so they round-trip without a schema bump.

  /// Mixer channel this track feeds (S3). null = derive one channel per track.
  final String? mixerChannelId;

  /// Send configuration (S3): one entry per send slot.
  final List<Map<String, dynamic>>? sends;

  /// Automation lanes attached to this track (S2).
  final List<Map<String, dynamic>>? automation;

  /// True for note-based tracks (instrument or synth) — they carry notes +
  /// an instrumentName preset and render through the synth pipeline.
  bool get isInstrument =>
      type == TrackType.instrument || type == TrackType.synth;

  const Track({
    required this.id,
    required this.name,
    this.type = TrackType.audio,
    this.instrumentName,
    this.notes = const [],
    this.volume = 0.8,
    this.pan = 0.0,
    this.isMuted = false,
    this.isSolo = false,
    this.audioFilePath,
    this.color = AppColors.waveform,
    this.duration = 0,
    this.stepPattern = const [],
    this.compressor,
    this.mixerChannelId,
    this.sends,
    this.automation,
  });

  double get computedDuration {
    if (isInstrument && notes.isNotEmpty) {
      final last = notes.last;
      return last.startTime + last.duration;
    }
    return duration;
  }

  Track copyWith({
    String? id,
    String? name,
    TrackType? type,
    String? instrumentName,
    List<Note>? notes,
    double? volume,
    double? pan,
    bool? isMuted,
    bool? isSolo,
    String? audioFilePath,
    Color? color,
    double? duration,
    List<bool>? stepPattern,
    TrackCompressorParams? compressor,
    String? mixerChannelId,
    List<Map<String, dynamic>>? sends,
    List<Map<String, dynamic>>? automation,
  }) {
    return Track(
      id: id ?? this.id,
      name: name ?? this.name,
      type: type ?? this.type,
      instrumentName: instrumentName ?? this.instrumentName,
      notes: notes ?? this.notes,
      volume: volume ?? this.volume,
      pan: pan ?? this.pan,
      isMuted: isMuted ?? this.isMuted,
      isSolo: isSolo ?? this.isSolo,
      audioFilePath: audioFilePath ?? this.audioFilePath,
      color: color ?? this.color,
      duration: duration ?? this.duration,
      stepPattern: stepPattern ?? this.stepPattern,
      compressor: compressor ?? this.compressor,
      mixerChannelId: mixerChannelId ?? this.mixerChannelId,
      sends: sends ?? this.sends,
      automation: automation ?? this.automation,
    );
  }

  bool get shouldPlay {
    return !isMuted && volume > 0;
  }
}

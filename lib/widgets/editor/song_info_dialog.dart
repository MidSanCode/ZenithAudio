import 'dart:ui' show FontFeature;
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../../models/project.dart';
import '../../providers/playback_provider.dart';
import '../../providers/project_provider.dart';
import '../../services/audio_service.dart';

/// Song info panel: project metadata, playback position, output format
/// (sample rate / bitrate) and the active audio device.
class SongInfoDialog extends ConsumerStatefulWidget {
  const SongInfoDialog({super.key});

  static Future<void> show(BuildContext context) {
    return showDialog<void>(
      context: context,
      builder: (_) => const SongInfoDialog(),
    );
  }

  @override
  ConsumerState<SongInfoDialog> createState() => _SongInfoDialogState();
}

class _SongInfoDialogState extends ConsumerState<SongInfoDialog> {
  String _formatTime(double sec) {
    if (sec < 0) sec = 0;
    final m = sec ~/ 60;
    final s = (sec % 60).floor();
    return '${m.toString().padLeft(2, '0')}:${s.toString().padLeft(2, '0')}';
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final project = ref.watch(projectProvider);
    final playhead = ref.watch(playheadPositionProvider);
    final isPlaying = ref.watch(playbackProvider) == PlaybackState.playing;
    final out = ref.watch(audioServiceProvider).getOutputInfo();

    return AlertDialog(
      title: Row(children: [
        const Icon(Icons.info_outline_rounded, size: 20),
        const SizedBox(width: 8),
        Text('songInfo.title'.tr()),
      ]),
      content: SizedBox(
        width: 380,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              _section(cs, 'songInfo.sectionProject'),
              _row(cs, 'songInfo.projectName', project.name),
              _row(cs, 'songInfo.bpm',
                  project.bpm.toStringAsFixed(project.bpm.truncateToDouble() == project.bpm ? 0 : 1)),
              _row(cs, 'songInfo.timeSignature',
                  '${project.timeSignatureNumerator}/${project.timeSignatureDenominator}'),
              _row(cs, 'songInfo.keySignature', project.keySignature),
              _row(cs, 'songInfo.trackCount', '${project.tracks.length}'),
              _row(cs, 'songInfo.totalDuration', _formatTime(project.duration)),
              const SizedBox(height: 12),
              _section(cs, 'songInfo.sectionPlayback'),
              _row(cs, 'songInfo.state',
                  isPlaying ? 'songInfo.statePlaying'.tr() : 'songInfo.stateStopped'.tr()),
              // Live-updating playhead row.
              Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  Text('songInfo.position'.tr(),
                      style: TextStyle(color: cs.onSurfaceVariant, fontSize: 13)),
                  Text(_formatTime(playhead),
                      style: TextStyle(
                          color: cs.onSurface,
                          fontSize: 13,
                          fontFeatures: const [FontFeature.tabularFigures()])),
                ],
              ),
              _row(cs, 'songInfo.playbackSpeed',
                  'x${project.playbackSpeed.toStringAsFixed(2)}'),
              const SizedBox(height: 12),
              _section(cs, 'songInfo.sectionAudio'),
              _row(cs, 'songInfo.sampleRate',
                  out?.sampleRate != null
                      ? '${(out!.sampleRate! / 1000).toStringAsFixed(1)} kHz'
                      : 'songInfo.notPlaying'.tr()),
              _row(cs, 'songInfo.bitDepth', '16 bit'),
              _row(cs, 'songInfo.channels',
                  out?.channels != null
                      ? (out!.channels == 1
                          ? 'songInfo.channelsMono'.tr()
                          : 'songInfo.channelsStereo'.tr())
                      : 'songInfo.notPlaying'.tr()),
              _row(cs, 'songInfo.bitrate',
                  out?.bitrateKbps != null && out!.bitrateKbps! > 0
                      ? '${out.bitrateKbps!.toStringAsFixed(0)} kbps'
                      : 'songInfo.notPlaying'.tr()),
              const SizedBox(height: 12),
              _section(cs, 'songInfo.sectionOutput'),
              _row(cs, 'songInfo.outputDevice',
                  out?.deviceDescription ?? 'songInfo.deviceAuto'.tr()),
              if (out != null && out.availableDevices.length > 1)
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(
                    '${'songInfo.deviceCount'.tr()}: ${out.availableDevices.length}',
                    style: TextStyle(color: cs.onSurfaceVariant, fontSize: 11),
                  ),
                ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text('common.close'.tr()),
        ),
      ],
    );
  }

  Widget _section(ColorScheme cs, String key) => Padding(
        padding: const EdgeInsets.only(bottom: 6),
        child: Text(
          key.tr(),
          style: TextStyle(
            color: cs.primary,
            fontSize: 12,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.5,
          ),
        ),
      );

  Widget _row(ColorScheme cs, String key, String value) => Padding(
        padding: const EdgeInsets.symmetric(vertical: 2),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(key.tr(),
                style: TextStyle(color: cs.onSurfaceVariant, fontSize: 13)),
            Flexible(
              child: Text(value,
                  textAlign: TextAlign.right,
                  style: TextStyle(color: cs.onSurface, fontSize: 13)),
            ),
          ],
        ),
      );
}

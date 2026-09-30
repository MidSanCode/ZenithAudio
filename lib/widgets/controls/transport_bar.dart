import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/theme_colors.dart';
import '../../engine/audio_engine_adapter.dart';
import '../../providers/playback_provider.dart';
import '../../providers/project_provider.dart';
import '../../providers/recording_provider.dart';
import '../../core/utils/logger.dart';

String _formatTime(double seconds) {
  final m = (seconds ~/ 60).toString().padLeft(2, '0');
  final s = (seconds % 60).toStringAsFixed(1).padLeft(4, '0');
  return '$m:$s';
}

class TransportBar extends ConsumerWidget {
  const TransportBar({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final playbackState = ref.watch(playbackProvider);
    final playhead = ref.watch(playheadPositionProvider);
    final project = ref.watch(projectProvider);
    final masterVol = ref.watch(masterVolumeProvider);
    final recState = ref.watch(recordingProvider);
    final recElapsed = ref.watch(recordingElapsedProvider);
    final isRecording = recState == RecordingState.recording;
    final wavProgress = ref.watch(wavGenerationProgressProvider);
    final cs = Theme.of(context).colorScheme;

    return Container(
      height: AppConstants.transportBarHeight,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(
          top: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          _TransportButton(
            icon: Icons.skip_previous_rounded,
            tooltip: 'transport.skipStart'.tr(),
            onTap: () {
              ref.read(playbackProvider.notifier).seekTo(0);
              AppLogger.i('跳到开头');
            },
          ),
          const SizedBox(width: 4),
          _TransportButton(
            icon: playbackState == PlaybackState.playing
                ? Icons.pause_rounded
                : Icons.play_arrow_rounded,
            tooltip: playbackState == PlaybackState.playing
                ? 'transport.pause'.tr()
                : 'transport.play'.tr(),
            size: 32, iconSize: 24, isPrimary: true,
            onTap: () {
              ref.read(playbackProvider.notifier).toggle();
            },
          ),
          const SizedBox(width: 4),
          _TransportButton(
            icon: Icons.stop_rounded,
            tooltip: 'transport.stop'.tr(),
            onTap: () {
              ref.read(playbackProvider.notifier).stop();
              AppLogger.i('停止');
            },
          ),
          const SizedBox(width: 4),
          _TransportButton(
            icon: Icons.skip_next_rounded,
            tooltip: 'transport.skipEnd'.tr(),
            onTap: () {
              final dur = project.duration > 0 ? project.duration : 60.0;
              ref.read(playbackProvider.notifier).seekTo(dur);
              AppLogger.i('跳到末尾');
            },
          ),
          const SizedBox(width: 4),
          _TransportButton(
            icon: isRecording ? Icons.stop_rounded : Icons.fiber_manual_record_rounded,
            tooltip: isRecording ? 'transport.stopRec'.tr() : 'transport.record'.tr(),
            size: 28, iconSize: 18,
            onTap: () async {
              final notifier = ref.read(recordingProvider.notifier);
              if (isRecording) {
                final path = await notifier.stopRecording();
                if (path != null) {
                  ref.read(projectProvider.notifier).addTrack(
                    name: '录音_${DateTime.now().millisecondsSinceEpoch}',
                    audioFilePath: path,
                  );
                }
              } else {
                await notifier.startRecording();
              }
            },
          ),
          const SizedBox(width: 16),
          if (isRecording)
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Container(
                    width: 10, height: 10,
                    decoration: const BoxDecoration(
                      color: Colors.red,
                      shape: BoxShape.circle,
                    ),
                  ),
                  const SizedBox(width: 4),
                  Text(
                    _formatTime(recElapsed),
                    style: TextStyle(
                      color: Colors.red.shade300,
                      fontSize: 13,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
              ),
            ),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
            decoration: BoxDecoration(
              color: Theme.of(context).scaffoldBackgroundColor,
              borderRadius: BorderRadius.circular(4),
              border: Border.all(color: Theme.of(context).dividerColor),
            ),
            child: Text(
              '${_formatTime(playhead)} / ${_formatTime(project.duration > 0 ? project.duration : 0)}',
              style: TextStyle(
                color: cs.primary,
                fontSize: 13,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
          if (wavProgress > 0 && wavProgress < 1) ...[
            SizedBox(
              width: 60,
              child: LinearProgressIndicator(
                value: wavProgress,
                backgroundColor: cs.surfaceContainerHigh,
              ),
            ),
            const SizedBox(width: 8),
          ],
          const Spacer(),
          Icon(Icons.volume_up_outlined, size: 16, color: context.outline),
          const SizedBox(width: 6),
          SizedBox(
            width: 100,
            child: SliderTheme(
              data: SliderTheme.of(context).copyWith(
                trackHeight: 3,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
                overlayShape: const RoundSliderOverlayShape(overlayRadius: 10),
              ),
              child: Slider(
                value: masterVol,
                min: 0, max: 1,
                onChanged: (v) {
                  ref.read(masterVolumeProvider.notifier).state = v;
                  ref.read(audioEngineProvider).masterVolume = v;
                },
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _TransportButton extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback? onTap;
  final double size;
  final double iconSize;
  final bool isPrimary;

  const _TransportButton({
    required this.icon,
    required this.tooltip,
    this.onTap,
    this.size = 28,
    this.iconSize = 18,
    this.isPrimary = false,
  });

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Tooltip(
      message: tooltip,
      child: Material(
        color: isPrimary ? cs.primary : Colors.transparent,
        borderRadius: BorderRadius.circular(isPrimary ? 16 : 4),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(isPrimary ? 16 : 4),
          hoverColor: isPrimary
              ? cs.primary.withAlpha(204)
              : context.surfaceHigh,
          child: Container(
            width: size, height: size,
            alignment: Alignment.center,
            child: Icon(
              icon,
              size: iconSize,
              color: isPrimary
                  ? Theme.of(context).scaffoldBackgroundColor
                  : cs.onSurfaceVariant,
            ),
          ),
        ),
      ),
    );
  }
}

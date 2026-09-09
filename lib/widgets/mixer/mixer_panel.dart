import 'dart:math' as math;
import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../../core/constants/app_constants.dart';
import '../../models/track.dart';
import '../../models/project.dart';
import '../../providers/project_provider.dart';
import '../../providers/playback_provider.dart';
import '../../providers/mixer_provider.dart';
import '../../providers/layout_provider.dart';
import '../layout/rotary_knob.dart';

class MixerPanel extends ConsumerWidget {
  const MixerPanel({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final mixerState = ref.watch(mixerProvider);
    final project = ref.watch(projectProvider);

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        _MixerHeader(mixerState: mixerState),
        if (mixerState.isExpanded)
          _MixerStrips(project: project),
      ],
    );
  }
}

class _MixerHeader extends ConsumerWidget {
  final MixerState mixerState;
  const _MixerHeader({required this.mixerState});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    final isPlaying = ref.watch(playbackProvider) == PlaybackState.playing;

    return GestureDetector(
      onTap: () => ref.read(mixerProvider.notifier).toggleExpanded(),
      child: Container(
        height: AppConstants.mixerPanelCollapsedHeight,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        decoration: BoxDecoration(
          color: cs.surfaceContainerHigh,
          border: Border(
            top: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
          ),
        ),
        child: Row(
          children: [
            Icon(
              mixerState.isExpanded
                  ? Icons.keyboard_arrow_down_rounded
                  : Icons.keyboard_arrow_up_rounded,
              size: 14,
              color: cs.onSurfaceVariant,
            ),
            const SizedBox(width: 4),
            Icon(Icons.tune_rounded, size: 12, color: cs.onSurfaceVariant),
            const SizedBox(width: 4),
            Text(
              'MIXER',
              style: TextStyle(
                color: cs.onSurfaceVariant,
                fontSize: 9,
                fontWeight: FontWeight.w700,
                letterSpacing: 1.2,
              ),
            ),
            const SizedBox(width: 8),
            Text(
              'mixer.channels'.tr(namedArgs: {'n': '${ref.watch(projectProvider).tracks.length}'}),
              style: TextStyle(color: cs.onSurfaceVariant.withAlpha(153), fontSize: 9),
            ),
            const Spacer(),
            if (mixerState.isExpanded)
              _MasterLevelMeter(isPlaying: isPlaying),
          ],
        ),
      ),
    );
  }
}

class _MasterLevelMeter extends StatefulWidget {
  final bool isPlaying;
  const _MasterLevelMeter({required this.isPlaying});

  @override
  State<_MasterLevelMeter> createState() => _MasterLevelMeterState();
}

class _MasterLevelMeterState extends State<_MasterLevelMeter>
    with SingleTickerProviderStateMixin {
  late AnimationController _animCtrl;
  double _level = 0;

  @override
  void initState() {
    super.initState();
    _animCtrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 50),
    )..addListener(() {
        if (widget.isPlaying) {
          _level = (math.Random().nextDouble() * 0.8 + 0.1);
        } else {
          _level = 0;
        }
        setState(() {});
      });
    _animCtrl.repeat();
  }

  @override
  void dispose() {
    _animCtrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 60,
      height: 12,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(2),
        child: Stack(
          children: [
            Container(color: AppColors.muteStrip),
            AnimatedContainer(
              duration: const Duration(milliseconds: 80),
              width: 60 * _level,
              height: 12,
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  colors: [
                    AppColors.neonGreen,
                    AppColors.neonYellow,
                    AppColors.neonOrange,
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _MixerStrips extends ConsumerWidget {
  final Project project;
  const _MixerStrips({required this.project});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final isPlaying = ref.watch(playbackProvider) == PlaybackState.playing;
    final stripWidth = 52.0;
    // Resizable: drag the divider above the mixer header (see audio_editor).
    final mixerH = ref.watch(mixerHeightProvider).clamp(
        AppConstants.mixerPanelCollapsedHeight + 90.0, 480.0);
    final stripHeight = mixerH - AppConstants.mixerPanelCollapsedHeight;

    return Container(
      height: stripHeight,
      decoration: BoxDecoration(
        color: AppColors.mixerBackground,
        border: Border(
          top: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          Expanded(
            child: ListView.builder(
              scrollDirection: Axis.horizontal,
              itemCount: project.tracks.length + 1,
              itemExtent: stripWidth,
              itemBuilder: (context, index) {
                if (index == project.tracks.length) {
                  return _MasterStrip(
                    height: stripHeight,
                    isPlaying: isPlaying,
                  );
                }
                return _ChannelStrip(
                  track: project.tracks[index],
                  height: stripHeight,
                  isPlaying: isPlaying,
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

class _ChannelStrip extends ConsumerWidget {
  final Track track;
  final double height;
  final bool isPlaying;

  const _ChannelStrip({
    required this.track,
    required this.height,
    required this.isPlaying,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    final hasSolo = ref.watch(projectProvider).hasSoloTrack;

    return Container(
      width: 52,
      decoration: BoxDecoration(
        color: hasSolo && !track.isSolo
            ? cs.surfaceContainerHighest.withAlpha(100)
            : Colors.transparent,
        border: Border(
          right: BorderSide(color: Theme.of(context).dividerColor.withAlpha(77), width: 0.5),
        ),
      ),
      child: Column(
        children: [
          // Color strip + name
          Container(
            height: 18,
            color: track.color.withAlpha(38),
            padding: const EdgeInsets.symmetric(horizontal: 3),
            child: Row(
              children: [
                Container(
                  width: 3,
                  height: 12,
                  decoration: BoxDecoration(
                    color: track.color,
                    borderRadius: BorderRadius.circular(1),
                  ),
                ),
                const SizedBox(width: 2),
                Expanded(
                  child: Text(
                    track.name,
                    style: TextStyle(
                      color: cs.onSurface,
                      fontSize: 8,
                      fontWeight: FontWeight.w500,
                    ),
                    overflow: TextOverflow.ellipsis,
                    maxLines: 1,
                  ),
                ),
              ],
            ),
          ),
          // Level meter
          Expanded(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              child: _LevelMeter(
                trackId: track.id,
                isPlaying: isPlaying && !track.isMuted,
                color: track.color,
              ),
            ),
          ),
          // Controls row
          Container(
            height: 22,
            padding: const EdgeInsets.symmetric(horizontal: 3),
            child: Row(
              children: [
                // Pan knob
                SizedBox(
                  width: 18, height: 18,
                  child: RotaryKnob(
                    value: (track.pan + 1) / 2,
                    min: 0, max: 1,
                    size: 16,
                    activeColor: AppColors.neonGreen,
                    onChanged: (v) => ref
                        .read(projectProvider.notifier)
                        .updateTrackPan(track.id, (v * 2) - 1),
                  ),
                ),
                const Spacer(),
                // Mute
                GestureDetector(
                  onTap: () => ref
                      .read(projectProvider.notifier)
                      .toggleTrackMute(track.id),
                  child: Container(
                    width: 10, height: 10,
                    decoration: BoxDecoration(
                      color: track.isMuted ? AppColors.mute : AppColors.ledOff,
                      borderRadius: BorderRadius.circular(1),
                    ),
                  ),
                ),
                const SizedBox(width: 2),
                // Solo
                GestureDetector(
                  onTap: () => ref
                      .read(projectProvider.notifier)
                      .toggleTrackSolo(track.id),
                  child: Container(
                    width: 10, height: 10,
                    decoration: BoxDecoration(
                      color: track.isSolo ? AppColors.solo : AppColors.ledOff,
                      borderRadius: BorderRadius.circular(1),
                    ),
                  ),
                ),
              ],
            ),
          ),
          // Volume fader
          SizedBox(
            height: 32,
            child: RotatedBox(
              quarterTurns: -1,
              child: SliderTheme(
                data: SliderTheme.of(context).copyWith(
                  trackHeight: 3,
                  thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 4),
                  overlayShape: const RoundSliderOverlayShape(overlayRadius: 8),
                  activeTrackColor: AppColors.accent,
                  inactiveTrackColor: cs.surfaceContainerHighest,
                  thumbColor: cs.onSurface,
                ),
                child: Slider(
                  value: track.volume,
                  min: 0, max: 1, divisions: 100,
                  onChanged: (v) => ref
                      .read(projectProvider.notifier)
                      .updateTrackVolume(track.id, v),
                ),
              ),
            ),
          ),
          const SizedBox(height: 2),
        ],
      ),
    );
  }
}

class _MasterStrip extends ConsumerWidget {
  final double height;
  final bool isPlaying;

  const _MasterStrip({
    required this.height,
    required this.isPlaying,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    final mixer = ref.watch(mixerProvider);

    return Container(
      width: 52,
      decoration: BoxDecoration(
        color: AppColors.masterStrip,
        border: Border(
          left: BorderSide(color: Theme.of(context).dividerColor.withAlpha(128), width: 1),
        ),
      ),
      child: Column(
        children: [
          Container(
            height: 18,
            color: Colors.black.withAlpha(77),
            alignment: Alignment.center,
            child: Text(
              'MASTER',
              style: TextStyle(
                color: cs.onSurface,
                fontSize: 8,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.5,
              ),
            ),
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              child: _LevelMeter(
                trackId: '__master__',
                isPlaying: isPlaying,
                color: AppColors.neonOrange,
              ),
            ),
          ),
          Container(
            height: 22,
            alignment: Alignment.center,
            child: Icon(Icons.volume_up_rounded, size: 10, color: cs.onSurfaceVariant),
          ),
          SizedBox(
            height: 32,
            child: RotatedBox(
              quarterTurns: -1,
              child: SliderTheme(
                data: SliderTheme.of(context).copyWith(
                  trackHeight: 3,
                  thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 4),
                  overlayShape: const RoundSliderOverlayShape(overlayRadius: 8),
                  activeTrackColor: AppColors.neonOrange,
                  inactiveTrackColor: cs.surfaceContainerHighest,
                  thumbColor: cs.onSurface,
                ),
                child: Slider(
                  value: mixer.masterVolume,
                  min: 0, max: 1, divisions: 100,
                  onChanged: (v) => ref
                      .read(mixerProvider.notifier)
                      .setMasterVolume(v),
                ),
              ),
            ),
          ),
          const SizedBox(height: 2),
        ],
      ),
    );
  }
}

class _LevelMeter extends StatefulWidget {
  final String trackId;
  final bool isPlaying;
  final Color color;

  const _LevelMeter({
    required this.trackId,
    required this.isPlaying,
    required this.color,
  });

  @override
  State<_LevelMeter> createState() => _LevelMeterState();
}

class _LevelMeterState extends State<_LevelMeter>
    with SingleTickerProviderStateMixin {
  late AnimationController _animCtrl;
  double _level = 0;

  @override
  void initState() {
    super.initState();
    _animCtrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 60),
    )..addListener(() {
        if (widget.isPlaying) {
          _level = (math.Random().nextDouble() * 0.7 + 0.05).clamp(0.0, 1.0);
        } else {
          _level = 0;
        }
        if (mounted) setState(() {});
      });
    _animCtrl.repeat();
  }

  @override
  void dispose() {
    _animCtrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final maxH = constraints.maxHeight;
        return ClipRRect(
          borderRadius: BorderRadius.circular(2),
          child: Stack(
            children: [
              Container(
                decoration: BoxDecoration(
                  color: AppColors.muteStrip,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
              Align(
                alignment: Alignment.bottomCenter,
                child: AnimatedContainer(
                  duration: const Duration(milliseconds: 80),
                  width: double.infinity,
                  height: maxH * _level,
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topCenter,
                      end: Alignment.bottomCenter,
                      colors: [
                        widget.color.withAlpha(180),
                        widget.color,
                      ],
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

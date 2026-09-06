import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/responsive_utils.dart';
import '../../models/track.dart';
import '../../core/instrument_picker.dart';
import '../../providers/project_provider.dart';
import '../../providers/settings_provider.dart';
import '../../providers/playback_provider.dart';
import '../../providers/floating_window_provider.dart';
import '../../services/synth_engine.dart' show TrackCompressorParams;
import '../layout/rotary_knob.dart';
import 'piano_roll_editor.dart';
import 'audio_clip_editor.dart';

String _formatDuration(double sec) {
  final m = (sec ~/ 60).toString().padLeft(2, '0');
  final s = (sec % 60).toStringAsFixed(1).padLeft(4, '0');
  return '$m:$s';
}

class TrackTile extends ConsumerWidget {
  final Track track;
  final int index;

  const TrackTile({
    super.key,
    required this.track,
    required this.index,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final hasSolo = ref.watch(projectProvider).hasSoloTrack;
    final isPlaying = ref.watch(playbackProvider) == PlaybackState.playing;
    final cs = Theme.of(context).colorScheme;
    final isMobile = getScreenSize(context) == ScreenSize.mobile;

    return GestureDetector(
      onTap: () => _openEditor(context, ref),
      onSecondaryTapDown: (details) =>
          _showContextMenu(context, ref, details.localPosition),
      onLongPressStart: (details) {
        if (isMobile) _showContextMenu(context, ref, details.localPosition);
      },
      child: Container(
        height: AppConstants.trackTileHeight,
        decoration: BoxDecoration(
          color: _getBackgroundColor(context, hasSolo),
          border: Border(
            bottom: BorderSide(color: Theme.of(context).dividerColor.withAlpha(128)),
          ),
        ),
        child: Row(
          children: [
            _ColorStrip(color: track.color),
            _ChannelLabel(index: index, cs: cs),
            _TypeIcon(track: track, cs: cs),
            const SizedBox(width: 4),
            Expanded(
              child: _TrackName(
                track: track, cs: cs,
                onRename: (name) => ref.read(projectProvider.notifier).renameTrack(track.id, name),
              ),
            ),
            if (!isMobile) ...[
              _VolumeKnob(track: track, ref: ref),
              _PanKnob(track: track, ref: ref),
            ],
            _ControlButtons(track: track, ref: ref),
            if (track.type == TrackType.instrument && !isMobile)
              Flexible(
                child: ClipRect(
                  child: SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: _StepGrid(track: track, ref: ref, isPlaying: isPlaying),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Color _getBackgroundColor(BuildContext context, bool hasSolo) {
    final bg = Theme.of(context).scaffoldBackgroundColor;
    if (hasSolo && !track.isSolo) return bg.withAlpha(128);
    if (track.isMuted) return bg.withAlpha(179);
    return bg;
  }

  void _openEditor(BuildContext context, WidgetRef ref) {
    if (track.type == TrackType.instrument) {
      final settings = ref.read(settingsProvider);
      if (settings.editorMode == 'float') {
        ref.read(floatingWindowProvider.notifier).open(
          title: 'Piano Roll: ${track.name}',
          builder: (_) => PianoRollEditor(trackId: track.id, isFloating: true),
        );
      } else {
        Navigator.of(context).push(
          MaterialPageRoute(
            builder: (_) => PianoRollEditor(trackId: track.id),
          ),
        );
      }
    } else if (track.type == TrackType.audio) {
      final settings = ref.read(settingsProvider);
      if (settings.editorMode == 'float') {
        ref.read(floatingWindowProvider.notifier).open(
          title: 'Audio: ${track.name}',
          builder: (_) => AudioClipEditor(trackId: track.id, isFloating: true),
        );
      } else {
        openAudioClipEditor(context, track.id);
      }
    }
  }

  void _showContextMenu(BuildContext context, WidgetRef ref, Offset localPos) {
    final renderBox = context.findRenderObject() as RenderBox?;
    if (renderBox == null) return;
    final globalPos = renderBox.localToGlobal(localPos);

    final items = <PopupMenuEntry<String>>[
      const PopupMenuItem(value: 'rename', child: Text('重命名')),
      const PopupMenuItem(value: 'properties', child: Text('属性')),
    ];
    if (track.type == TrackType.instrument) {
      items.add(const PopupMenuItem(value: 'editPianoRoll', child: Text('编辑钢琴卷帘')));
      items.add(const PopupMenuItem(value: 'changeInstrument', child: Text('更换乐器')));
      items.add(PopupMenuItem(
          value: 'compressor',
          child: Row(children: [
            Icon(track.compressor?.enabled == true
                ? Icons.compress
                : Icons.compress_outlined,
                size: 16),
            const SizedBox(width: 8),
            Text(track.compressor?.enabled == true ? '压缩器 ✓' : '压缩器'),
          ])));
    } else if (track.type == TrackType.audio) {
      items.add(const PopupMenuItem(value: 'editAudio', child: Text('编辑音频')));
    }
    items.add(const PopupMenuItem(value: 'delete', child: Text('删除')));

    showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(
          globalPos.dx, globalPos.dy, globalPos.dx + 1, globalPos.dy + 1),
      items: items,
    ).then((value) {
      final settings = ref.read(settingsProvider);
      switch (value) {
        case 'rename':
          _showRenameDialog(context, ref);
        case 'properties':
          _showPropertiesDialog(context);
        case 'editPianoRoll':
          if (settings.editorMode == 'float') {
            ref.read(floatingWindowProvider.notifier).open(
              title: 'Piano Roll: ${track.name}',
              builder: (_) => PianoRollEditor(trackId: track.id, isFloating: true),
            );
          } else {
            Navigator.of(context).push(
              MaterialPageRoute(
                builder: (_) => PianoRollEditor(trackId: track.id),
              ),
            );
          }
        case 'editAudio':
          if (settings.editorMode == 'float') {
            ref.read(floatingWindowProvider.notifier).open(
              title: 'Audio: ${track.name}',
              builder: (_) => AudioClipEditor(trackId: track.id, isFloating: true),
            );
          } else {
            openAudioClipEditor(context, track.id);
          }
        case 'changeInstrument':
          _showChangeInstrumentDialog(context, ref);
        case 'compressor':
          _showCompressorDialog(context, ref);
        case 'delete':
          _showDeleteDialog(context, ref);
      }
    });
  }

  Future<void> _showRenameDialog(BuildContext context, WidgetRef ref) async {
    final controller = TextEditingController(text: track.name);
    final newName = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('重命名音轨'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(labelText: '音轨名称'),
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.of(ctx).pop(),
              child: const Text('取消')),
          TextButton(
              onPressed: () => Navigator.of(ctx).pop(controller.text),
              child: const Text('确定')),
        ],
      ),
    );
    controller.dispose();
    if (newName != null && newName.trim().isNotEmpty) {
      ref.read(projectProvider.notifier).renameTrack(track.id, newName.trim());
    }
  }

  Future<void> _showChangeInstrumentDialog(BuildContext context, WidgetRef ref) async {
    final current = track.instrumentName ?? 'piano';
    final instrument = await showInstrumentPicker(context, current: current);
    if (instrument != null) {
      ref.read(projectProvider.notifier).setTrackInstrument(track.id, instrument);
    }
  }

  void _showPropertiesDialog(BuildContext context) {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('音轨属性'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('名称: ${track.name}'),
            const SizedBox(height: 8),
            Text('类型: ${track.type == TrackType.instrument ? "乐器" : "音频"}'),
            if (track.type == TrackType.instrument && track.instrumentName != null)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text('乐器: ${track.instrumentName}'),
              ),
            const SizedBox(height: 8),
            Text('音符: ${track.notes.length}'),
            const SizedBox(height: 8),
            Text('时长: ${_formatDuration(track.duration)}'),
          ],
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.of(ctx).pop(),
              child: const Text('确定')),
        ],
      ),
    );
  }

  Future<void> _showDeleteDialog(BuildContext context, WidgetRef ref) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('删除音轨'),
        content: Text('确定删除音轨 "${track.name}" 吗？此操作不可撤销。'),
        actions: [
          TextButton(
              onPressed: () => Navigator.of(ctx).pop(false),
              child: const Text('取消')),
          TextButton(
              onPressed: () => Navigator.of(ctx).pop(true),
              child: const Text('删除',
                  style: TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    if (confirmed == true) {
      ref.read(projectProvider.notifier).removeTrack(track.id);
    }
  }

  void _showCompressorDialog(BuildContext context, WidgetRef ref) {
    final cur = track.compressor ?? const TrackCompressorParams();
    showDialog<void>(
      context: context,
      builder: (ctx) => _CompressorDialog(
        initial: cur,
        onApply: (params) {
          ref
              .read(projectProvider.notifier)
              .setTrackCompressor(track.id, params);
        },
      ),
    );
  }
}

/// Per-track compressor editor: enable toggle + threshold/ratio/attack/
/// release sliders with live numeric readouts.
class _CompressorDialog extends StatefulWidget {
  final TrackCompressorParams initial;
  final ValueChanged<TrackCompressorParams> onApply;

  const _CompressorDialog({required this.initial, required this.onApply});

  @override
  State<_CompressorDialog> createState() => _CompressorDialogState();
}

class _CompressorDialogState extends State<_CompressorDialog> {
  late bool _enabled = widget.initial.enabled;
  late double _threshold = widget.initial.threshold;
  late double _ratio = widget.initial.ratio;
  late double _attack = widget.initial.attack;
  late double _release = widget.initial.release;

  TrackCompressorParams get _params => TrackCompressorParams(
        enabled: _enabled,
        threshold: _threshold,
        ratio: _ratio,
        attack: _attack,
        release: _release,
      );

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return AlertDialog(
      title: const Row(children: [
        Icon(Icons.compress, size: 20),
        SizedBox(width: 8),
        Text('轨道压缩器', style: TextStyle(fontSize: 16)),
      ]),
      content: SizedBox(
        width: 340,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              dense: true,
              title: const Text('启用压缩', style: TextStyle(fontSize: 13)),
              subtitle: const Text('加快衰减、压平动态(渲染时应用)',
                  style: TextStyle(fontSize: 10)),
              value: _enabled,
              onChanged: (v) => setState(() => _enabled = v),
            ),
            const SizedBox(height: 4),
            _row('阈值', _formatThreshold(), Slider(
              value: _threshold,
              min: 0.05,
              max: 0.95,
              onChanged: (v) => setState(() => _threshold = v),
            )),
            _row('比率', '${_ratio.toStringAsFixed(1)} : 1', Slider(
              value: _ratio,
              min: 1,
              max: 20,
              onChanged: (v) => setState(() => _ratio = v),
            )),
            _row('启动', '${(_attack * 1000).round()} ms', Slider(
              value: _attack,
              min: 0.0005,
              max: 0.05,
              onChanged: (v) => setState(() => _attack = v),
            )),
            _row('释放', '${(_release * 1000).round()} ms', Slider(
              value: _release,
              min: 0.01,
              max: 0.5,
              onChanged: (v) => setState(() => _release = v),
            )),
            const SizedBox(height: 6),
            Text(
              '预设:',
              style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant),
            ),
            Wrap(
              spacing: 6,
              children: [
                OutlinedButton(
                  onPressed: () => setState(() {
                    _enabled = true; _threshold = 0.35; _ratio = 8; _attack = 0.002; _release = 0.05;
                  }),
                  child: const Text('快速衰减', style: TextStyle(fontSize: 11)),
                ),
                OutlinedButton(
                  onPressed: () => setState(() {
                    _enabled = true; _threshold = 0.5; _ratio = 3; _attack = 0.01; _release = 0.12;
                  }),
                  child: const Text('平滑 glue', style: TextStyle(fontSize: 11)),
                ),
                OutlinedButton(
                  onPressed: () => setState(() {
                    _enabled = true; _threshold = 0.25; _ratio = 12; _attack = 0.001; _release = 0.03;
                  }),
                  child: const Text('极限压平', style: TextStyle(fontSize: 11)),
                ),
              ],
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () {
            widget.onApply(const TrackCompressorParams(enabled: false));
            Navigator.pop(context);
          },
          child: const Text('关闭压缩器'),
        ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () {
            widget.onApply(_params);
            Navigator.pop(context);
          },
          child: const Text('应用'),
        ),
      ],
    );
  }

  String _formatThreshold() => '${(_threshold * 100).round()}%';

  Widget _row(String label, String value, Widget slider) {
    return Row(
      children: [
        SizedBox(width: 46, child: Text(label, style: const TextStyle(fontSize: 11))),
        Expanded(child: slider),
        SizedBox(
          width: 52,
          child: Text(value,
              style: const TextStyle(fontSize: 10), textAlign: TextAlign.right),
        ),
      ],
    );
  }
}

// ──── Sub-widgets ────

class _ColorStrip extends StatelessWidget {
  final Color color;
  const _ColorStrip({required this.color});

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 4,
      height: double.infinity,
      decoration: BoxDecoration(
        color: color,
        borderRadius: const BorderRadius.only(
          topLeft: Radius.circular(2),
          bottomLeft: Radius.circular(2),
        ),
      ),
    );
  }
}

class _ChannelLabel extends StatelessWidget {
  final int index;
  final ColorScheme cs;
  const _ChannelLabel({required this.index, required this.cs});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 20,
      child: Center(
        child: Text(
          '${index + 1}',
          style: TextStyle(
            color: cs.onSurfaceVariant.withAlpha(179),
            fontSize: 9,
            fontWeight: FontWeight.w400,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
      ),
    );
  }
}

class _TypeIcon extends StatelessWidget {
  final Track track;
  final ColorScheme cs;
  const _TypeIcon({required this.track, required this.cs});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(right: 4),
      child: Icon(
        track.type == TrackType.instrument
            ? Icons.piano_rounded
            : Icons.audiotrack_rounded,
        size: 13,
        color: track.color.withAlpha(204),
      ),
    );
  }
}

class _TrackName extends StatefulWidget {
  final Track track;
  final ColorScheme cs;
  final ValueChanged<String> onRename;
  const _TrackName({
    required this.track,
    required this.cs,
    required this.onRename,
  });

  @override
  State<_TrackName> createState() => _TrackNameState();
}

class _TrackNameState extends State<_TrackName> {
  late bool _isEditing;
  late TextEditingController _controller;

  @override
  void initState() {
    super.initState();
    _isEditing = false;
    _controller = TextEditingController(text: widget.track.name);
  }

  @override
  void didUpdateWidget(_TrackName old) {
    super.didUpdateWidget(old);
    if (widget.track.name != old.track.name && !_isEditing) {
      _controller.text = widget.track.name;
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _startEditing() {
    setState(() => _isEditing = true);
    _controller.selection = TextSelection(
      baseOffset: 0,
      extentOffset: _controller.text.length,
    );
  }

  void _finishEditing() {
    final newName = _controller.text.trim();
    if (newName.isNotEmpty && newName != widget.track.name) {
      widget.onRename(newName);
    }
    setState(() => _isEditing = false);
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onDoubleTap: _startEditing,
      child: _isEditing
          ? SizedBox(
              height: 20,
              child: TextField(
                controller: _controller,
                autofocus: true,
                style: TextStyle(
                  color: widget.cs.onSurface,
                  fontSize: 11,
                  fontWeight: FontWeight.w500,
                ),
                decoration: const InputDecoration(
                  isDense: true,
                  contentPadding: EdgeInsets.zero,
                  border: InputBorder.none,
                ),
                onSubmitted: (_) => _finishEditing(),
                onTapOutside: (_) => _finishEditing(),
              ),
            )
          : Text(
              widget.track.name,
              style: TextStyle(
                color: widget.cs.onSurface,
                fontSize: 11,
                fontWeight: FontWeight.w500,
              ),
              overflow: TextOverflow.ellipsis,
              maxLines: 1,
            ),
    );
  }
}

class _VolumeKnob extends StatelessWidget {
  final Track track;
  final WidgetRef ref;
  const _VolumeKnob({required this.track, required this.ref});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(right: 2),
      child: SizedBox(
        width: 28,
        child: Center(
          child: RotaryKnob(
            value: track.volume,
            min: 0, max: 1,
            size: 18,
            onChanged: (v) =>
                ref.read(projectProvider.notifier).updateTrackVolume(track.id, v),
          ),
        ),
      ),
    );
  }
}

class _PanKnob extends StatelessWidget {
  final Track track;
  final WidgetRef ref;
  const _PanKnob({required this.track, required this.ref});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(right: 2),
      child: SizedBox(
        width: 28,
        child: Center(
          child: RotaryKnob(
            value: (track.pan + 1) / 2,
            min: 0, max: 1,
            size: 18,
            onChanged: (v) =>
                ref.read(projectProvider.notifier).updateTrackPan(track.id, (v * 2) - 1),
          ),
        ),
      ),
    );
  }
}

class _ControlButtons extends StatelessWidget {
  final Track track;
  final WidgetRef ref;
  const _ControlButtons({required this.track, required this.ref});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(right: 4),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _LedButton(
            active: track.isMuted,
            onColor: AppColors.mute,
            offColor: AppColors.ledOff,
            onTap: () => ref.read(projectProvider.notifier).toggleTrackMute(track.id),
          ),
          const SizedBox(width: 3),
          _LedButton(
            active: track.isSolo,
            onColor: AppColors.solo,
            offColor: AppColors.ledOff,
            onTap: () => ref.read(projectProvider.notifier).toggleTrackSolo(track.id),
          ),
        ],
      ),
    );
  }
}

class _LedButton extends StatelessWidget {
  final bool active;
  final Color onColor;
  final Color offColor;
  final VoidCallback onTap;

  const _LedButton({
    required this.active,
    required this.onColor,
    required this.offColor,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        width: 18,
        height: 14,
        decoration: BoxDecoration(
          color: active ? onColor : offColor,
          borderRadius: BorderRadius.circular(2),
          border: Border.all(
            color: active
                ? onColor.withAlpha(179)
                : Theme.of(context).dividerColor.withAlpha(128),
            width: 0.5,
          ),
        ),
      ),
    );
  }
}

class _StepGrid extends ConsumerWidget {
  final Track track;
  final WidgetRef ref;
  final bool isPlaying;

  const _StepGrid({
    required this.track,
    required this.ref,
    required this.isPlaying,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pattern = track.stepPattern;
    final currentStep = ref.watch(currentStepProvider);
    final cellSize = 14.0;
    final gap = 1.0;

    return SizedBox(
      height: AppConstants.trackTileHeight - 4,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: List.generate(16, (i) {
          final isActive = i < pattern.length && pattern[i];
          final isCurrent = isPlaying && i == currentStep;
          return Padding(
            padding: EdgeInsets.only(right: gap),
            child: GestureDetector(
              onTap: () =>
                  ref.read(projectProvider.notifier).toggleTrackStep(track.id, i),
              child: Container(
                width: cellSize,
                height: double.infinity,
                decoration: BoxDecoration(
                  color: isCurrent
                      ? (isActive
                          ? AppColors.accent
                          : AppColors.accent.withAlpha(77))
                      : (isActive
                          ? AppColors.stepActive
                          : AppColors.stepInactive),
                  borderRadius: BorderRadius.circular(2),
                  border: isCurrent
                      ? Border.all(color: AppColors.playhead, width: 1)
                      : null,
                ),
              ),
            ),
          );
        }),
      ),
    );
  }
}

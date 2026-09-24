import 'dart:ui' show FontFeature;

import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/theme_colors.dart';
import '../../core/instrument_picker.dart';
import '../../providers/project_provider.dart';
import '../../providers/playback_provider.dart';
import '../../providers/settings_provider.dart';
import '../../providers/browser_provider.dart';
import '../../services/file_service.dart';
import '../../services/audio_converter.dart';
import '../../services/cpu_monitor.dart';
import '../../screens/settings_page.dart';
import '../../screens/about_dialog.dart' as app;
import '../editor/song_info_dialog.dart';
import '../../core/utils/logger.dart';
class AudioToolBar extends ConsumerWidget {
  const AudioToolBar({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    final playback = ref.watch(playbackProvider);
    final project = ref.watch(projectProvider);
    final loop = ref.watch(settingsProvider).autoLoop;
    final isMobile = MediaQuery.of(context).size.width < AppConstants.mobileBreakpoint;

    return Container(
      height: AppConstants.toolbarHeight,
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(
          bottom: BorderSide(color: cs.outlineVariant, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          // ── Back to workspace ──
          _TbBtn(
            icon: Icons.arrow_back_rounded,
            tooltip: 'toolbar.backToWorkspace'.tr(),
            onTap: () => ref.read(projectProvider.notifier).leaveEditor(context),
          ),
          _TbSep(),

          // ── File operations ──
          _TbBtn(
            icon: Icons.note_add_outlined,
            tooltip: 'toolbar.newProject'.tr(),
            onTap: () => ref.read(projectProvider.notifier).tryNewProject(context),
          ),
          _TbBtn(
            icon: Icons.folder_open_outlined,
            tooltip: 'toolbar.openProject'.tr(),
            onTap: () => ref.read(projectProvider.notifier).openProject(),
          ),
          _TbBtn(
            icon: Icons.save_outlined,
            tooltip: 'toolbar.saveProject'.tr(),
            onTap: () => ref.read(projectProvider.notifier).saveProject(),
          ),
          _TbSep(),

          // ── Undo / Redo ──
          _TbBtn(
            icon: Icons.undo_outlined,
            tooltip: 'toolbar.undo'.tr(),
            onTap: () => ref.read(projectProvider.notifier).undo(),
          ),
          _TbBtn(
            icon: Icons.redo_outlined,
            tooltip: 'toolbar.redo'.tr(),
            onTap: () => ref.read(projectProvider.notifier).redo(),
          ),
          _TbSep(),

          // ── Transport ──
          _TbBtn(
            icon: Icons.skip_previous_rounded,
            tooltip: 'transport.skipStart'.tr(),
            iconSize: 14,
            onTap: () => ref.read(playbackProvider.notifier).seekTo(0),
          ),
          _TbBtn(
            icon: playback == PlaybackState.playing
                ? Icons.pause_rounded
                : Icons.play_arrow_rounded,
            tooltip: playback == PlaybackState.playing
                ? 'transport.pause'.tr()
                : 'transport.play'.tr(),
            iconSize: 18,
            isPrimary: true,
            onTap: () => ref.read(playbackProvider.notifier).toggle(),
          ),
          _TbBtn(
            icon: Icons.stop_rounded,
            tooltip: 'transport.stop'.tr(),
            iconSize: 14,
            onTap: () => ref.read(playbackProvider.notifier).stop(),
          ),
          _TbBtn(
            icon: Icons.fiber_manual_record_rounded,
            tooltip: 'transport.record'.tr(),
            iconSize: 12,
            onTap: () {},
          ),
          _TbBtn(
            icon: Icons.skip_next_rounded,
            tooltip: 'transport.skipEnd'.tr(),
            iconSize: 14,
            onTap: () {
              final dur = project.duration > 0 ? project.duration : 60.0;
              ref.read(playbackProvider.notifier).seekTo(dur);
            },
          ),
          _TbBtn(
            icon: loop ? Icons.loop_rounded : Icons.loop_outlined,
            tooltip: 'settings.playback.autoLoop'.tr(),
            iconSize: 14,
            active: loop,
            activeColor: AppColors.neonGreen,
            onTap: () {
              final notifier = ref.read(settingsProvider.notifier);
              notifier.setAutoLoop(!loop);
            },
          ),
          _TbSep(),

          // ── Pattern / Playlist placeholder ──
          _ModeChip(
            label: 'PAT',
            tooltip: 'toolbar.patternMode'.tr(),
            selected: true,
            onTap: () {},
          ),
          _ModeChip(
            label: 'SONG',
            tooltip: 'toolbar.songMode'.tr(),
            selected: false,
            onTap: () {},
          ),
          _TbSep(),

          // ── Time display (bars:beats:ticks) ──
          _TimeDisplay(playhead: ref.watch(playheadPositionProvider), bpm: project.bpm),
          const SizedBox(width: 8),

          const Spacer(),

          // ── BPM (compact) ──
          _BpmWidget(bpm: project.bpm, ref: ref),
          const SizedBox(width: 8),

          // ── CPU usage ──
          const _CpuIndicator(),
          const SizedBox(width: 8),

          // ── Snap toggle ──
          _TbBtn(
            icon: Icons.grid_on_outlined,
            tooltip: 'settings.pianoRoll.snapToGrid'.tr(),
            iconSize: 14,
            active: ref.watch(settingsProvider).snapToGrid,
            activeColor: AppColors.accent,
            onTap: () {
              final notifier = ref.read(settingsProvider.notifier);
              notifier.setSnapToGrid(!ref.read(settingsProvider).snapToGrid);
            },
          ),
          _TbSep(),

          // ── Add Track / Song Info / Settings / Zoom / Menu ──
          if (!isMobile) ...[
            _TbBtn(
              icon: Icons.add,
              tooltip: 'toolbar.addTrack'.tr(),
              iconSize: 16,
              onTap: () {
                final idx = ref.read(projectProvider).tracks.length + 1;
                ref.read(projectProvider.notifier).addTrack(name: 'Track $idx');
              },
            ),
            _TbBtn(
              icon: Icons.piano_outlined,
              tooltip: 'toolbar.addInstrumentTrack'.tr(),
              iconSize: 14,
              onTap: () async {
                final inst = await showInstrumentPicker(context);
                if (inst == null) return;
                final idx = ref.read(projectProvider).tracks.length + 1;
                ref.read(projectProvider.notifier).addInstrumentTrack(
                  name: 'Track $idx', instrumentName: inst,
                );
              },
            ),
          ],
          _TbBtn(
            icon: ref.watch(browserVisibilityProvider)
                ? Icons.folder_rounded
                : Icons.folder_outlined,
            tooltip: 'toolbar.browser'.tr(),
            iconSize: 14,
            onTap: () {
              final v = ref.read(browserVisibilityProvider.notifier);
              v.state = !v.state;
            },
          ),
          _TbBtn(
            icon: Icons.info_outline_rounded,
            tooltip: 'songInfo.title'.tr(),
            iconSize: 14,
            onTap: () => SongInfoDialog.show(context),
          ),
          _TbBtn(
            icon: Icons.tune_outlined,
            tooltip: 'toolbar.projectSettings'.tr(),
            iconSize: 14,
            onTap: () => _showSettingsDialog(context, ref),
          ),
          _TbBtn(
            icon: Icons.zoom_in_outlined,
            tooltip: 'toolbar.zoomIn'.tr(),
            iconSize: 14,
            onTap: () {
              final cur = ref.read(pixelsPerSecondProvider);
              ref.read(pixelsPerSecondProvider.notifier).state =
                  (cur * 1.25).clamp(10, 500);
            },
          ),
          _TbBtn(
            icon: Icons.zoom_out_outlined,
            tooltip: 'toolbar.zoomOut'.tr(),
            iconSize: 14,
            onTap: () {
              final cur = ref.read(pixelsPerSecondProvider);
              ref.read(pixelsPerSecondProvider.notifier).state =
                  (cur / 1.25).clamp(10, 500);
            },
          ),
          if (isMobile)
            _TbBtn(
              icon: Icons.menu,
              tooltip: 'toolbar.menu'.tr(),
              iconSize: 16,
              onTap: () => _showMobileMenu(context, ref),
            ),
        ],
      ),
    );
  }

  void _showMobileMenu(BuildContext context, WidgetRef ref) {
    showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(999, 34, 999, 34),
      items: [
        PopupMenuItem(value: 'new', child: Text('menu.file.newProject'.tr())),
        PopupMenuItem(value: 'open', child: Text('menu.file.openProject'.tr())),
        PopupMenuItem(value: 'save', child: Text('menu.file.saveProject'.tr())),
        PopupMenuItem(value: 'import', child: Text('menu.track.importAudio'.tr())),
        PopupMenuItem(value: 'addAudio', child: Text('toolbar.addTrack'.tr())),
        PopupMenuItem(value: 'addInst', child: Text('menu.track.addInstrument'.tr())),
        PopupMenuItem(value: 'settings', child: Text('menu.file.settings'.tr())),
        PopupMenuItem(value: 'about', child: Text('menu.help.about'.tr())),
      ],
    ).then((v) {
      if (v == null) return;
      switch (v) {
        case 'new': ref.read(projectProvider.notifier).tryNewProject(context);
        case 'open': ref.read(projectProvider.notifier).openProject();
        case 'save': ref.read(projectProvider.notifier).saveProject();
        case 'addAudio':
          final idx = ref.read(projectProvider).tracks.length + 1;
          ref.read(projectProvider.notifier).addTrack(name: 'Track $idx');
        case 'addInst':
          showInstrumentPicker(context).then((inst) {
            if (inst == null) return;
            final idx = ref.read(projectProvider).tracks.length + 1;
            ref.read(projectProvider.notifier).addInstrumentTrack(
              name: 'Track $idx', instrumentName: inst,
            );
          });
        case 'settings': Navigator.of(context).push(MaterialPageRoute(builder: (_) => const SettingsPage()));
        case 'about': showDialog(context: context, builder: (_) => const app.AboutDialog());
      }
    });
  }

  static void _showSettingsDialog(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    final bpmCtrl = TextEditingController(text: ref.read(projectProvider).bpm.round().toString());
    showDialog(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setDialogState) {
          final proj = ref.watch(projectProvider);
          return AlertDialog(
            backgroundColor: cs.surfaceContainer,
            title: Row(
              children: [
                Icon(Icons.tune, size: 16, color: cs.primary),
                const SizedBox(width: 8),
                Text('toolbar.projectSettings'.tr(), style: const TextStyle(fontSize: 14)),
              ],
            ),
            content: SizedBox(
              width: 300,
              child: SingleChildScrollView(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    _setLabel('songInfo.timeSignature'.tr(), cs),
                    const SizedBox(height: 4),
                    DropdownButton<String>(
                      value: '${proj.timeSignatureNumerator}/${proj.timeSignatureDenominator}',
                      isExpanded: true,
                      isDense: true,
                      items: ['2/4', '3/4', '4/4', '5/4', '6/8']
                          .map((s) => DropdownMenuItem(value: s, child: Text(s, style: const TextStyle(fontSize: 12))))
                          .toList(),
                      onChanged: (v) {
                        if (v == null) return;
                        final parts = v.split('/');
                        ref.read(projectProvider.notifier).setTimeSignature(int.parse(parts[0]), int.parse(parts[1]));
                      },
                    ),
                    const SizedBox(height: 12),
                    _setLabel('songInfo.keySignature'.tr(), cs),
                    const SizedBox(height: 4),
                    DropdownButton<String>(
                      value: proj.keySignature,
                      isExpanded: true,
                      isDense: true,
                      items: _keys.map((k) => DropdownMenuItem(value: k, child: Text(k, style: const TextStyle(fontSize: 12)))).toList(),
                      onChanged: (v) {
                        if (v != null) ref.read(projectProvider.notifier).setKeySignature(v);
                      },
                    ),
                    const SizedBox(height: 12),
                    _setLabel('BPM', cs),
                    const SizedBox(height: 4),
                    Row(
                      children: [
                        SizedBox(
                          width: 60,
                          child: TextField(
                            controller: bpmCtrl,
                            keyboardType: TextInputType.number,
                            style: const TextStyle(fontSize: 12),
                            decoration: const InputDecoration(isDense: true, contentPadding: EdgeInsets.symmetric(horizontal: 6, vertical: 4)),
                            onChanged: (v) {
                              final val = double.tryParse(v);
                              if (val != null) ref.read(projectProvider.notifier).setBpm(val);
                            },
                          ),
                        ),
                        const SizedBox(width: 8),
                        GestureDetector(
                          onVerticalDragUpdate: (d) {
                            final delta = (-d.delta.dy).round();
                            if (delta == 0) return;
                            final newVal = (proj.bpm + delta).clamp(20, 300).roundToDouble();
                            bpmCtrl.text = newVal.round().toString();
                            ref.read(projectProvider.notifier).setBpm(newVal);
                            setDialogState(() {});
                          },
                          child: Text(proj.bpm.round().toString(), style: TextStyle(fontSize: 22, fontWeight: FontWeight.bold, color: cs.primary)),
                        ),
                      ],
                    ),
                    const SizedBox(height: 12),
                    _setLabel('songInfo.playbackSpeed'.tr(), cs),
                    Row(
                      children: [
                        Expanded(
                          child: Slider(
                            value: proj.playbackSpeed,
                            min: 0.25, max: 4.0, divisions: 15,
                            onChanged: (v) {
                              ref.read(projectProvider.notifier).setPlaybackSpeed(v);
                              setDialogState(() {});
                            },
                          ),
                        ),
                        Text('${proj.playbackSpeed.toStringAsFixed(2)}x', style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant)),
                      ],
                    ),
                  ],
                ),
              ),
            ),
            actions: [
              TextButton(onPressed: () => Navigator.of(ctx).pop(), child: Text('common.close'.tr(), style: const TextStyle(fontSize: 12))),
            ],
          );
        },
      ),
    );
  }

  static const _keys = ['C', 'G', 'D', 'A', 'E', 'B', 'F#', 'C#', 'F', 'Bb', 'Eb', 'Ab', 'Db', 'Gb', 'Cb'];

  static Widget _setLabel(String text, ColorScheme cs) {
    return Text(text, style: TextStyle(fontSize: 10, color: cs.onSurfaceVariant, fontWeight: FontWeight.w600, letterSpacing: 0.5));
  }
}

// ── Toolbar button ──
class _TbBtn extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback? onTap;
  final double iconSize;
  final bool isPrimary;
  final bool active;
  final Color? activeColor;

  const _TbBtn({
    required this.icon,
    required this.tooltip,
    this.onTap,
    this.iconSize = 16,
    this.isPrimary = false,
    this.active = false,
    this.activeColor,
  });

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    Color iconColor;
    if (active) {
      iconColor = activeColor ?? cs.primary;
    } else if (isPrimary) {
      iconColor = cs.onPrimary;
    } else {
      iconColor = cs.onSurfaceVariant;
    }

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 1),
      child: Tooltip(
        message: tooltip,
        child: SizedBox(
          width: 28,
          height: 28,
          child: Material(
            color: isPrimary ? cs.primary : (active ? cs.primary.withAlpha(30) : Colors.transparent),
            borderRadius: BorderRadius.circular(4),
            child: InkWell(
              onTap: onTap,
              borderRadius: BorderRadius.circular(4),
              child: Icon(icon, size: iconSize, color: iconColor),
            ),
          ),
        ),
      ),
    );
  }
}

// ── Small separator ──
class _TbSep extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Container(
      width: 1,
      height: 20,
      margin: const EdgeInsets.symmetric(horizontal: 3),
      color: Theme.of(context).dividerColor,
    );
  }
}

// ── Mode chip (PAT / SONG) ──
class _ModeChip extends StatelessWidget {
  final String label;
  final String tooltip;
  final bool selected;
  final VoidCallback onTap;

  const _ModeChip({
    required this.label,
    required this.tooltip,
    required this.selected,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 1),
      child: Tooltip(
        message: tooltip,
        child: GestureDetector(
          onTap: onTap,
          child: Container(
            height: 22,
            padding: const EdgeInsets.symmetric(horizontal: 6),
            decoration: BoxDecoration(
              color: selected ? cs.primary : Colors.transparent,
              borderRadius: BorderRadius.circular(3),
              border: Border.all(color: selected ? cs.primary : cs.outlineVariant, width: 0.5),
            ),
            alignment: Alignment.center,
            child: Text(
              label,
              style: TextStyle(
                fontSize: 9,
                fontWeight: FontWeight.w700,
                color: selected ? cs.onPrimary : cs.onSurfaceVariant,
                letterSpacing: 0.5,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ── Time display (bars:beats:ticks) ──
class _TimeDisplay extends StatelessWidget {
  final double playhead;
  final double bpm;

  const _TimeDisplay({required this.playhead, required this.bpm});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final secondsPerBeat = 60.0 / (bpm > 0 ? bpm : 120);
    final totalBeats = playhead / secondsPerBeat;
    final bars = (totalBeats / 4).floor() + 1;
    final beats = (totalBeats % 4).floor() + 1;
    final ticks = ((totalBeats * 240) % 240).floor();

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: cs.surfaceContainerHigh,
        borderRadius: BorderRadius.circular(3),
      ),
      child: Text(
        '${bars.toString().padLeft(3, '0')}.${beats.toString().padLeft(2, '0')}.${ticks.toString().padLeft(3, '0')}',
        style: TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w600,
          color: cs.primary,
          fontFamily: 'monospace',
        ),
      ),
    );
  }
}

// ── BPM widget ──
class _BpmWidget extends StatelessWidget {
  final double bpm;
  final WidgetRef ref;

  const _BpmWidget({required this.bpm, required this.ref});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return GestureDetector(
      onVerticalDragUpdate: (details) {
        final delta = (-details.delta.dy).round();
        if (delta == 0) return;
        ref.read(projectProvider.notifier).setBpm((bpm + delta).clamp(20, 300));
      },
      child: Container(
        height: 22,
        padding: const EdgeInsets.symmetric(horizontal: 6),
        decoration: BoxDecoration(
          color: cs.surfaceContainerHigh,
          borderRadius: BorderRadius.circular(3),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.speed, size: 12, color: cs.onSurfaceVariant),
            const SizedBox(width: 4),
            Text(
              '${bpm.round()}',
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w700,
                color: cs.onSurface,
              ),
            ),
            Text(
              'toolbar.bpmSuffix'.tr(),
              style: TextStyle(
                fontSize: 9,
                color: cs.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── CPU usage indicator (Windows: GetSystemTimes via FFI) ──
class _CpuIndicator extends ConsumerStatefulWidget {
  const _CpuIndicator();

  @override
  ConsumerState<_CpuIndicator> createState() => _CpuIndicatorState();
}

class _CpuIndicatorState extends ConsumerState<_CpuIndicator> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) ref.read(cpuMonitorProvider).start();
    });
  }

  @override
  Widget build(BuildContext context) {
    final monitor = ref.watch(cpuMonitorProvider);
    if (!monitor.supported) return const SizedBox.shrink();
    final usage = ref.watch(cpuUsageProvider);
    final pct = (usage * 100).round();
    final cs = Theme.of(context).colorScheme;
    final color = pct >= 85
        ? Colors.red.shade300
        : pct >= 65
            ? Colors.orange.shade300
            : cs.onSurfaceVariant;

    return Tooltip(
      message: 'toolbar.cpuTooltip'.tr(),
      child: Container(
        height: 22,
        padding: const EdgeInsets.symmetric(horizontal: 6),
        decoration: BoxDecoration(
          color: cs.surfaceContainerHigh,
          borderRadius: BorderRadius.circular(3),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.memory_rounded, size: 12, color: color),
            const SizedBox(width: 4),
            Text(
              'CPU $pct%',
              style: TextStyle(
                fontSize: 10,
                fontWeight: FontWeight.w600,
                color: color,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

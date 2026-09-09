import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/responsive_utils.dart';
import '../../core/utils/theme_colors.dart';
import '../../providers/project_provider.dart';
import '../../providers/playback_provider.dart';
import '../../providers/settings_provider.dart';
import '../../providers/floating_window_provider.dart';
import '../toolbar/menu_bar.dart';
import '../toolbar/tool_bar.dart';
import '../layout/floating_window.dart';
import '../mixer/mixer_panel.dart';
import '../browser/browser_panel.dart';
import '../../providers/browser_provider.dart';
import '../../providers/layout_provider.dart';
import '../../providers/mixer_provider.dart';
import '../layout/resize_handle.dart';
import 'timeline_ruler.dart';
import 'track_panel.dart' show TrackPanel, ChannelRackHeader;
import 'waveform_view.dart';
import 'piano_roll_track.dart';
import 'piano_roll_editor.dart';
import 'audio_clip_editor.dart';

class AudioEditor extends ConsumerStatefulWidget {
  const AudioEditor({super.key});

  @override
  ConsumerState<AudioEditor> createState() => _AudioEditorState();
}

class _AudioEditorState extends ConsumerState<AudioEditor> {
  final ScrollController _rulerScrollCtrl = ScrollController();
  final ScrollController _waveformScrollCtrl = ScrollController();
  final ScrollController _trackVScrollCtrl = ScrollController();
  final ScrollController _channelVScrollCtrl = ScrollController();
  bool _syncing = false;
  bool _userInteracted = false;

  bool _recoveryChecked = false;

  @override
  void initState() {
    super.initState();
    _rulerScrollCtrl.addListener(_onRulerScroll);
    _waveformScrollCtrl.addListener(_onWaveformScroll);
    _trackVScrollCtrl.addListener(_onTrackVScroll);
    _channelVScrollCtrl.addListener(_onChannelVScroll);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_recoveryChecked) {
        _recoveryChecked = true;
        ProjectNotifier.checkForAutoSaveRecovery(context, ref);
      }
    });
  }

  @override
  void dispose() {
    _rulerScrollCtrl.removeListener(_onRulerScroll);
    _waveformScrollCtrl.removeListener(_onWaveformScroll);
    _trackVScrollCtrl.removeListener(_onTrackVScroll);
    _channelVScrollCtrl.removeListener(_onChannelVScroll);
    _rulerScrollCtrl.dispose();
    _waveformScrollCtrl.dispose();
    _trackVScrollCtrl.dispose();
    _channelVScrollCtrl.dispose();
    super.dispose();
  }

  void _onRulerScroll() {
    if (_syncing) return;
    _syncing = true;
    if (_waveformScrollCtrl.hasClients) {
      _waveformScrollCtrl.jumpTo(_rulerScrollCtrl.offset);
    }
    _syncing = false;
  }

  /// Keep the channel rack (left) locked to the track lanes (right).
  void _onTrackVScroll() {
    if (_syncing) return;
    _syncing = true;
    if (_channelVScrollCtrl.hasClients) {
      final maxOffset = _channelVScrollCtrl.position.maxScrollExtent;
      _channelVScrollCtrl.jumpTo(_trackVScrollCtrl.offset.clamp(0.0, maxOffset));
    }
    _syncing = false;
  }

  void _onChannelVScroll() {
    if (_syncing) return;
    _syncing = true;
    if (_trackVScrollCtrl.hasClients) {
      final maxOffset = _trackVScrollCtrl.position.maxScrollExtent;
      _trackVScrollCtrl.jumpTo(_channelVScrollCtrl.offset.clamp(0.0, maxOffset));
    }
    _syncing = false;
  }

  void _onWaveformScroll() {
    if (_syncing) return;
    _syncing = true;
    if (_rulerScrollCtrl.hasClients) {
      _rulerScrollCtrl.jumpTo(_waveformScrollCtrl.offset);
    }
    setState(() => _userInteracted = true);
    _syncing = false;
  }

  /// Auto-follow: scroll waveform view to keep the playhead visible.
  ///
  /// Called on each playhead position change via the listener below.
  void _autoScroll(double playheadSec) {
    if (!_waveformScrollCtrl.hasClients) return;
    final playing = ref.read(playbackProvider) == PlaybackState.playing;
    if (!playing || _userInteracted) return;

    final pps = ref.read(pixelsPerSecondProvider);
    final offset = _waveformScrollCtrl.offset;
    final viewportWidth = _waveformScrollCtrl.position.viewportDimension;
    final playheadScreenX = playheadSec * pps - offset;

    if (playheadScreenX > viewportWidth * 2 / 3) {
      final target = playheadSec * pps - viewportWidth * 0.2;
      final maxExtent = _waveformScrollCtrl.position.maxScrollExtent;
      _waveformScrollCtrl.animateTo(
        target.clamp(0, maxExtent),
        duration: const Duration(milliseconds: 200),
        curve: Curves.easeOut,
      );
    }

    // Reset user-interacted flag once playhead is back in view.
    if (_userInteracted &&
        playheadScreenX >= -10 &&
        playheadScreenX <= viewportWidth + 10) {
      setState(() => _userInteracted = false);
    }
  }

  void _zoom(double factor, double? focusX) {
    final oldPps = ref.read(pixelsPerSecondProvider);
    final newPps = (oldPps * factor).clamp(10.0, 500.0);
    if (newPps == oldPps) return;
    ref.read(pixelsPerSecondProvider.notifier).state = newPps;

    if (_waveformScrollCtrl.hasClients && focusX != null) {
      final offset = _waveformScrollCtrl.offset;
      final ratio = newPps / oldPps;
      final newOffset = (offset + focusX) * ratio - focusX;
      _waveformScrollCtrl.jumpTo(newOffset);
    }
  }

  void _onPointerSignal(PointerSignalEvent event) {
    if (event is! PointerScrollEvent || !_waveformScrollCtrl.hasClients) return;
    final isCtrl = HardwareKeyboard.instance.isControlPressed;
    final delta = event.scrollDelta.dy;

    if (isCtrl) {
      final factor = delta < 0 ? 1.15 : 1 / 1.15;
      final box = context.findRenderObject() as RenderBox?;
      final localPos = box != null ? box.globalToLocal(event.position) : Offset.zero;
      _zoom(factor, localPos.dx);
    } else {
      _waveformScrollCtrl.jumpTo(
        (_waveformScrollCtrl.offset + delta * 3)
            .clamp(0, _waveformScrollCtrl.position.maxScrollExtent),
      );
    }
  }

  bool get _showBackButton {
    if (!_waveformScrollCtrl.hasClients) return false;
    final pps = ref.read(pixelsPerSecondProvider);
    final pos = ref.read(playheadPositionProvider);
    final screenX = pos * pps - _waveformScrollCtrl.offset;
    final vpw = _waveformScrollCtrl.position.viewportDimension;
    return screenX < -10 || screenX > vpw + 10;
  }

  void _scrollToPlayhead() {
    if (!_waveformScrollCtrl.hasClients) return;
    final pps = ref.read(pixelsPerSecondProvider);
    final playhead = ref.read(playheadPositionProvider);
    final target = playhead * pps - 50;
    _waveformScrollCtrl.animateTo(
      target.clamp(0, _waveformScrollCtrl.position.maxScrollExtent),
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOut,
    );
    setState(() => _userInteracted = false);
  }

  void _openAudioClipEditor(BuildContext context, String trackId) {
    final settings = ref.read(settingsProvider);
    if (settings.editorMode == 'float') {
      final track = ref.read(projectProvider).tracks.firstWhere((t) => t.id == trackId);
      ref.read(floatingWindowProvider.notifier).open(
        title: 'Audio: ${track.name}',
        builder: (_) => AudioClipEditor(
          trackId: trackId,
          isFloating: true,
        ),
      );
    } else {
      openAudioClipEditor(context, trackId);
    }
  }

  void _openPianoRoll(String trackId) {
    final settings = ref.read(settingsProvider);
    if (settings.editorMode == 'float') {
      final track = ref.read(projectProvider).tracks.firstWhere((t) => t.id == trackId);
      ref.read(floatingWindowProvider.notifier).open(
        title: 'Piano Roll: ${track.name}',
        builder: (_) => PianoRollEditor(
          trackId: trackId,
          isFloating: true,
        ),
      );
    } else {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => PianoRollEditor(trackId: trackId),
        ),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final project = ref.watch(projectProvider);
    final playhead = ref.watch(playheadPositionProvider);
    final pps = ref.watch(pixelsPerSecondProvider);
    final cs = Theme.of(context).colorScheme;
    final screenSize = getScreenSize(context);
    final totalWidth = (project.duration > 0 ? project.duration : 60) * pps;
    // Resizable workspace panels.
    final isMobile = screenSize == ScreenSize.mobile;
    final rackWidth = isMobile
        ? TrackPanel.widthFor(screenSize)
        : ref.watch(trackPanelWidthProvider).clamp(180.0, 460.0);
    final mixerH = ref
        .watch(mixerHeightProvider)
        .clamp(AppConstants.mixerPanelCollapsedHeight + 90.0, 480.0);
    final mixerExpanded = ref.watch(mixerProvider).isExpanded;

    // Listen for playback state changes to reset user-interacted on play.
    ref.listen<PlaybackState>(playbackProvider, (_, next) {
      if (next == PlaybackState.playing) {
        setState(() => _userInteracted = false);
      }
    });

    // Auto-scroll: run on each playhead update while playing.
    ref.listen<double>(playheadPositionProvider, (_, pos) {
      _autoScroll(pos);
    });

    return PopScope(
      canPop: !ref.read(projectProvider.notifier).isDirty,
      onPopInvokedWithResult: (didPop, _) async {
        if (didPop) return;
        final ok = await ref.read(projectProvider.notifier).confirmDiscard(context);
        if (ok && context.mounted) Navigator.of(context).pop();
      },
      child: Scaffold(
        backgroundColor: Theme.of(context).scaffoldBackgroundColor,
        body: SafeArea(
        child: Stack(
          children: [
            Column(
              children: [
                if (screenSize != ScreenSize.mobile) const AudioMenuBar(),
                const AudioToolBar(),
                Expanded(
                  child: Listener(
                    onPointerSignal: _onPointerSignal,
                    child: Column(
                      children: [
                        Row(
                          children: [
                            // Ruler band: mirrors the left panels' widths so
                            // the ruler starts where the lanes do, and hosts
                            // the CHANNELS header so both lists below start
                            // at exactly the same y.
                            if (ref.watch(browserVisibilityProvider))
                              const SizedBox(width: AppConstants.browserPanelWidth),
                            SizedBox(
                              width: rackWidth,
                              child: ChannelRackHeader(),
                            ),
                            // Mirror the rack-resize handle so the ruler
                            // starts exactly where the lanes do.
                            if (!isMobile)
                              const SizedBox(width: ResizeHandle.thickness),
                            Expanded(
                              child: ClipRect(
                                child: SingleChildScrollView(
                                  controller: _rulerScrollCtrl,
                                  scrollDirection: Axis.horizontal,
                                  child: TimelineRuler(
                                    duration: project.duration > 0 ? project.duration : 60,
                                    pixelsPerSecond: pps,
                                    currentPosition: playhead,
                                    bpm: project.bpm,
                                    timeSignatureNumerator: project.timeSignatureNumerator,
                                    onSeek: (sec) =>
                                        ref.read(playbackProvider.notifier).seekTo(sec),
                                  ),
                                ),
                              ),
                            ),
                          ],
                        ),
                        Expanded(
                          child: Row(
                            children: [
                              const BrowserPanel(),
                              SizedBox(
                                width: rackWidth,
                                child: TrackPanel(
                                  scrollController: _channelVScrollCtrl,
                                ),
                              ),
                              ResizeHandle(
                                axis: Axis.vertical,
                                onDrag: (dx) => ref
                                        .read(trackPanelWidthProvider.notifier)
                                        .state =
                                    (ref.read(trackPanelWidthProvider) + dx)
                                        .clamp(180.0, 460.0),
                              ),
                              Expanded(
                                child: ClipRect(
                                  child: Stack(
                                    children: [
                                      project.tracks.isEmpty
                                          ? _buildEmptyState(context)
                                          : SingleChildScrollView(
                                              controller: _waveformScrollCtrl,
                                              scrollDirection: Axis.horizontal,
                                              physics: const ClampingScrollPhysics(),
                                              child: SizedBox(
                                                width: totalWidth,
                                                child: ListView.builder(
                                                  controller: _trackVScrollCtrl,
                                                  itemCount: project.tracks.length,
                                                  itemExtent: AppConstants.trackTileHeight,
                                                  itemBuilder: (context, index) {
                                                    final tr = project.tracks[index];
                                                      if (tr.isInstrument) {
                                                        return PianoRollTrack(
                                                          track: tr,
                                                          pixelsPerSecond: pps,
                                                          onEdit: () => _openPianoRoll(tr.id),
                                                        );
                                                      }
                                                    return WaveformView(
                                                      track: tr,
                                                      pixelsPerSecond: pps,
                                                      onTap: () => _openAudioClipEditor(context, tr.id),
                                                    );
                                                  },
                                                ),
                                              ),
                                            ),
                                      // Playhead line overlay.
                                      if (project.tracks.isNotEmpty)
                                      Positioned(
                                        left: playhead * pps -
                                            (_waveformScrollCtrl.hasClients
                                                ? _waveformScrollCtrl.offset
                                                : 0),
                                        top: 0,
                                        bottom: 0,
                                        child: IgnorePointer(
                                          child: Container(
                                              width: 2, color: AppColors.playhead),
                                          ),
                                        ),
                                      if (_showBackButton)
                                        Positioned(
                                          right: 8,
                                          top: 8,
                                          child: Material(
                                            color: cs.primary,
                                            borderRadius:
                                                BorderRadius.circular(20),
                                            child: InkWell(
                                              borderRadius:
                                                  BorderRadius.circular(20),
                                              onTap: _scrollToPlayhead,
                                              child: Container(
                                                width: 32,
                                                height: 32,
                                                alignment: Alignment.center,
                                                child: Icon(
                                                  Icons.play_arrow_rounded,
                                                  size: 18,
                                                  color: cs.onPrimary,
                                                ),
                                              ),
                                            ),
                                          ),
                                        ),
                                    ],
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                if (mixerExpanded)
                  ResizeHandle(
                    axis: Axis.horizontal,
                    onDrag: (dy) =>
                        ref.read(mixerHeightProvider.notifier).state =
                            ref.read(mixerHeightProvider) - dy,
                  ),
                MixerPanel(),
              ],
            ),
            // Floating windows overlay
            ...ref.watch(floatingWindowProvider).map((fw) {
              final notifier = ref.read(floatingWindowProvider.notifier);
              return FloatingWindow(
                key: ValueKey(fw.id),
                windowId: fw.id,
                title: fw.title,
                child: fw.builder(context),
                initialSize: fw.size,
                initialPosition: fw.position,
                isMinimized: fw.isMinimized,
                onMinimize: () => notifier.toggleMinimize(fw.id),
                onClose: () => notifier.close(fw.id),
                onMove: () => notifier.bringToFront(fw.id),
                onPositionChanged: (pos) => notifier.updatePosition(fw.id, pos),
                onSizeChanged: (size) => notifier.updateSize(fw.id, size),
              );
            }),
          ],
        ),     // Stack
      ),       // SafeArea
    ),        // Scaffold
  );          // PopScope + return
  }

  Widget _buildEmptyState(BuildContext context) {
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(Icons.waves_outlined, size: 64,
              color: context.outline.withAlpha(77)),
          const SizedBox(height: 16),
          Text('editor.emptyTitle'.tr(),
              style: TextStyle(color: context.outline.withAlpha(153), fontSize: 14)),
          const SizedBox(height: 8),
          Text('editor.emptySubtitle'.tr(),
              style: TextStyle(color: context.outline.withAlpha(102), fontSize: 11)),
        ],
      ),
    );
  }
}

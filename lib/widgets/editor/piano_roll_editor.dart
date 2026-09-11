import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:path_provider/path_provider.dart';
import '../../models/track.dart';
import '../../models/note.dart';
import '../../models/project.dart';
import '../../models/instrument.dart';
import '../../providers/project_provider.dart';
import '../../providers/playback_provider.dart';
import '../../providers/settings_provider.dart';
import '../../services/synth_service.dart';
import '../../services/audio_service.dart';
import '../../widgets/editor/chord_generator_dialog.dart';
import '../../core/constants/app_constants.dart';

enum ViewportMode { edit, select, scroll }

class PianoRollEditor extends ConsumerStatefulWidget {
  final String trackId;
  final bool isFloating;

  const PianoRollEditor({super.key, required this.trackId, this.isFloating = false});

  @override
  ConsumerState<PianoRollEditor> createState() => _PianoRollEditorState();
}

class _PianoRollEditorState extends ConsumerState<PianoRollEditor> {
  Track get _track =>
      ref.read(projectProvider).tracks.firstWhere((t) => t.id == widget.trackId);

  static const double _basePps = 40;
  static const double _baseRowH = 5;

  double _zoomLevel = 2.0;
  double get _pps => (_basePps * _zoomLevel).clamp(20, 600);
  double get _noteRowHeight => (_baseRowH * _zoomLevel).clamp(4, 60);

  final ScrollController _hScrollCtrl = ScrollController();
  final ScrollController _keyVScrollCtrl = ScrollController();
  final ScrollController _gridVScrollCtrl = ScrollController();
  bool _isSyncing = false;

  static bool _isMobilePlatform() =>
      !kIsWeb && (Platform.isAndroid || Platform.isIOS);

  static const int _minNote = 12;
  static const int _maxNote = 108;
  static const int _noteCount = _maxNote - _minNote + 1;

  ViewportMode _viewportMode = ViewportMode.edit;
  final Set<int> _selectedIndices = {};
  Rect? _selectionRect;
  Offset? _selectionStart;

  int? _dragNoteIndex;
  bool _isResizing = false;
  double? _dragStartX;
  double? _dragStartY;
  List<Note>? _dragOriginNotes;
  List<Note>? _localDragNotes;
  List<Rect> _ghostRects = [];

  bool _isPanning = false;
  int _activePointers = 0;
  double? _panStartX;
  double? _panStartY;
  double? _panOffX;
  double? _panOffY;

  final Map<int, Offset> _pointerPos = {};
  double? _pinchStartDist;
  double? _pinchStartZoom;

  bool _isCtrlDown = false;

  // Note preview (keyboard tap → short sound)
  final _synth = SynthService();
  String? _previewTrackId;

  // Guard
  bool _disposed = false;

  @override
  void initState() {
    super.initState();
    _keyVScrollCtrl.addListener(_syncKeyToGrid);
    _gridVScrollCtrl.addListener(_syncGridToKey);
  }

  @override
  void dispose() {
    _disposed = true;
    _stopPreview();
    _keyVScrollCtrl.removeListener(_syncKeyToGrid);
    _gridVScrollCtrl.removeListener(_syncGridToKey);
    _hScrollCtrl.dispose();
    _keyVScrollCtrl.dispose();
    _gridVScrollCtrl.dispose();
    super.dispose();
  }

  void _safeSetState(VoidCallback fn) {
    if (!_disposed && mounted) setState(fn);
  }

  void _syncKeyToGrid() {
    if (_isSyncing) return;
    _isSyncing = true;
    if (_gridVScrollCtrl.hasClients) {
      _gridVScrollCtrl.jumpTo(_keyVScrollCtrl.offset);
    }
    _isSyncing = false;
  }

  void _syncGridToKey() {
    if (_isSyncing) return;
    _isSyncing = true;
    if (_keyVScrollCtrl.hasClients) {
      _keyVScrollCtrl.jumpTo(_gridVScrollCtrl.offset);
    }
    _isSyncing = false;
  }

  double _pitchToY(int pitch) => (_maxNote - pitch) * _noteRowHeight;
  int _yToPitch(double y) =>
      (_maxNote - (y / _noteRowHeight).round()).clamp(_minNote, _maxNote);
  double _timeToX(double t) => t * _pps;
  double _xToTime(double x) => x / _pps;

  double _snapTime(double t) {
    final settings = ref.read(settingsProvider);
    if (!settings.snapToGrid) return t;
    final project = ref.read(projectProvider);
    final grid = settings.gridResolution * project.secondsPerBeat;
    if (grid <= 0) return t;
    return (t / grid).round() * grid;
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final project = ref.watch(projectProvider);
    final settings = ref.watch(settingsProvider);
    final track = _track;
    final playbackState = ref.watch(playbackProvider);
    final playhead = ref.watch(playheadPositionProvider);

    final isPlaying = playbackState == PlaybackState.playing;
    final wavProgress = ref.watch(wavGenerationProgressProvider);

    return Scaffold(
      backgroundColor: cs.surface,
      appBar: widget.isFloating ? null : _buildAppBar(context, cs, track, settings),
      body: Focus(
        autofocus: true,
        onKeyEvent: _onKeyEvent,
        child: _buildBody(context, cs, track, project, isPlaying, playhead),
      ),
      bottomNavigationBar: _buildTransportBar(cs, playbackState, playhead, project, wavProgress),
    );
  }

  KeyEventResult _onKeyEvent(FocusNode node, KeyEvent event) {
    if (event.logicalKey == LogicalKeyboardKey.controlLeft ||
        event.logicalKey == LogicalKeyboardKey.controlRight) {
      _isCtrlDown = event is KeyDownEvent;
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.space &&
        event is KeyDownEvent) {
      ref.read(playbackProvider.notifier).toggle(editingTrackId: widget.trackId);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  PreferredSizeWidget _buildAppBar(
      BuildContext context, ColorScheme cs, Track track, SettingsState settings) {
    return AppBar(
      backgroundColor: cs.surface,
      elevation: 0,
      leading: IconButton(
        icon: const Icon(Icons.arrow_back),
        onPressed: () => Navigator.of(context).pop(),
      ),
      title: Row(
        children: [
          SegmentedButton<ViewportMode>(
            segments: const [
              ButtonSegment(value: ViewportMode.edit, label: Text('Edit', style: TextStyle(fontSize: 11))),
              ButtonSegment(value: ViewportMode.select, label: Text('Select', style: TextStyle(fontSize: 11))),
              ButtonSegment(value: ViewportMode.scroll, label: Text('Scroll', style: TextStyle(fontSize: 11))),
            ],
            selected: {_viewportMode},
            onSelectionChanged: (v) => _safeSetState(() {
              _viewportMode = v.first;
              if (_viewportMode != ViewportMode.select) _selectedIndices.clear();
            }),
            style: ButtonStyle(
              visualDensity: VisualDensity.compact,
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
          ),
          const SizedBox(width: 8),
          Icon(Icons.piano_outlined, size: 18, color: track.color),
          const SizedBox(width: 6),
          Text(track.name, style: const TextStyle(fontSize: 13)),
          const Spacer(),
          _ToolChip(
            icon: Icons.library_music,
            label: 'pianoRoll.chordTool'.tr(),
            onTap: _openChordGenerator,
          ),
          const SizedBox(width: 4),
          _ToolChip(
            icon: settings.snapToGrid ? Icons.grid_on : Icons.grid_off,
            label: settings.snapToGrid ? 'Snap ON' : 'Snap OFF',
            onTap: () => ref.read(settingsProvider.notifier).setSnapToGrid(!settings.snapToGrid),
          ),
          const SizedBox(width: 4),
          if (settings.snapToGrid)
            _ToolChip(
              icon: Icons.tune,
              label: '1/${(1 / settings.gridResolution).round()}',
              onTap: _cycleGridResolution,
            ),
          const SizedBox(width: 8),
          SizedBox(
            width: 80,
            child: SliderTheme(
              data: SliderThemeData(
                trackHeight: 3,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
                overlayShape: const RoundSliderOverlayShape(overlayRadius: 12),
                activeTrackColor: cs.primary,
                inactiveTrackColor: cs.outlineVariant,
                thumbColor: cs.primary,
              ),
              child: Slider(
                value: _zoomLevel,
                min: 0.5,
                max: 10.0,
                onChanged: (v) => _safeSetState(() => _zoomLevel = v),
              ),
            ),
          ),
          SizedBox(
            width: 32,
            child: Text('${(_zoomLevel * 100).round()}%',
                style: TextStyle(fontSize: 10, color: cs.onSurfaceVariant)),
          ),
        ],
      ),
    );
  }

  void _cycleGridResolution() {
    final cur = ref.read(settingsProvider).gridResolution;
    final next = cur >= 1 ? 0.5 : cur >= 0.5 ? 0.25 : cur >= 0.25 ? 0.125 : 1.0;
    ref.read(settingsProvider.notifier).setGridResolution(next);
  }

  void _openChordGenerator() {
    final project = ref.read(projectProvider);
    final settings = ref.read(settingsProvider);
    final track = _track;
    final playhead = ref.read(playheadPositionProvider);
    final insertSec =
        playhead > 0.01 ? playhead : _firstNoteStartOrZero(track.notes);
    showDialog<bool>(
      context: context,
      builder: (_) => ChordGeneratorDialog(
        project: project,
        existingNotes: track.notes,
        insertSec: insertSec,
        secPerBeat: project.secondsPerBeat,
        gridResolution: settings.gridResolution,
        snapToGrid: settings.snapToGrid,
        onInsert: (newNotes) {
          _lastGeneratedChordNotes = newNotes;
        },
      ),
    ).then((replaceExisting) {
      if (replaceExisting == null) return; // cancelled
      final generated = _lastGeneratedChordNotes;
      if (generated == null || generated.isEmpty) return;
      _lastGeneratedChordNotes = null;
      // Re-read the track so we merge into the LATEST notes (the dialog may
      // have been open while other edits landed).
      final latest = ref
          .read(projectProvider)
          .tracks
          .where((t) => t.id == widget.trackId)
          .firstOrNull;
      if (latest == null) return;
      final merged =
          replaceExisting ? generated : [...latest.notes, ...generated];
      ref
          .read(projectProvider.notifier)
          .updateTrackNotes(widget.trackId, merged);
      // Immediately re-render + hot-swap (or invalidate the cached WAV when
      // stopped) so the newly generated chords are audible right away in the
      // piano roll instead of only after leaving this page.
      ref.read(playbackProvider.notifier).refreshTrackAudio(widget.trackId);
    });
  }

  List<Note>? _lastGeneratedChordNotes;

  double _firstNoteStartOrZero(List<Note> notes) => notes.isEmpty
      ? 0.0
      : notes.map((n) => n.startTime).reduce((a, b) => a < b ? a : b);

  Widget _buildTransportBar(
      ColorScheme cs, PlaybackState state, double playhead, Project project, double wavProgress) {
    final track = _track;
    return Container(
      height: 48,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(
          top: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          _MiniTransportButton(
            icon: Icons.skip_previous_rounded,
            onTap: () => ref.read(playbackProvider.notifier).seekTo(0),
          ),
          const SizedBox(width: 4),
          _MiniTransportButton(
            icon: state == PlaybackState.playing
                ? Icons.pause_rounded
                : Icons.play_arrow_rounded,
            isPrimary: true,
            onTap: () => ref.read(playbackProvider.notifier)
                .toggle(editingTrackId: widget.trackId),
          ),
          const SizedBox(width: 4),
          _MiniTransportButton(
            icon: Icons.stop_rounded,
            onTap: () => ref.read(playbackProvider.notifier).stop(),
          ),
          const SizedBox(width: 4),
          _MiniTransportButton(
            icon: Icons.skip_next_rounded,
            onTap: () {
              final dur = project.duration > 0 ? project.duration : 60.0;
              ref.read(playbackProvider.notifier).seekTo(dur);
            },
          ),
          const SizedBox(width: 4),
          _MiniTransportButton(
            icon: Icons.headphones_outlined,
            onTap: () => ref.read(projectProvider.notifier).toggleTrackSolo(widget.trackId),
            active: track.isSolo,
            activeColor: const Color(0xFFFFD740),
          ),
          const SizedBox(width: 4),
          if (wavProgress > 0 && wavProgress < 1)
            SizedBox(
              width: 60,
              child: LinearProgressIndicator(
                value: wavProgress,
                backgroundColor: cs.surfaceContainerHigh,
              ),
            ),
          if (wavProgress > 0 && wavProgress < 1) const SizedBox(width: 8),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
            decoration: BoxDecoration(
              color: Theme.of(context).scaffoldBackgroundColor,
              borderRadius: BorderRadius.circular(4),
              border: Border.all(color: Theme.of(context).dividerColor),
            ),
            child: Text(
              '${_formatTime(playhead)} / ${_formatTime(project.duration > 0 ? project.duration : 0)}',
              style: TextStyle(color: cs.primary, fontSize: 13, fontWeight: FontWeight.w500),
            ),
          ),
        ],
      ),
    );
  }

  String _formatTime(double seconds) {
    final m = (seconds ~/ 60).toString().padLeft(2, '0');
    final s = (seconds % 60).toStringAsFixed(1).padLeft(4, '0');
    return '$m:$s';
  }

  Widget _buildBody(BuildContext context, ColorScheme cs, Track track, Project project,
      bool isPlaying, double playhead) {
    return Row(
      children: [
        SizedBox(
          width: 48,
          child: _buildKeyboard(context, cs, track),
        ),
        Container(width: 1, color: Theme.of(context).dividerColor.withAlpha(77)),
        Expanded(child: _buildGridArea(context, cs, track, project, isPlaying, playhead)),
      ],
    );
  }

  static const _majorScale = [0, 2, 4, 5, 7, 9, 11];

  Set<int> _scalePitchClasses(String key) {
    const map = {
      'C': 0, 'C#': 1, 'Db': 1, 'D': 2, 'D#': 3, 'Eb': 3,
      'E': 4, 'F': 5, 'F#': 6, 'Gb': 6, 'G': 7, 'G#': 8,
      'Ab': 8, 'A': 9, 'A#': 10, 'Bb': 10, 'B': 11, 'Cb': 11,
    };
    final root = map[key] ?? 0;
    return _majorScale.map((s) => (root + s) % 12).toSet();
  }

  Widget _buildKeyboard(BuildContext context, ColorScheme cs, Track track) {
    final divColor = Theme.of(context).dividerColor;
    final hasInstrument = track.instrumentName != null;
    final project = ref.read(projectProvider);
    final scalePcs = _scalePitchClasses(project.keySignature);

    return ScrollConfiguration(
      behavior: ScrollConfiguration.of(context).copyWith(scrollbars: false),
      child: SingleChildScrollView(
        controller: _keyVScrollCtrl,
        child: SizedBox(
          height: _noteCount * _noteRowHeight,
          child: Column(
            children: List.generate(_noteCount, (i) {
              final pitch = _maxNote - i;
              final isC = pitch % 12 == 0;
              final isBlack = [1, 3, 6, 8, 10].contains(pitch % 12);
              final pc = pitch % 12;
              final inScale = scalePcs.contains(pc);
              final isTonic = pc == scalePcs.first;
              Color bgColor;
              if (isBlack) {
                bgColor = inScale
                    ? cs.primary.withAlpha(18)
                    : Colors.black26;
              } else {
                bgColor = inScale
                    ? (isTonic ? cs.primary.withAlpha(14) : Colors.transparent)
                    : cs.outlineVariant.withAlpha(10);
              }
              final noteLabels = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
              final isNatural = !isBlack;
              final label = noteLabels[pitch % 12];
              return GestureDetector(
                onTap: hasInstrument ? () => _previewNote(pitch) : null,
                child: Container(
                  height: _noteRowHeight,
                  decoration: BoxDecoration(
                    color: bgColor,
                    border: Border(
                      bottom: BorderSide(color: divColor.withAlpha(38), width: 0.5),
                    ),
                  ),
                  alignment: Alignment.centerRight,
                  padding: const EdgeInsets.only(right: 4),
                  child: Text(
                    isNatural ? '$label${(pitch ~/ 12) - 1}' : '',
                    style: TextStyle(fontSize: 7, color: cs.onSurfaceVariant.withAlpha(isC ? 255 : 153)),
                  ),
                ),
              );
            }),
          ),
        ),
      ),
    );
  }

  Future<void> _previewNote(int pitch) async {
    final track = _track;
    if (track.instrumentName == null || _disposed) return;
    final inst = InstrumentPreset.fromId(track.instrumentName!);
    final wav = _synth.renderPreviewWav(inst, pitch: pitch, duration: 0.9, velocity: 100);

    final dir = await getTemporaryDirectory();
    if (_disposed) return;
    final ts = DateTime.now().millisecondsSinceEpoch;
    final previewId = '__preview_${widget.trackId}_$ts';
    final filePath = '${dir.path}/note_preview_$ts.wav';
    await File(filePath).writeAsBytes(wav);
    if (_disposed) return;

    _stopPreview();
    final audio = ref.read(audioServiceProvider);
    await audio.loadTrackFromPath(previewId, filePath, volume: 1.0);
    if (_disposed) return;
    _previewTrackId = previewId;
    await audio.playSingleTrack(previewId);
    Future.delayed(const Duration(milliseconds: 1000), () {
      if (!_disposed) _stopPreview();
    });
  }

  void _stopPreview() {
    if (_previewTrackId != null) {
      ref.read(audioServiceProvider).stopAndUnloadTrack(_previewTrackId!);
      _previewTrackId = null;
    }
  }

  Widget _buildGridArea(BuildContext context, ColorScheme cs, Track track, Project project,
      bool isPlaying, double playhead) {
    final beatSec = project.secondsPerBeat;
    final totalTime = max(project.duration, 30.0);
    final totalWidth = totalTime * _pps;
    final notes = _localDragNotes ?? track.notes;

    final isMobile = _isMobilePlatform();
    final verticalPhysics = (isMobile && _viewportMode == ViewportMode.edit)
        ? const NeverScrollableScrollPhysics()
        : null;

    return Scrollbar(
      controller: _hScrollCtrl,
      child: SingleChildScrollView(
        controller: _hScrollCtrl,
        scrollDirection: Axis.horizontal,
        child: SizedBox(
          width: totalWidth,
          child: Scrollbar(
            controller: _gridVScrollCtrl,
            child: SingleChildScrollView(
              controller: _gridVScrollCtrl,
              physics: verticalPhysics ?? const ClampingScrollPhysics(),
              child: Listener(
                onPointerDown: _onPointerDown,
                onPointerMove: _onPointerMove,
                onPointerUp: _onPointerUp,
                onPointerSignal: _onPointerSignal,
                behavior: HitTestBehavior.translucent,
                child: GestureDetector(
                  onTapUp: (details) => _onTapUp(details, track, project),
                  onPanStart: (details) => _onPanStart(details, track),
                  onPanUpdate: (details) => _onPanUpdate(details),
                  onPanEnd: (details) => _onPanEnd(track),
                  child: CustomPaint(
                    size: Size(totalWidth, _noteCount * _noteRowHeight),
                    painter: _PianoRollEditorPainter(
                      notes: notes,
                      pps: _pps,
                      noteRowHeight: _noteRowHeight,
                      minNote: _minNote,
                      maxNote: _maxNote,
                      beatSec: beatSec,
                      timeSigNum: project.timeSignatureNumerator,
                      gridColor: const Color(0xFF2A2A2A),
                      beatColor: const Color(0xFF3A3A3A),
                      barColor: const Color(0xFF00AAFF).withAlpha(38),
                      noteColor: track.color,
                      selectedIndices: _selectedIndices,
                      dragNoteIndex: _dragNoteIndex,
                      accentColor: AppColors.accent,
                      ghostRects: _ghostRects,
                      playPos: isPlaying ? playhead : null,
                      playheadColor: cs.primary,
                      scalePcs: _scalePitchClasses(project.keySignature),
                    selectionRect: _selectionRect,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  // ── Viewport pan / pinch zoom / scroll-wheel zoom ──

  void _onPointerDown(PointerDownEvent event) {
    _activePointers++;
    _pointerPos[event.pointer] = event.localPosition;

    if ((event.buttons & 4) != 0) {
      _startPan(event.localPosition);
    } else if (_activePointers == 2) {
      _dragNoteIndex = null;
      _isResizing = false;
      _dragStartX = null;
      _dragStartY = null;
      _dragOriginNotes = null;
      _localDragNotes = null;
      _ghostRects = [];
      _selectionRect = null;
      _selectionStart = null;
      _startPan(event.localPosition);
      final pts = _pointerPos.values.toList();
      _pinchStartDist = (pts[1] - pts[0]).distance;
      _pinchStartZoom = _zoomLevel;
    } else if (_activePointers > 2) {
      _startPan(event.localPosition);
    } else if (_viewportMode == ViewportMode.scroll) {
      _startPan(event.localPosition);
    } else if (_isMobilePlatform() && _activePointers == 1) {
      // On mobile in edit/select mode, single finger is for notes, not pan
    }
  }

  void _onPointerMove(PointerMoveEvent event) {
    _pointerPos[event.pointer] = event.localPosition;

    if (_isPanning && _panStartX != null && _panStartY != null) {
      if (_activePointers >= 2 && _pinchStartDist != null && _pinchStartDist! > 0 && _pinchStartZoom != null && _pointerPos.length >= 2) {
        final pts = _pointerPos.values.toList();
        final dist = (pts[0] - pts[1]).distance;
        final newZoom = (_pinchStartZoom! * (dist / _pinchStartDist!)).clamp(0.5, 10.0);
        final ratio = newZoom / _pinchStartZoom!;
        if (ratio != 1.0) {
          // Focal point anchoring: keep content under pinch center stationary
          final focus = (pts[0] + pts[1]) / 2;
          if (_hScrollCtrl.hasClients) {
            final offset = _hScrollCtrl.offset;
            _hScrollCtrl.jumpTo((offset + focus.dx) * ratio - focus.dx);
          }
          if (_gridVScrollCtrl.hasClients) {
            final offset = _gridVScrollCtrl.offset;
            _gridVScrollCtrl.jumpTo((offset + focus.dy) * ratio - focus.dy);
          }
        }
        _safeSetState(() => _zoomLevel = newZoom);
      }
      final dx = event.localPosition.dx - _panStartX!;
      final dy = event.localPosition.dy - _panStartY!;
      if (_hScrollCtrl.hasClients) {
        _hScrollCtrl.jumpTo((_panOffX! - dx).clamp(0, _hScrollCtrl.position.maxScrollExtent));
      }
      if (_gridVScrollCtrl.hasClients) {
        _gridVScrollCtrl.jumpTo((_panOffY! - dy).clamp(0, _gridVScrollCtrl.position.maxScrollExtent));
      }
    }
  }

  void _onPointerUp(PointerUpEvent event) {
    _activePointers = max(0, _activePointers - 1);
    _pointerPos.remove(event.pointer);
    if (_activePointers < 2) {
      _isPanning = false;
      _pinchStartDist = null;
      _pinchStartZoom = null;
      // Clear selection rect if pan gesture stole it before onPanEnd
      if (_viewportMode == ViewportMode.scroll) {
        _selectionRect = null;
        _selectionStart = null;
      }
    }
  }

  void _onPointerSignal(PointerSignalEvent event) {
    if (event is PointerScrollEvent && _isCtrlDown) {
      final localPos = event.localPosition;
      final oldZoom = _zoomLevel;
      final newZoom = (oldZoom * (event.scrollDelta.dy < 0 ? 1.15 : 0.85)).clamp(0.5, 10.0);
      final ratio = newZoom / oldZoom;
      if (_hScrollCtrl.hasClients) {
        final offset = _hScrollCtrl.offset;
        _hScrollCtrl.jumpTo((offset + localPos.dx) * ratio - localPos.dx);
      }
      if (_gridVScrollCtrl.hasClients) {
        final offset = _gridVScrollCtrl.offset;
        _gridVScrollCtrl.jumpTo((offset + localPos.dy) * ratio - localPos.dy);
      }
      _safeSetState(() => _zoomLevel = newZoom);
    }
  }

  void _startPan(Offset localPos) {
    _isPanning = true;
    _panStartX = localPos.dx;
    _panStartY = localPos.dy;
    _panOffX = _hScrollCtrl.hasClients ? _hScrollCtrl.offset : 0;
    _panOffY = _gridVScrollCtrl.hasClients ? _gridVScrollCtrl.offset : 0;
  }

  // ── Tap / Drag ──

  void _onTapUp(TapUpDetails details, Track track, Project project) {
    if (_isPanning) return;
    final localPos = details.localPosition;
    final idx = _noteIndexAt(localPos, track.notes);

    if (_viewportMode == ViewportMode.select) {
      if (idx >= 0) {
        _safeSetState(() {
          if (_selectedIndices.contains(idx)) _selectedIndices.remove(idx);
          else _selectedIndices.add(idx);
        });
      } else {
        _safeSetState(() => _selectedIndices.clear());
      }
    } else if (_viewportMode == ViewportMode.edit) {
      if (idx >= 0) {
        final updated = List<Note>.from(track.notes)..removeAt(idx);
        ref.read(projectProvider.notifier).updateTrackNotes(widget.trackId, updated);
      } else {
        _addNoteAt(localPos, track, project);
      }
    }
    // In scroll mode, taps do nothing
  }

  void _onPanStart(DragStartDetails details, Track track) {
    if (_isPanning) return;
    final localPos = details.localPosition;

    // Scroll mode → handled by _onPointerDown single-finger pan
    if (_viewportMode == ViewportMode.scroll) return;

    if (_viewportMode == ViewportMode.select) {
      final idx = _noteIndexAt(localPos, track.notes);
      if (idx >= 0) {
        // Drag selected notes
        if (!_selectedIndices.contains(idx)) {
          _safeSetState(() => _selectedIndices.add(idx));
        }
        _dragOriginNotes = track.notes.map((n) => n.copyWith()).toList();
        _localDragNotes = List<Note>.from(track.notes);
        _ghostRects = [];
        _dragNoteIndex = idx;
        _dragStartX = localPos.dx;
        _dragStartY = localPos.dy;
        _isResizing = false;
      } else {
        // Start rectangle selection
        _selectionStart = localPos;
        _selectionRect = Rect.fromPoints(localPos, localPos);
      }
      return;
    }

    // Edit mode
    final idx = _noteIndexAt(localPos, track.notes);
    if (idx < 0) return;

    _dragOriginNotes = track.notes.map((n) => n.copyWith()).toList();
    _localDragNotes = List<Note>.from(track.notes);
    _ghostRects = [];

    final n = track.notes[idx];
    final noteRightX = _timeToX(n.startTime + n.duration);
    if ((localPos.dx - noteRightX).abs() < 8.0) {
      _dragNoteIndex = idx;
      _dragStartX = localPos.dx;
      _isResizing = true;
    } else {
      _dragNoteIndex = idx;
      _dragStartX = localPos.dx;
      _dragStartY = localPos.dy;
      _isResizing = false;
    }
  }

  void _onPanUpdate(DragUpdateDetails details) {
    if (_isPanning) return;
    final curX = details.localPosition.dx;
    final curY = details.localPosition.dy;

    // Selection rectangle in progress
    if (_viewportMode == ViewportMode.select && _selectionStart != null) {
      _safeSetState(() {
        _selectionRect = Rect.fromPoints(_selectionStart!, Offset(curX, curY));
      });
      return;
    }

    if (_dragNoteIndex == null || _dragOriginNotes == null || _localDragNotes == null) return;

    _ghostRects = [];

    if (_isResizing) {
      final orig = _dragOriginNotes![_dragNoteIndex!];
      final rawDelta = _xToTime(curX) - _xToTime(_dragStartX!);
      final newDur = max(0.125, _snapTime(orig.duration + rawDelta));
      _localDragNotes![_dragNoteIndex!] = orig.copyWith(duration: newDur);
      _ghostRects.add(Rect.fromLTWH(
        orig.startTime * _pps, _pitchToY(orig.pitch), newDur * _pps, _noteRowHeight - 1));
    } else {
      final rawDeltaTime = _xToTime(curX) - _xToTime(_dragStartX!);
      final rawDeltaPitch = (_dragStartY! - curY) / _noteRowHeight;
      final deltaPitch = rawDeltaPitch.round();
      if (rawDeltaTime == 0 && deltaPitch == 0) return;

      final indices = _viewportMode == ViewportMode.select ? _selectedIndices.toList() : [_dragNoteIndex!];
      for (final i in indices) {
        if (i < 0 || i >= _localDragNotes!.length) continue;
        if (i >= _dragOriginNotes!.length) continue;
        final orig = _dragOriginNotes![i];
        final snapTime = max(0.0, _snapTime(orig.startTime + rawDeltaTime));
        final snapPitch = (orig.pitch + deltaPitch).clamp(_minNote, _maxNote);
        _localDragNotes![i] = orig.copyWith(startTime: snapTime, pitch: snapPitch);
        _ghostRects.add(Rect.fromLTWH(
          snapTime * _pps, _pitchToY(snapPitch),
          orig.duration * _pps, _noteRowHeight - 1));
      }
    }
    _safeSetState(() {});
  }

  Rect _normalizeRect(Rect r) {
    return Rect.fromLTRB(
      min(r.left, r.right), min(r.top, r.bottom),
      max(r.left, r.right), max(r.top, r.bottom));
  }

  void _onPanEnd(Track track) {
    if (_localDragNotes != null) {
      ref.read(projectProvider.notifier).updateTrackNotes(widget.trackId, _localDragNotes!);
    }

    // Finalize rectangle selection
    if (_viewportMode == ViewportMode.select && _selectionRect != null) {
      final r = _normalizeRect(_selectionRect!);
      for (int i = 0; i < track.notes.length; i++) {
        final n = track.notes[i];
        final noteRect = Rect.fromLTWH(
          _timeToX(n.startTime), _pitchToY(n.pitch),
          _timeToX(n.duration), _noteRowHeight);
        if (r.overlaps(noteRect)) {
          _selectedIndices.add(i);
        }
      }
    }

    _safeSetState(() {
      _dragNoteIndex = null;
      _isResizing = false;
      _dragStartX = null;
      _dragStartY = null;
      _dragOriginNotes = null;
      _localDragNotes = null;
      _ghostRects = [];
      _selectionRect = null;
      _selectionStart = null;
    });
  }

  void _addNoteAt(Offset localPos, Track track, Project project) {
    final pitch = _yToPitch(localPos.dy);
    final t = _snapTime(_xToTime(localPos.dx));
    if (t.isNaN || t.isInfinite) return;
    final newNote = Note(pitch: pitch, startTime: t, duration: 0.5, velocity: 100);
    final updated = List<Note>.from(track.notes)
      ..add(newNote)
      ..sort((a, b) => a.startTime.compareTo(b.startTime));
    ref.read(projectProvider.notifier).updateTrackNotes(widget.trackId, updated);
  }

  int _noteIndexAt(Offset localPos, List<Note> notes) {
    for (int i = notes.length - 1; i >= 0; i--) {
      final n = notes[i];
      final y = _pitchToY(n.pitch);
      final x = _timeToX(n.startTime);
      final w = _timeToX(n.duration);
      final h = _noteRowHeight;
      if (localPos.dx >= x && localPos.dx <= x + w &&
          localPos.dy >= y && localPos.dy <= y + h) {
        return i;
      }
    }
    return -1;
  }
}

// ─────────────────── Tool Chip ───────────────────

class _ToolChip extends StatelessWidget {
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  const _ToolChip({required this.icon, required this.label, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        decoration: BoxDecoration(
          color: cs.surfaceContainerHigh,
          borderRadius: BorderRadius.circular(6),
          border: Border.all(color: cs.outlineVariant.withAlpha(128)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 14, color: cs.onSurfaceVariant),
            if (label.isNotEmpty) ...[
              const SizedBox(width: 4),
              Text(label, style: TextStyle(fontSize: 10, color: cs.onSurfaceVariant)),
            ],
          ],
        ),
      ),
    );
  }
}

// ─────────────────── Mini Transport Button ───────────────────

class _MiniTransportButton extends StatelessWidget {
  final IconData icon;
  final VoidCallback? onTap;
  final bool isPrimary;
  final bool active;
  final Color? activeColor;

  const _MiniTransportButton({
    required this.icon,
    this.onTap,
    this.isPrimary = false,
    this.active = false,
    this.activeColor,
  });

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final bool activated = active;
    final Color bgColor;
    if (activated) {
      bgColor = activeColor ?? cs.primary;
    } else if (isPrimary) {
      bgColor = cs.primary;
    } else {
      bgColor = Colors.transparent;
    }
    return Material(
      color: bgColor,
      borderRadius: BorderRadius.circular(isPrimary ? 16 : 4),
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(isPrimary ? 16 : 4),
        child: Container(
          width: 32, height: 32,
          alignment: Alignment.center,
          child: Icon(
            icon, size: 20,
            color: activated || isPrimary
                ? Theme.of(context).scaffoldBackgroundColor
                : cs.onSurfaceVariant,
          ),
        ),
      ),
    );
  }
}

// ─────────────────── Painter ───────────────────

class _PianoRollEditorPainter extends CustomPainter {
  final List<Note> notes;
  final double pps;
  final double noteRowHeight;
  final int minNote;
  final int maxNote;
  final double beatSec;
  final int timeSigNum;
  final Color gridColor;
  final Color beatColor;
  final Color barColor;
  final Color noteColor;
  final Set<int> selectedIndices;
  final int? dragNoteIndex;
  final Color accentColor;
  final List<Rect> ghostRects;
  final double? playPos;
  final Color playheadColor;
  final Set<int> scalePcs;
  final Rect? selectionRect;

  _PianoRollEditorPainter({
    required this.notes,
    required this.pps,
    required this.noteRowHeight,
    required this.minNote,
    required this.maxNote,
    required this.beatSec,
    required this.timeSigNum,
    required this.gridColor,
    required this.beatColor,
    required this.barColor,
    required this.noteColor,
    this.selectedIndices = const {},
    this.dragNoteIndex,
    required this.accentColor,
    this.ghostRects = const [],
    this.playPos,
    required this.playheadColor,
    this.scalePcs = const {},
    this.selectionRect,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()..strokeWidth = 0.5;

    for (int p = minNote; p <= maxNote; p++) {
      final pc = p % 12;
      final inScale = scalePcs.contains(pc);
      final isTonic = scalePcs.isNotEmpty && pc == scalePcs.first;
      if (inScale) {
        canvas.drawRect(
          Rect.fromLTWH(0, _pitchToY(p), size.width, noteRowHeight),
          Paint()..color = (isTonic ? Colors.white : Colors.white).withAlpha(isTonic ? 14 : 8),
        );
      } else {
        canvas.drawRect(
          Rect.fromLTWH(0, _pitchToY(p), size.width, noteRowHeight),
          Paint()..color = Colors.black.withAlpha(6),
        );
      }
    }

    final barSec = beatSec * timeSigNum;
    for (double t = 0; t < size.width / pps; t += beatSec) {
      final x = t * pps;
      if (x > size.width) break;
      final isBar = (t / barSec).round() * barSec == t && t > 0;
      paint.color = isBar ? barColor : beatColor;
      paint.strokeWidth = isBar ? 1.5 : 0.5;
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), paint);
    }

    paint.strokeWidth = 0.5;
    paint.color = gridColor;
    for (int i = 0; i <= (maxNote - minNote); i++) {
      if (i % 12 == 0) continue;
      canvas.drawLine(Offset(0, i * noteRowHeight), Offset(size.width, i * noteRowHeight), paint);
    }

    for (final r in ghostRects) {
      final ghostRRect = RRect.fromRectAndRadius(r, const Radius.circular(2));
      canvas.drawRRect(ghostRRect, Paint()..color = noteColor.withAlpha(40));
      canvas.drawRRect(ghostRRect, Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = noteColor.withAlpha(80));
    }

    for (int i = 0; i < notes.length; i++) {
      final n = notes[i];
      if (n.pitch < minNote || n.pitch > maxNote) continue;

      final x = n.startTime * pps;
      final w = n.duration * pps;
      final y = _pitchToY(n.pitch);
      final h = noteRowHeight - 1;

      final isDragged = i == dragNoteIndex;
      final isSelected = selectedIndices.contains(i) || isDragged;
      final color = isSelected ? accentColor : noteColor;

      final rrect = RRect.fromRectAndRadius(Rect.fromLTWH(x, y + 0.5, w, h), const Radius.circular(2));

      if (isDragged) {
        final shadowRect = RRect.fromRectAndRadius(
          Rect.fromLTWH(x + 3, y + 2.5, w, h), const Radius.circular(2));
        canvas.drawRRect(shadowRect, Paint()..color = Colors.black.withAlpha(60));
        canvas.drawRRect(rrect, Paint()..color = accentColor.withAlpha(220));
        canvas.drawRRect(rrect, Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2
          ..color = accentColor);
      } else {
        canvas.drawRRect(rrect, Paint()..color = color.withAlpha(180));
        canvas.drawRRect(rrect, Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = isSelected ? 2 : 1
          ..color = color);
      }
    }

    if (selectionRect != null) {
      final r = Rect.fromLTRB(
        min(selectionRect!.left, selectionRect!.right),
        min(selectionRect!.top, selectionRect!.bottom),
        max(selectionRect!.left, selectionRect!.right),
        max(selectionRect!.top, selectionRect!.bottom));
      canvas.drawRect(r, Paint()..color = accentColor.withAlpha(30));
      canvas.drawRect(r, Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.5
        ..color = accentColor.withAlpha(180));
    }

    if (playPos != null) {
      final phx = playPos! * pps;
      paint.color = playheadColor;
      paint.strokeWidth = 2;
      canvas.drawLine(Offset(phx, 0), Offset(phx, size.height), paint);
    }
  }

  double _pitchToY(int pitch) => (maxNote - pitch) * noteRowHeight;

  @override
  bool shouldRepaint(covariant _PianoRollEditorPainter oldDelegate) =>
      oldDelegate.notes != notes ||
      oldDelegate.pps != pps ||
      oldDelegate.noteRowHeight != noteRowHeight ||
      oldDelegate.selectedIndices != selectedIndices ||
      oldDelegate.dragNoteIndex != dragNoteIndex ||
      oldDelegate.ghostRects != ghostRects ||
      oldDelegate.playPos != playPos ||
      oldDelegate.selectionRect != selectionRect;
}

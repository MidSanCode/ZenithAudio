import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:uuid/uuid.dart';
import '../../models/audio_clip.dart';
import '../../models/track.dart';

import '../../providers/project_provider.dart';
import '../../providers/playback_provider.dart';
import '../layout/floating_window.dart';
import '../../services/audio_service.dart';
import '../../services/audio_converter.dart';
import '../../services/fft_service.dart';
import '../../services/audio_processing_service.dart';
import '../../core/utils/logger.dart';
import 'waveform_painter.dart';
import 'spectrogram_painter.dart';
import 'generator_panel.dart';
import 'waveform_cards.dart';
import 'effect_dialog.dart';
import 'frequency_split_dialog.dart';
import 'audio_clip_toolbar.dart';
import '../../models/waveform_drop_data.dart';
import '../../services/waveform_generator.dart';

/// Open the audio clip editor for a given track.
/// Desktop: full-screen dialog. Mobile: Navigator push.
Future<void> openAudioClipEditor(BuildContext context, String trackId) {
  if (defaultTargetPlatform == TargetPlatform.android ||
      defaultTargetPlatform == TargetPlatform.iOS) {
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => AudioClipEditor(trackId: trackId),
      ),
    );
  }
  return showDialog(
    context: context,
    barrierDismissible: false,
    barrierColor: Colors.black87,
    builder: (_) => Dialog(
      insetPadding: EdgeInsets.zero,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(8),
        child: AudioClipEditor(trackId: trackId),
      ),
    ),
  );
}

/// Open the audio clip editor for a new generated track.
Future<void> openAudioClipEditorForNew(
    BuildContext context, Float64List samples, int sampleRate, String genType) {
  if (defaultTargetPlatform == TargetPlatform.android ||
      defaultTargetPlatform == TargetPlatform.iOS) {
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => AudioClipEditor(
          initialSamples: samples,
          initialSampleRate: sampleRate,
          initialGenType: genType,
        ),
      ),
    );
  }
  return showDialog(
    context: context,
    barrierDismissible: false,
    barrierColor: Colors.black87,
    builder: (_) => Dialog(
      insetPadding: EdgeInsets.zero,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(8),
        child: AudioClipEditor(
          initialSamples: samples,
          initialSampleRate: sampleRate,
          initialGenType: genType,
        ),
      ),
    ),
  );
}

class AudioClipEditor extends ConsumerStatefulWidget {
  final String? trackId;
  final Float64List? initialSamples;
  final int? initialSampleRate;
  final String? initialGenType;
  final bool isFloating;

  const AudioClipEditor({
    super.key,
    this.trackId,
    this.initialSamples,
    this.initialSampleRate,
    this.initialGenType,
    this.isFloating = false,
  });

  @override
  ConsumerState<AudioClipEditor> createState() => _AudioClipEditorState();
}

class _AudioClipEditorState extends ConsumerState<AudioClipEditor> {
  Track? _track;
  AudioClip? _clip;
  bool _loading = true;
  bool _disposed = false;
  String? _loadError;

  double _zoom = 2.0;
  double get _pps => (_basePps * _zoom).clamp(20, 600);
  static const double _basePps = 40;

  final ScrollController _hScrollCtrl = ScrollController();
  double _playheadSec = 0;
  bool _isPlaying = false;
  bool _showGenerator = false;
  bool _hasUnsavedChanges = false;

  bool _showSpectrogram = false;

  // Pending waveform drop settings
  WaveformDropData? _pendingDropData;
  bool _showDropSettings = false;
  double _dropFrequency = 440;
  double _dropDuration = 2.0;
  double _dropAmplitude = 0.8;
  double _dropMixLevel = 0.5;

  // AudioClip-level undo/redo
  static const int _maxUndo = 30;
  final List<Float64List> _sampleUndoStack = [];
  final List<Float64List> _sampleRedoStack = [];

  void _pushSampleUndo() {
    if (_clip == null) return;
    _sampleUndoStack.add(Float64List.fromList(_clip!.samples));
    if (_sampleUndoStack.length > _maxUndo) _sampleUndoStack.removeAt(0);
    _sampleRedoStack.clear();
  }

  void _undoSample() {
    if (_clip == null || _sampleUndoStack.isEmpty) return;
    _sampleRedoStack.add(Float64List.fromList(_clip!.samples));
    _clip = _clip!.copyWith(samples: _sampleUndoStack.removeLast());
    _safeSetState(() {});
  }

  void _redoSample() {
    if (_clip == null || _sampleRedoStack.isEmpty) return;
    _sampleUndoStack.add(Float64List.fromList(_clip!.samples));
    _clip = _clip!.copyWith(samples: _sampleRedoStack.removeLast());
    _safeSetState(() {});
  }

  bool get _canUndoSample => _sampleUndoStack.isNotEmpty;
  bool get _canRedoSample => _sampleRedoStack.isNotEmpty;

  // Selection
  double? _dragStartSec;
  double? _dragCurrentSec;

  @override
  void initState() {
    super.initState();
    _loadAudio();
  }

  @override
  void dispose() {
    _disposed = true;
    _hScrollCtrl.dispose();
    if (_hasUnsavedChanges && _clip != null && widget.trackId != null) {
      try {
        final samples = Float64List.fromList(_clip!.samples);
        final path = _saveToWavSync(samples, prefix: 'track_${widget.trackId}');
        ref.read(projectProvider.notifier).setTrackAudioFile(widget.trackId!, path);
      } catch (_) {}
    }
    super.dispose();
  }

  void _safeSetState(VoidCallback fn) {
    if (!_disposed && mounted) setState(fn);
  }

  Future<void> _loadAudio() async {
    if (widget.initialSamples != null) {
      _clip = AudioClip(
        samples: widget.initialSamples!,
        sampleRate: widget.initialSampleRate ?? 44100,
        genParams: widget.initialGenType != null
            ? WaveformGenParams(type: widget.initialGenType!, frequency: 440)
            : null,
      );
      _loadError = null;
      _safeSetState(() => _loading = false);
      return;
    }

    if (widget.trackId == null) {
      _safeSetState(() => _loading = false);
      return;
    }

    final project = ref.read(projectProvider);
    final track = project.tracks.where((t) => t.id == widget.trackId).firstOrNull;
    if (track == null) {
      _safeSetState(() => _loading = false);
      return;
    }
    _track = track;

    if (track.audioFilePath != null && await File(track.audioFilePath!).exists()) {
      final originalPath = track.audioFilePath!;
      var samples = await _readWavFile(originalPath);
      var sourceFile = originalPath;
      if (samples == null) {
        final converted = await convertToWav(originalPath);
        if (converted != null) {
          sourceFile = converted;
          samples = await _readWavFile(sourceFile);
        }
      }
      if (samples != null) {
        _clip = AudioClip(samples: samples, sampleRate: widget.initialSampleRate ?? 44100, sourceFile: sourceFile);
        _loadError = null;
      } else {
        _loadError = 'audioClip.loadError'.tr();
      }
    } else {
      _loadError = 'audioClip.fileNotFound'.tr();
    }

    _safeSetState(() => _loading = false);
  }

  /// Read PCM samples from a WAV file.
  Future<Float64List?> _readWavFile(String path) async {
    try {
      final file = File(path);
      final bytes = await file.readAsBytes();
      if (bytes.length < 12) return null;

      // RIFF header: "RIFF" + fileSize + "WAVE"
      final riff = String.fromCharCodes(bytes.sublist(0, 4));
      if (riff != 'RIFF') return null;
      final wave = String.fromCharCodes(bytes.sublist(8, 12));
      if (wave != 'WAVE') return null;

      // Parse sub-chunks to find "fmt " and "data"
      int channels = 1;
      int bitsPerSample = 16;
      int dataOffset = 0;
      int dataSize = 0;
      int offset = 12;
      while (offset + 8 <= bytes.length) {
        final chunkId = String.fromCharCodes(bytes.sublist(offset, offset + 4));
        final chunkSize = ByteData.sublistView(bytes, offset + 4, offset + 8).getUint32(0, Endian.little);
        if (chunkId == 'fmt ') {
          if (chunkSize >= 16) {
            final audioFormat = ByteData.sublistView(bytes, offset + 8, offset + 10).getUint16(0, Endian.little);
            if (audioFormat != 1) return null; // Only PCM supported
            channels = ByteData.sublistView(bytes, offset + 10, offset + 12).getUint16(0, Endian.little);
            bitsPerSample = ByteData.sublistView(bytes, offset + 22, offset + 24).getUint16(0, Endian.little);
          }
        } else if (chunkId == 'data') {
          dataOffset = offset + 8;
          dataSize = chunkSize.toInt();
        }
        offset += 8 + chunkSize + (chunkSize % 2); // Skip padding byte if odd-sized
      }

      if (dataSize <= 0) return null;

      final int bytesPerSample = bitsPerSample ~/ 8;
      final int totalSamples = dataSize ~/ bytesPerSample;
      final int numSamples = totalSamples ~/ channels; // De-interleave to mono
      final result = Float64List(numSamples);

      if (bitsPerSample == 16) {
        for (int i = 0; i < numSamples; i++) {
          final srcIdx = dataOffset + i * channels * 2;
          final int sample = ByteData.sublistView(bytes, srcIdx, srcIdx + 2).getInt16(0, Endian.little);
          result[i] = sample / 32768.0;
        }
      } else if (bitsPerSample == 24) {
        for (int i = 0; i < numSamples; i++) {
          final srcIdx = dataOffset + i * channels * 3;
          final int b0 = bytes[srcIdx];
          final int b1 = bytes[srcIdx + 1];
          final int b2 = bytes[srcIdx + 2];
          int sample = b0 | (b1 << 8) | (b2 << 16);
          if (sample >= 0x800000) sample -= 0x1000000;
          result[i] = sample / 8388608.0;
        }
      } else if (bitsPerSample == 32) {
        for (int i = 0; i < numSamples; i++) {
          final srcIdx = dataOffset + i * channels * 4;
          final int sample = ByteData.sublistView(bytes, srcIdx, srcIdx + 4).getInt32(0, Endian.little);
          result[i] = sample / 2147483648.0;
        }
      } else {
        AppLogger.w('Unsupported WAV bit depth: $bitsPerSample');
        return null;
      }
      return result;
    } catch (e) {
      AppLogger.e('Failed to read WAV', e);
      return null;
    }
  }

  /// Save clip to WAV file and update track.
  Future<String> _saveToWav(Float64List samples, {String? prefix}) async {
    final dir = Directory.systemTemp;
    final name = prefix ?? 'clip_${const Uuid().v4().substring(0, 8)}';
    final path = '${dir.path}/$name.wav';
    final wav = _encodeWav(samples);
    await File(path).writeAsBytes(wav);
    return path;
  }

  /// Synchronous version for use in dispose().
  String _saveToWavSync(Float64List samples, {String? prefix}) {
    final dir = Directory.systemTemp;
    final name = prefix ?? 'clip_${const Uuid().v4().substring(0, 8)}';
    final path = '${dir.path}/$name.wav';
    final wav = _encodeWav(samples);
    File(path).writeAsBytesSync(wav);
    return path;
  }

  Uint8List _encodeWav(Float64List buffer) {
    final numSamples = buffer.length;
    final sampleRate = 44100;
    final bytesPerSample = 2;
    final dataSize = numSamples * bytesPerSample;
    final fileSize = 44 + dataSize;
    final data = List<int>.filled(fileSize, 0);
    int offset = 0;
    void w4(int v) {
      data[offset] = v & 0xFF; data[offset + 1] = (v >> 8) & 0xFF;
      data[offset + 2] = (v >> 16) & 0xFF; data[offset + 3] = (v >> 24) & 0xFF;
      offset += 4;
    }
    void w2(int v) {
      data[offset] = v & 0xFF; data[offset + 1] = (v >> 8) & 0xFF;
      offset += 2;
    }
    void ws(String s) {
      for (int i = 0; i < s.length; i++) data[offset++] = s.codeUnitAt(i);
    }
    ws('RIFF'); w4(fileSize - 8); ws('WAVE');
    ws('fmt '); w4(16); w2(1); w2(1); w4(sampleRate);
    w4(sampleRate * bytesPerSample); w2(bytesPerSample); w2(16);
    ws('data'); w4(dataSize);
    for (int i = 0; i < numSamples; i++) {
      final clamped = buffer[i].clamp(-1.0, 1.0);
      final sample = (clamped * 32767).round().clamp(-32768, 32767);
      w2(sample);
    }
    return Uint8List.fromList(data);
  }

  Future<String?> _trySave() async {
    if (!_hasUnsavedChanges || _clip == null || widget.trackId == null) return null;
    final samples = Float64List.fromList(_clip!.samples);
    final trackId = widget.trackId!;
    final path = await _saveToWav(samples, prefix: 'track_$trackId');
    if (!mounted) return null;
    ref.read(projectProvider.notifier).setTrackAudioFile(trackId, path);
    return path;
  }

  Future<void> _closeEditor() async {
    await _trySave();
    if (widget.isFloating) {
      final closeFn = FloatingWindowCloseScope.of(context);
      if (closeFn != null) {
        closeFn();
        return;
      }
    }
    if (mounted) Navigator.of(context).pop();
  }

  void _togglePlayback() {
    final notifier = ref.read(playbackProvider.notifier);
    if (_isPlaying) {
      notifier.stop();
      _safeSetState(() => _isPlaying = false);
    } else {
      // Load the track into audio service and play
      _startPlayback();
    }
  }

  Future<void> _startPlayback() async {
    if (_clip == null) return;
    final audio = ref.read(audioServiceProvider);
    await audio.unloadAll();

    // Generate a WAV for the current clip
    final path = await _saveToWav(_clip!.samples, prefix: 'editor_play');
    await audio.loadTrackFromPath('editor_play', path, volume: 1.0);

    audio.onPositionChanged = (pos) {
      _safeSetState(() => _playheadSec = pos);
      // Auto-scroll to follow playhead
      if (_hScrollCtrl.hasClients) {
        final viewportWidth = _hScrollCtrl.position.viewportDimension;
        final playheadX = pos * _pps;
        if (playheadX < _hScrollCtrl.offset || playheadX > _hScrollCtrl.offset + viewportWidth * 0.8) {
          _hScrollCtrl.jumpTo(max(0, playheadX - viewportWidth * 0.2));
        }
      }
    };
    audio.onCompleted = () {
      _safeSetState(() => _isPlaying = false);
    };

    await audio.play();
    _safeSetState(() => _isPlaying = true);
  }

  void _stopPlayback() {
    ref.read(audioServiceProvider).stop();
    _safeSetState(() {
      _isPlaying = false;
      _playheadSec = 0;
    });
  }

  void _onWaveformTap(TapUpDetails details) {
    if (_clip == null) return;
    if (_showDropSettings) {
      _applyDropSettings();
      return;
    }
    final sec = (details.localPosition.dx + _hScrollCtrl.offset) / _pps;
    _safeSetState(() => _playheadSec = sec.clamp(0, _clip!.duration));
  }

  void _onWaveformPanStart(DragStartDetails details) {
    if (_clip == null) return;
    final sec = (details.localPosition.dx + _hScrollCtrl.offset) / _pps;
    _safeSetState(() {
      _dragStartSec = sec.clamp(0, _clip!.duration);
      _dragCurrentSec = _dragStartSec;
    });
  }

  void _onWaveformPanUpdate(DragUpdateDetails details) {
    if (_clip == null || _dragStartSec == null) return;
    final sec = (details.localPosition.dx + _hScrollCtrl.offset) / _pps;
    _safeSetState(() {
      _dragCurrentSec = sec.clamp(0, _clip!.duration);
      final start = min(_dragStartSec!, _dragCurrentSec!);
      final end = max(_dragStartSec!, _dragCurrentSec!);
      _clip!.selection = Selection(startSec: start, endSec: end);
    });
  }

  void _onWaveformPanEnd(DragEndDetails details) {
    // Selection is already set
  }

  void _clearSelection() {
    _safeSetState(() {
      _clip?.selection = null;
      _dragStartSec = null;
      _dragCurrentSec = null;
    });
  }

  void _applyToSelection(Float64List Function(Float64List) processor) {
    if (_clip?.selection == null || !_clip!.selection!.isValid) return;
    final sel = _clip!.selection!;
    final startSample = (sel.startSec * _clip!.sampleRate).round();
    final endSample = (sel.endSec * _clip!.sampleRate).round().clamp(0, _clip!.samples.length);
    final region = _clip!.samples.sublist(startSample, endSample);
    final processed = processor(Float64List.fromList(region));
    for (int i = 0; i < processed.length && startSample + i < _clip!.samples.length; i++) {
      _clip!.samples[startSample + i] = processed[i];
    }
    _safeSetState(() {});
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;

    if (_loading) {
      return Scaffold(
        backgroundColor: cs.surface,
        body: const Center(child: CircularProgressIndicator()),
      );
    }

    return Scaffold(
      backgroundColor: cs.surface,
      body: Column(
        children: [
          if (!widget.isFloating) _buildTitleBar(cs),
          if (_clip != null)
            AudioClipToolbar(
              canUndo: _canUndoSample,
              canRedo: _canRedoSample,
              onUndo: _undoSample,
              onRedo: _redoSample,
              onProcess: _handleProcess,
              onFrequencySplit: () => _handleSplit(context),
              showGenerator: _showGenerator,
              onToggleGenerator: () => _safeSetState(() => _showGenerator = !_showGenerator),
              showSpectrogram: _showSpectrogram,
              onToggleSpectrogram: () => _safeSetState(() => _showSpectrogram = !_showSpectrogram),
              zoom: _zoom,
              onZoomChanged: (v) => _safeSetState(() => _zoom = v),
            ),
          Expanded(
            child: ClipRect(
              child: Stack(
                children: [
                  if (_clip != null)
                    Column(
                      children: [
                        Expanded(
                          child: Row(
                            children: [
                              Expanded(child: _buildWaveformArea(cs)),
                              _buildWaveformCards(cs),
                            ],
                          ),
                        ),
                      ],
                    )
                  else
                    Center(
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Icon(Icons.audio_file_outlined, size: 48, color: cs.onSurfaceVariant),
                          const SizedBox(height: 8),
                          Text(_loadError ?? 'audioClip.noAudioData'.tr(),
                              style: TextStyle(color: _loadError != null ? cs.error : cs.onSurfaceVariant)),
                          const SizedBox(height: 16),
                          FilledButton.icon(
                            icon: const Icon(Icons.add, size: 16),
                            label: Text('audioClip.generateWaveform'.tr()),
                            onPressed: () => _safeSetState(() => _showGenerator = !_showGenerator),
                          ),
                        ],
                      ),
                    ),
                  if (_showGenerator)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      child: GeneratorPanel(
                        onGenerate: (samples, type) {
                          _pushSampleUndo();
                          _safeSetState(() {
                            _clip = AudioClip(samples: samples, sampleRate: 44100,
                                genParams: WaveformGenParams(type: type, frequency: 440));
                            _loadError = null;
                            _hasUnsavedChanges = true;
                            _showGenerator = false;
                          });
                        },
                      ),
                    ),
                ],
              ),
            ),
          ),
          if (_showDropSettings) _buildDropSettingsPanel(cs),
          if (_clip != null) _buildTransportBar(cs),
        ],
      ),
    );
  }

  Widget _buildTitleBar(ColorScheme cs) {
    return Container(
      height: 44,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(
          bottom: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          IconButton(
            icon: const Icon(Icons.close, size: 18),
            onPressed: () { _closeEditor(); },
            padding: EdgeInsets.zero,
            splashRadius: 16,
            style: IconButton.styleFrom(
              shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
            ),
          ),
          const SizedBox(width: 8),
          Text(
            _track?.name ??
                (widget.initialGenType != null
                    ? 'audioClip.newGenerated'.tr(namedArgs: {'type': widget.initialGenType!})
                    : 'audioClip.audioEditor'.tr()),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 14),
          ),
          const Spacer(),
          if (_clip == null && _loadError != null)
            TextButton.icon(
              icon: const Icon(Icons.add, size: 14),
              label: Text('audioClip.generateWaveform'.tr(), style: const TextStyle(fontSize: 11)),
              onPressed: () => _safeSetState(() => _showGenerator = !_showGenerator),
              style: TextButton.styleFrom(
                visualDensity: VisualDensity.compact,
                padding: const EdgeInsets.symmetric(horizontal: 8),
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildWaveformArea(ColorScheme cs) {
    if (_clip == null) return const SizedBox();
    return DragTarget<WaveformDropData>(
      onAcceptWithDetails: (details) => _onWaveformDropped(details.data),
      builder: (context, candidateData, rejectedData) {
        final isDragOver = candidateData.isNotEmpty;
        return Listener(
          onPointerSignal: (event) {
            if (event is PointerScrollEvent) {
              final ctrl = HardwareKeyboard.instance.logicalKeysPressed.any(
                (k) => k == LogicalKeyboardKey.controlLeft || k == LogicalKeyboardKey.controlRight,
              );
              if (ctrl) {
                _safeSetState(() {
                  _zoom = (_zoom * (event.scrollDelta.dy < 0 ? 1.15 : 0.85)).clamp(0.5, 10.0);
                });
              } else {
                final delta = -event.scrollDelta.dy;
                if (delta != 0 && _hScrollCtrl.hasClients) {
                  _hScrollCtrl.jumpTo(
                    (_hScrollCtrl.offset + delta).clamp(0.0, _hScrollCtrl.position.maxScrollExtent),
                  );
                }
              }
            }
          },
          child: Container(
            decoration: isDragOver
                ? BoxDecoration(
                    border: Border.all(color: cs.primary.withAlpha(150), width: 2),
                  )
                : null,
            child: GestureDetector(
              onTapUp: _onWaveformTap,
              onPanStart: _onWaveformPanStart,
              onPanUpdate: _onWaveformPanUpdate,
              onPanEnd: _onWaveformPanEnd,
              child: SingleChildScrollView(
                controller: _hScrollCtrl,
                scrollDirection: Axis.horizontal,
                child: SizedBox(
                  width: _clip!.duration * _pps,
                  child: CustomPaint(
                    size: Size(_clip!.duration * _pps, 300),
                    painter: _showSpectrogram
                        ? SpectrogramPainter(
                            samples: _clip!.samples,
                            sampleRate: _clip!.sampleRate,
                            pps: _pps,
                            scrollOffset: _hScrollCtrl.hasClients ? _hScrollCtrl.offset : 0,
                          )
                        : WaveformPainter(
                            samples: _clip!.samples,
                            sampleRate: _clip!.sampleRate,
                            pps: _pps,
                            playheadSec: _isPlaying ? _playheadSec : -1,
                            selection: _clip!.selection,
                            waveformColor: _track?.color ?? cs.primary,
                            scrollOffset: _hScrollCtrl.hasClients ? _hScrollCtrl.offset : 0,
                          ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  void _onWaveformDropped(WaveformDropData data) {
    if (_clip == null) return;
    _safeSetState(() {
      _pendingDropData = data;
      _dropFrequency = data.frequency;
      _dropDuration = data.duration;
      _dropAmplitude = data.amplitude;
      _dropMixLevel = 0.5;
      _showDropSettings = true;
    });
  }

  void _applyDropSettings() {
    if (_clip == null || _pendingDropData == null) return;
    _pushSampleUndo();
    final data = _pendingDropData!;
    final samples = _generateDropWaveform(data);
    final insertSample = (_playheadSec * _clip!.sampleRate).round().clamp(0, _clip!.samples.length);
    final endSample = (insertSample + samples.length).clamp(0, _clip!.samples.length);
    final mixLen = endSample - insertSample;
    if (mixLen <= 0) {
      _safeSetState(() => _showDropSettings = false);
      return;
    }

    final newSamples = Float64List.fromList(_clip!.samples);
    final mix = _dropMixLevel.clamp(0.0, 1.0);
    for (int i = 0; i < mixLen && i < samples.length; i++) {
      newSamples[insertSample + i] = (newSamples[insertSample + i] * (1 - mix) + samples[i] * mix).clamp(-1.0, 1.0);
    }
    _safeSetState(() {
      _clip = AudioClip(
        samples: newSamples,
        sampleRate: _clip!.sampleRate,
        sourceFile: _clip!.sourceFile,
        genParams: _clip!.genParams,
      );
      _hasUnsavedChanges = true;
      _showDropSettings = false;
      _pendingDropData = null;
    });
  }

  void _cancelDropSettings() {
    _safeSetState(() {
      _showDropSettings = false;
      _pendingDropData = null;
    });
  }

  Float64List _generateDropWaveform(WaveformDropData data) {
    switch (data.type) {
      case 'sine':
        return WaveformGenerator.sine(_dropFrequency, _dropDuration, amplitude: _dropAmplitude);
      case 'square':
        return WaveformGenerator.square(_dropFrequency, _dropDuration, amplitude: _dropAmplitude, dutyCycle: data.dutyCycle);
      case 'sawtooth':
        return WaveformGenerator.sawtooth(_dropFrequency, _dropDuration, amplitude: _dropAmplitude);
      case 'triangle':
        return WaveformGenerator.triangle(_dropFrequency, _dropDuration, amplitude: _dropAmplitude);
      case 'whiteNoise':
        return WaveformGenerator.whiteNoise(_dropDuration, amplitude: _dropAmplitude);
      case 'pinkNoise':
        return WaveformGenerator.pinkNoise(_dropDuration, amplitude: _dropAmplitude);
      case 'brownNoise':
        return WaveformGenerator.brownNoise(_dropDuration, amplitude: _dropAmplitude);
      default:
        return WaveformGenerator.sine(_dropFrequency, _dropDuration, amplitude: _dropAmplitude);
    }
  }

  Widget _buildWaveformCards(ColorScheme cs) {
    return WaveformCards(onWaveformDropped: (data) => _onWaveformDropped(data));
  }

  Widget _buildTransportBar(ColorScheme cs) {
    final selText = _clip?.selection != null && _clip!.selection!.isValid
        ? 'audioClip.selection'.tr(namedArgs: {
            'start': _clip!.selection!.startSec.toStringAsFixed(1),
            'end': _clip!.selection!.endSec.toStringAsFixed(1),
          })
        : '';
    return Container(
      height: 48,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(top: BorderSide(color: Theme.of(context).dividerColor)),
      ),
      child: Row(
        children: [
          _MiniBtn(
            icon: Icons.skip_previous_rounded,
            onTap: () => _safeSetState(() => _playheadSec = 0),
          ),
          const SizedBox(width: 4),
          _MiniBtn(
            icon: _isPlaying ? Icons.pause_rounded : Icons.play_arrow_rounded,
            isPrimary: true,
            onTap: _togglePlayback,
          ),
          const SizedBox(width: 4),
          _MiniBtn(
            icon: Icons.stop_rounded,
            onTap: _stopPlayback,
          ),
          const SizedBox(width: 4),
          _MiniBtn(
            icon: Icons.skip_next_rounded,
            onTap: () => _clip != null
                ? _safeSetState(() => _playheadSec = _clip!.duration)
                : null,
          ),
          const SizedBox(width: 12),
          Text(
            '${_formatTime(_playheadSec)} / ${_formatTime(_clip?.duration ?? 0)}',
            style: TextStyle(color: cs.primary, fontSize: 13, fontWeight: FontWeight.w500),
          ),
          const Spacer(),
          if (selText.isNotEmpty)
            Text(selText, style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant)),
          if (selText.isNotEmpty) const SizedBox(width: 8),
          if (_clip?.selection != null)
            TextButton.icon(
              icon: const Icon(Icons.close, size: 14),
              label: Text('audioClip.clear'.tr(), style: const TextStyle(fontSize: 10)),
              onPressed: _clearSelection,
              style: TextButton.styleFrom(
                visualDensity: VisualDensity.compact,
                padding: const EdgeInsets.symmetric(horizontal: 8),
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildDropSettingsPanel(ColorScheme cs) {
    final data = _pendingDropData;
    if (data == null) return const SizedBox();
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(
          top: BorderSide(color: Theme.of(context).dividerColor),
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text('audioClip.dropSettings'.tr(namedArgs: {'type': data.type}),
                  style: TextStyle(fontSize: 11, fontWeight: FontWeight.w600, color: cs.onSurface)),
              const Spacer(),
              Text('audioClip.mixPercent'.tr(namedArgs: {'p': '${(_dropMixLevel * 100).round()}'}),
                  style: TextStyle(fontSize: 10, color: cs.onSurfaceVariant)),
            ],
          ),
          const SizedBox(height: 6),
          if (!data.isNoiseType)
            _buildDropSlider('audioClip.frequency'.tr(), _dropFrequency, 20, 20000, true,
                '${_dropFrequency.round()} Hz', (v) => _dropFrequency = v),
          _buildDropSlider('audioClip.duration'.tr(), _dropDuration, 0.1, 30, false,
              '${_dropDuration.toStringAsFixed(1)}s', (v) => _dropDuration = v),
          _buildDropSlider('audioClip.amplitude'.tr(), _dropAmplitude, 0, 1, false,
              '${(_dropAmplitude * 100).round()}%', (v) => _dropAmplitude = v),
          _buildDropSlider('audioClip.mixLevel'.tr(), _dropMixLevel, 0, 1, false,
              '${(_dropMixLevel * 100).round()}%', (v) => _dropMixLevel = v),
          const SizedBox(height: 4),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              TextButton.icon(
                icon: const Icon(Icons.close, size: 14),
                label: Text('common.cancel'.tr(), style: const TextStyle(fontSize: 11)),
                onPressed: _cancelDropSettings,
                style: TextButton.styleFrom(visualDensity: VisualDensity.compact),
              ),
              const SizedBox(width: 8),
              FilledButton.icon(
                icon: const Icon(Icons.check, size: 14),
                label: Text('common.apply'.tr(), style: const TextStyle(fontSize: 11)),
                onPressed: _applyDropSettings,
                style: FilledButton.styleFrom(visualDensity: VisualDensity.compact),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildDropSlider(String label, double value, double min, double max, bool logarithmic,
      String display, ValueChanged<double> onChanged) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          SizedBox(width: 56, child: Text(label, style: TextStyle(fontSize: 10, color: Theme.of(context).colorScheme.onSurfaceVariant))),
          Expanded(
            child: SliderTheme(
              data: SliderTheme.of(context).copyWith(
                trackHeight: 3,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
              ),
              child: Slider(
                value: logarithmic
                    ? (log(value / min) / log(max / min)).clamp(0.0, 1.0)
                    : ((value - min) / (max - min)).clamp(0.0, 1.0),
                onChanged: (v) {
                  final real = logarithmic
                      ? min * pow(max / min, v)
                      : min + (max - min) * v;
                  _safeSetState(() => onChanged(real.clamp(min, max)));
                },
              ),
            ),
          ),
          SizedBox(width: 50, child: Text(display, style: TextStyle(fontSize: 9, color: Theme.of(context).colorScheme.onSurface))),
        ],
      ),
    );
  }

  Future<void> _handleProcess(String action) async {
    if (_clip == null) return;

    void applyAll(Float64List Function(Float64List) fn) {
      _pushSampleUndo();
      if (_clip!.selection != null && _clip!.selection!.isValid) {
        _applyToSelection(fn);
      } else {
        _clip!.samples.setAll(0, fn(Float64List.fromList(_clip!.samples)));
        _safeSetState(() {});
      }
      _hasUnsavedChanges = true;
    }

    // Simple no-dialog effects
    switch (action) {
      case 'normalize':
        applyAll((s) => AudioProcessingService.normalizePeak(s));
        return;
      case 'reverse':
        applyAll((s) => AudioProcessingService.reverse(s));
        return;
      case 'removeDc':
        applyAll((s) => AudioProcessingService.removeDCOffset(s));
        return;
      case 'invert':
        applyAll((s) => AudioProcessingService.invert(s));
        return;
    }

    // Effects with dialogs
    final dialogResult = await _openEffectDialog(action);
    if (dialogResult == null || dialogResult.samples == null) return;

    _pushSampleUndo();
    _clip!.samples.setAll(0, dialogResult.samples!);
    _hasUnsavedChanges = true;
    _safeSetState(() {});
  }

  Future<EffectResult?> _openEffectDialog(String action) async {
    if (_clip == null) return null;

    switch (action) {
      case 'compressor':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.compressor'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'threshold': -20, 'ratio': 4, 'knee': 6, 'attackMs': 5, 'releaseMs': 100, 'makeupGain': 0},
          process: (s, sr, p) => AudioProcessingService.dynamicsProcessor(s, sr,
            threshold: p['threshold']!, ratio: p['ratio']!, knee: p['knee']!,
            attackMs: p['attackMs']!, releaseMs: p['releaseMs']!, makeupGain: p['makeupGain']!,
          ),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.threshold'.tr(), paramKey: 'threshold', params: params, onChanged: onChanged, min: -60, max: 0, display: (v) => '${v.round()} dB', defaultValue: -20),
              effectSlider(label: 'audioClip.param.ratio'.tr(), paramKey: 'ratio', params: params, onChanged: onChanged, min: 1, max: 20, display: (v) => '${v.toStringAsFixed(1)}:1', defaultValue: 4),
              effectSlider(label: 'audioClip.param.knee'.tr(), paramKey: 'knee', params: params, onChanged: onChanged, min: 0, max: 20, display: (v) => '${v.round()} dB', defaultValue: 6),
              effectSlider(label: 'audioClip.param.attack'.tr(), paramKey: 'attackMs', params: params, onChanged: onChanged, min: 0.1, max: 100, display: (v) => '${v.round()} ms', defaultValue: 5),
              effectSlider(label: 'audioClip.param.release'.tr(), paramKey: 'releaseMs', params: params, onChanged: onChanged, min: 10, max: 1000, display: (v) => '${v.round()} ms', defaultValue: 100),
              effectSlider(label: 'audioClip.param.makeup'.tr(), paramKey: 'makeupGain', params: params, onChanged: onChanged, min: -12, max: 24, display: (v) => '${v.round()} dB', defaultValue: 0),
            ],
          ),
        );
      case 'echo':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.echo'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'delay1': 0.3, 'delay2': 0.5, 'delay3': 0.7, 'gain': 0.4, 'mix': 0.5},
          process: (s, sr, p) => AudioProcessingService.echo(s, sr,
            delays: [p['delay1']!, p['delay2']!, p['delay3']!],
            gains: [p['gain']!, p['gain']! * 0.6, p['gain']! * 0.35],
            mix: p['mix']!,
          ),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.delay1'.tr(), paramKey: 'delay1', params: params, onChanged: onChanged, min: 0.05, max: 2, display: (v) => '${v.toStringAsFixed(2)}s', defaultValue: 0.3),
              effectSlider(label: 'audioClip.param.delay2'.tr(), paramKey: 'delay2', params: params, onChanged: onChanged, min: 0.05, max: 2, display: (v) => '${v.toStringAsFixed(2)}s', defaultValue: 0.5),
              effectSlider(label: 'audioClip.param.delay3'.tr(), paramKey: 'delay3', params: params, onChanged: onChanged, min: 0.05, max: 2, display: (v) => '${v.toStringAsFixed(2)}s', defaultValue: 0.7),
              effectSlider(label: 'audioClip.param.gain'.tr(), paramKey: 'gain', params: params, onChanged: onChanged, min: 0, max: 1, display: (v) => '${(v * 100).round()}%', defaultValue: 0.4),
              effectSlider(label: 'audioClip.param.mix'.tr(), paramKey: 'mix', params: params, onChanged: onChanged, min: 0, max: 1, display: (v) => '${(v * 100).round()}%', defaultValue: 0.5),
            ],
          ),
        );
      case 'reverb':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.reverb'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'roomSize': 0.6, 'damping': 0.3, 'predelayMs': 30, 'mix': 0.3},
          process: (s, sr, p) => AudioProcessingService.reverbEnhanced(s, sr,
            roomSize: p['roomSize']!, damping: p['damping']!, predelayMs: p['predelayMs']!, mix: p['mix']!,
          ),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.roomSize'.tr(), paramKey: 'roomSize', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 0.6),
              effectSlider(label: 'audioClip.param.damping'.tr(), paramKey: 'damping', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 0.3),
              effectSlider(label: 'audioClip.param.predelay'.tr(), paramKey: 'predelayMs', params: params, onChanged: onChanged, min: 0, max: 200, display: (v) => '${v.round()} ms', defaultValue: 30),
              effectSlider(label: 'audioClip.param.mix'.tr(), paramKey: 'mix', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 0.3),
            ],
          ),
        );
      case 'delay':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.delay'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'delayTime': 0.3, 'feedback': 0.4, 'mix': 0.5},
          process: (s, sr, p) => AudioProcessingService.delay(s, sr,
            delayTime: p['delayTime']!, feedback: p['feedback']!, mix: p['mix']!,
          ),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.delay'.tr(), paramKey: 'delayTime', params: params, onChanged: onChanged, min: 0.05, max: 2, display: (v) => '${v.toStringAsFixed(2)}s', defaultValue: 0.3),
              effectSlider(label: 'audioClip.param.feedback'.tr(), paramKey: 'feedback', params: params, onChanged: onChanged, min: 0, max: 0.99, display: (v) => '${(v * 100).round()}%', defaultValue: 0.4),
              effectSlider(label: 'audioClip.param.mix'.tr(), paramKey: 'mix', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 0.5),
            ],
          ),
        );
      case 'equalizer':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.equalizer'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'freq1': 80, 'gain1': 0, 'q1': 1, 'freq2': 1000, 'gain2': 0, 'q2': 1, 'freq3': 5000, 'gain3': 0, 'q3': 1},
          process: (s, sr, p) => AudioProcessingService.equalizer(s, sr,
            bands: [
              (freq: p['freq1']!, gain: p['gain1']!, q: p['q1']!),
              (freq: p['freq2']!, gain: p['gain2']!, q: p['q2']!),
              (freq: p['freq3']!, gain: p['gain3']!, q: p['q3']!),
            ],
          ),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text('audioClip.param.band1'.tr(), style: TextStyle(fontSize: 10, color: Theme.of(context).colorScheme.onSurfaceVariant)),
              effectSlider(label: 'audioClip.param.freq'.tr(), paramKey: 'freq1', params: params, onChanged: onChanged, min: 20, max: 500, logarithmic: true, display: (v) => '${v.round()} Hz', defaultValue: 80),
              effectSlider(label: 'audioClip.param.gain'.tr(), paramKey: 'gain1', params: params, onChanged: onChanged, min: -24, max: 24, display: (v) => '${v.round()} dB', defaultValue: 0),
              effectSlider(label: 'Q', paramKey: 'q1', params: params, onChanged: onChanged, min: 0.1, max: 10, display: (v) => v.toStringAsFixed(1), defaultValue: 1),
              const SizedBox(height: 8),
              Text('audioClip.param.band2'.tr(), style: TextStyle(fontSize: 10, color: Theme.of(context).colorScheme.onSurfaceVariant)),
              effectSlider(label: 'audioClip.param.freq'.tr(), paramKey: 'freq2', params: params, onChanged: onChanged, min: 200, max: 8000, logarithmic: true, display: (v) => '${v.round()} Hz', defaultValue: 1000),
              effectSlider(label: 'audioClip.param.gain'.tr(), paramKey: 'gain2', params: params, onChanged: onChanged, min: -24, max: 24, display: (v) => '${v.round()} dB', defaultValue: 0),
              effectSlider(label: 'Q', paramKey: 'q2', params: params, onChanged: onChanged, min: 0.1, max: 10, display: (v) => v.toStringAsFixed(1), defaultValue: 1),
              const SizedBox(height: 8),
              Text('audioClip.param.band3'.tr(), style: TextStyle(fontSize: 10, color: Theme.of(context).colorScheme.onSurfaceVariant)),
              effectSlider(label: 'audioClip.param.freq'.tr(), paramKey: 'freq3', params: params, onChanged: onChanged, min: 1000, max: 20000, logarithmic: true, display: (v) => '${v.round()} Hz', defaultValue: 5000),
              effectSlider(label: 'audioClip.param.gain'.tr(), paramKey: 'gain3', params: params, onChanged: onChanged, min: -24, max: 24, display: (v) => '${v.round()} dB', defaultValue: 0),
              effectSlider(label: 'Q', paramKey: 'q3', params: params, onChanged: onChanged, min: 0.1, max: 10, display: (v) => v.toStringAsFixed(1), defaultValue: 1),
            ],
          ),
        );
      case 'pitchShift':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.pitchShift'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'semitones': 0},
          process: (s, sr, p) => AudioProcessingService.pitchShift(s, sr, semitones: p['semitones']!),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.semitones'.tr(), paramKey: 'semitones', params: params, onChanged: onChanged, min: -12, max: 12, display: (v) => '${v >= 0 ? "+" : ""}${v.toStringAsFixed(1)}', defaultValue: 0),
              Text('audioClip.param.semitoneTip'.tr(), style: TextStyle(fontSize: 9, color: Theme.of(context).colorScheme.onSurfaceVariant)),
            ],
          ),
        );
      case 'doppler':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.doppler'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'depth': 0.5, 'rate': 0.5},
          process: (s, sr, p) => AudioProcessingService.doppler(s, sr, depth: p['depth']!, rate: p['rate']!),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.depth'.tr(), paramKey: 'depth', params: params, onChanged: onChanged, min: 0, max: 6, display: (v) => '${v.toStringAsFixed(1)} st', defaultValue: 0.5),
              effectSlider(label: 'audioClip.param.rate'.tr(), paramKey: 'rate', params: params, onChanged: onChanged, min: 0.1, max: 10, display: (v) => '${v.toStringAsFixed(1)} Hz', defaultValue: 0.5),
            ],
          ),
        );
      case 'fadeIn':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.fadeIn'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'duration': 0.5},
          process: (s, sr, p) => AudioProcessingService.fadeIn(s, p['duration']!, sr),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.duration'.tr(), paramKey: 'duration', params: params, onChanged: onChanged, min: 0.05, max: 10, display: (v) => '${v.toStringAsFixed(1)}s', defaultValue: 0.5),
            ],
          ),
        );
      case 'fadeOut':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.fadeOut'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'duration': 0.5},
          process: (s, sr, p) => AudioProcessingService.fadeOut(s, p['duration']!, sr),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.duration'.tr(), paramKey: 'duration', params: params, onChanged: onChanged, min: 0.05, max: 10, display: (v) => '${v.toStringAsFixed(1)}s', defaultValue: 0.5),
            ],
          ),
        );
      case 'distort':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.distort'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'threshold': 0.3},
          process: (s, sr, p) => AudioProcessingService.distort(s, p['threshold']!),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.threshold'.tr(), paramKey: 'threshold', params: params, onChanged: onChanged, min: 0.01, max: 1, display: (v) => '${(v * 100).round()}%', defaultValue: 0.3),
            ],
          ),
        );
      case 'amplitudeMap':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.amplitudeMap'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'drive': 1.0},
          process: (s, sr, p) {
            final drive = p['drive']!;
            // Build a curve: soft-clipping transfer
            final curve = List.generate(256, (i) {
              final x = i / 255.0;
              final mapped = x * drive;
              return mapped > 1 ? 1.0 - (mapped - 1) * 0.3 : mapped;
            });
            // Ensure monotonic
            for (int i = 1; i < curve.length; i++) {
              curve[i] = curve[i].clamp(0.0, 1.0);
            }
            return AudioProcessingService.amplitudeMap(s, curve);
          },
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.drive'.tr(), paramKey: 'drive', params: params, onChanged: onChanged, min: 0.1, max: 5, display: (v) => v.toStringAsFixed(1), defaultValue: 1.0),
            ],
          ),
        );
      case 'mechanize':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.mechanize'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'sampleRateReduce': 0.1, 'bitDepth': 8},
          process: (s, sr, p) => AudioProcessingService.mechanize(s, sr,
            sampleRateReduce: p['sampleRateReduce']!, bitDepth: p['bitDepth']!,
          ),
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.rateReduce'.tr(), paramKey: 'sampleRateReduce', params: params, onChanged: onChanged, min: 0.01, max: 0.5, display: (v) => '${(v * 100).round()}%', defaultValue: 0.1),
              effectSlider(label: 'audioClip.param.bitDepth'.tr(), paramKey: 'bitDepth', params: params, onChanged: onChanged, min: 2, max: 16, divisions: 14, display: (v) => '${v.round()} bit', defaultValue: 8),
            ],
          ),
        );
      case 'spectrumFilter':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.spectrumFilter'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'lowCut': 0, 'highCut': 1, 'amount': 1},
          process: (s, sr, p) {
            final fftSize = 2048;
            final half = fftSize ~/ 2;
            final lowBin = (p['lowCut']! * half).round().clamp(0, half);
            final highBin = (p['highCut']! * half).round().clamp(lowBin, half);
            final amount = p['amount']!;
            final envelope = List.generate(half + 1, (i) {
              if (i < lowBin || i > highBin) return 1.0 - amount * 0.8;
              return 1.0;
            });
            return AudioProcessingService.spectrumFilter(s, sr, envelope: envelope, fftSize: fftSize);
          },
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.lowCut'.tr(), paramKey: 'lowCut', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 0),
              effectSlider(label: 'audioClip.param.highCut'.tr(), paramKey: 'highCut', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 1),
              effectSlider(label: 'audioClip.param.amount'.tr(), paramKey: 'amount', params: params, onChanged: onChanged, display: (v) => '${(v * 100).round()}%', defaultValue: 1),
            ],
          ),
        );
      case 'splitByFreq':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.splitByFreq'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'lowFreq': 200, 'midFreq': 2000},
          process: (s, sr, p) {
            // Split into Low/Mid/High and sum back as multi-channel mix
            final bands = [
              FreqBand(lowFreq: 0, highFreq: p['lowFreq']!),
              FreqBand(lowFreq: p['lowFreq']!, highFreq: p['midFreq']!),
              FreqBand(lowFreq: p['midFreq']!, highFreq: sr / 2),
            ];
            // For preview, mix them back together
            final split = FftService.splitBands(s, sr, bands);
            if (split.length != 3) return Float64List.fromList(s);
            final result = Float64List(s.length);
            for (int i = 0; i < s.length; i++) {
              result[i] = (split[0][i] + split[1][i] + split[2][i]) / 3;
            }
            return result;
          },
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.lowMidCross'.tr(), paramKey: 'lowFreq', params: params, onChanged: onChanged, min: 20, max: 2000, logarithmic: true, display: (v) => '${v.round()} Hz', defaultValue: 200),
              effectSlider(label: 'audioClip.param.midHighCross'.tr(), paramKey: 'midFreq', params: params, onChanged: onChanged, min: 200, max: 20000, logarithmic: true, display: (v) => '${v.round()} Hz', defaultValue: 2000),
            ],
          ),
        );
      case 'splitByTime':
        return showEffectDialog(
          context: context,
          title: 'audioClip.dialog.splitByTime'.tr(),
          clipSamples: _clip!.samples,
          sampleRate: _clip!.sampleRate,
          initialParams: {'split1': 1.0, 'split2': 3.0},
          process: (s, sr, p) {
            // For preview, merge slices back
            final splits = AudioProcessingService.splitByTime(s, sr, [p['split1']!, p['split2']!]);
            if (splits.isEmpty) return Float64List.fromList(s);
            final result = Float64List(s.length);
            int pos = 0;
            for (final seg in splits) {
              for (int i = 0; i < seg.length && pos + i < s.length; i++) {
                result[pos + i] = seg[i];
              }
              pos += seg.length;
            }
            return result;
          },
          builder: (ctx, params, onChanged) => Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              effectSlider(label: 'audioClip.param.split1'.tr(), paramKey: 'split1', params: params, onChanged: onChanged, min: 0.1, max: 30, display: (v) => '${v.toStringAsFixed(1)}s', defaultValue: 1.0),
              effectSlider(label: 'audioClip.param.split2'.tr(), paramKey: 'split2', params: params, onChanged: onChanged, min: 0.1, max: 30, display: (v) => '${v.toStringAsFixed(1)}s', defaultValue: 3.0),
            ],
          ),
        );
      case 'mixer':
        // Multi-channel mixer is a special case that needs multiple tracks
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('audioClip.mixerHint'.tr())),
          );
        }
        return null;
      default:
        return null;
    }
  }

  Future<void> _handleSplit(BuildContext context) async {
    if (_clip == null) return;
    final bands = await showFrequencySplitDialog(context, _clip!.duration);
    if (bands == null || bands.isEmpty) return;

    final results = FftService.splitBands(_clip!.samples, _clip!.sampleRate, bands);
    final projectNotifier = ref.read(projectProvider.notifier);

    for (int i = 0; i < results.length && i < bands.length; i++) {
      final band = bands[i];
      final label = '${_track?.name ?? "Split"}_${band.lowFreq.round()}-${band.highFreq.round()}Hz';
      final path = await _saveToWav(results[i], prefix: 'split_${i}_${DateTime.now().millisecondsSinceEpoch}');
      projectNotifier.addAudioTrack(name: label, audioFilePath: path);
    }

    if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('audioClip.splitIntoBands'.tr(namedArgs: {'n': '${results.length}'}))),
      );
    }
  }

  String _formatTime(double seconds) {
    final m = (seconds ~/ 60).toString().padLeft(2, '0');
    final s = (seconds % 60).toStringAsFixed(1).padLeft(4, '0');
    return '$m:$s';
  }
}

class _MiniBtn extends StatelessWidget {
  final IconData icon;
  final VoidCallback? onTap;
  final bool isPrimary;

  const _MiniBtn({required this.icon, this.onTap, this.isPrimary = false});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Material(
      color: isPrimary ? cs.primary : Colors.transparent,
      borderRadius: BorderRadius.circular(isPrimary ? 16 : 4),
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(isPrimary ? 16 : 4),
        child: Container(
          width: 32, height: 32,
          alignment: Alignment.center,
          child: Icon(icon, size: 20,
              color: isPrimary ? Theme.of(context).scaffoldBackgroundColor : cs.onSurfaceVariant),
        ),
      ),
    );
  }
}

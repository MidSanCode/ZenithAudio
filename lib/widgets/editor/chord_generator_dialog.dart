import 'package:flutter/material.dart';

import '../../models/note.dart';
import '../../models/project.dart';
import '../../services/chord_service.dart';

/// Auto-chord generator dialog (FL-style chord tool).
///
/// Two modes:
///  1. Progression: pick key/mode/progression → insert chord track starting
///     at [insertSec] (playhead or 0).
///  2. Chordify: harmonize each melody note (thirds/sixths within scale).
class ChordGeneratorDialog extends StatefulWidget {
  final Project project;
  final List<Note> existingNotes;
  final double insertSec;
  final double secPerBeat;
  final int gridResolution; // beats per grid step (for snapping)
  final bool snapToGrid;
  final ValueChanged<List<Note>> onInsert;

  const ChordGeneratorDialog({
    super.key,
    required this.project,
    required this.existingNotes,
    required this.insertSec,
    required this.secPerBeat,
    required this.gridResolution,
    required this.snapToGrid,
    required this.onInsert,
  });

  @override
  State<ChordGeneratorDialog> createState() => _ChordGeneratorDialogState();
}

class _ChordGeneratorDialogState extends State<ChordGeneratorDialog> {
  late int _keyRoot = _defaultKeyRoot();
  String _mode = _defaultMode();
  String _progression = 'pop';
  String _pattern = 'block';
  int _octave = 3;
  double _beatsPerChord = 4;
  int _repeat = 2;
  bool _replaceExisting = false;

  int _defaultKeyRoot() {
    final ks = widget.project.keySignature; // e.g. 'C', 'Am', 'F#'
    final isMinor = ks.contains('m');
    final root = ks.replaceAll('m', '').trim();
    final idx = ChordService.noteNames.indexOf(root.isEmpty ? 'C' : root);
    return idx < 0 ? 0 : idx;
  }

  String _defaultMode() =>
      widget.project.keySignature.contains('m') ? 'minor' : 'major';

  String get _modeLabel => _mode == 'major' ? '大调' : '小调';

  double _snap(double sec) {
    if (!widget.snapToGrid) return sec;
    final step = widget.gridResolution * widget.secPerBeat;
    return (sec / step).round() * step;
  }

  List<Note> _build() {
    final start = _snap(widget.insertSec);
    return ChordService.generateProgression(
      keyRootPc: _keyRoot,
      mode: _mode,
      progressionId: _progression,
      startSec: start,
      secPerBeat: widget.secPerBeat,
      beatsPerChord: _beatsPerChord,
      repeat: _repeat,
      patternId: _pattern,
      octave: _octave,
    );
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final notes = _build();
    final endSec = notes.isEmpty
        ? 0.0
        : notes.map((n) => n.startTime + n.duration).reduce((a, b) => a > b ? a : b);

    return AlertDialog(
      title: Row(
        children: [
          const Icon(Icons.library_music, size: 20),
          const SizedBox(width: 8),
          const Expanded(child: Text('自动和弦生成', style: TextStyle(fontSize: 16))),
        ],
      ),
      content: SizedBox(
        width: 420,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              // Key + mode
              Row(
                children: [
                  const Text('调性', style: TextStyle(fontSize: 12)),
                  const SizedBox(width: 8),
                  Expanded(
                    child: DropdownButtonFormField<int>(
                      initialValue: _keyRoot,
                      decoration: const InputDecoration(
                          border: OutlineInputBorder(), isDense: true),
                      items: ChordService.noteNames
                          .asMap()
                          .entries
                          .map((e) => DropdownMenuItem(
                              value: e.key, child: Text(e.value)))
                          .toList(),
                      onChanged: (v) => setState(() => _keyRoot = v ?? 0),
                    ),
                  ),
                  const SizedBox(width: 8),
                  SegmentedButton<String>(
                    segments: const [
                      ButtonSegment(value: 'major', label: Text('大调')),
                      ButtonSegment(value: 'minor', label: Text('小调')),
                    ],
                    selected: {_mode},
                    onSelectionChanged: (s) => setState(() => _mode = s.first),
                  ),
                ],
              ),
              const SizedBox(height: 10),

              // Progression
              DropdownButtonFormField<String>(
                initialValue: _progression,
                decoration: const InputDecoration(
                  labelText: '和弦进行',
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
                items: ChordService.progressionIds
                    .map((id) => DropdownMenuItem(
                        value: id,
                        child: Text(_progressionLabel(id),
                            style: const TextStyle(fontSize: 12))))
                    .toList(),
                onChanged: (v) => setState(() => _progression = v ?? 'pop'),
              ),
              const SizedBox(height: 10),

              // Pattern
              DropdownButtonFormField<String>(
                initialValue: _pattern,
                decoration: const InputDecoration(
                  labelText: '演奏型态',
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
                items: const [
                  DropdownMenuItem(value: 'block', child: Text('柱式(整拍齐奏)')),
                  DropdownMenuItem(value: 'arpUp', child: Text('上行琶音')),
                  DropdownMenuItem(value: 'arpDown', child: Text('下行琶音')),
                  DropdownMenuItem(value: 'alberti', child: Text('阿尔贝蒂低音')),
                  DropdownMenuItem(value: 'strum', child: Text('扫弦(微错开)')),
                  DropdownMenuItem(value: 'bassChord', child: Text('贝斯+和弦')),
                ],
                onChanged: (v) => setState(() => _pattern = v ?? 'block'),
              ),
              const SizedBox(height: 10),

              // Octave / beats / repeat
              Row(
                children: [
                  const Text('八度', style: TextStyle(fontSize: 12)),
                  Expanded(
                    child: Slider(
                      value: _octave.toDouble(),
                      min: 2,
                      max: 5,
                      divisions: 3,
                      label: '$_octave',
                      onChanged: (v) => setState(() => _octave = v.round()),
                    ),
                  ),
                  const Text('每和弦拍数', style: TextStyle(fontSize: 12)),
                  Expanded(
                    child: Slider(
                      value: _beatsPerChord,
                      min: 1,
                      max: 8,
                      divisions: 7,
                      label: '${_beatsPerChord.round()}',
                      onChanged: (v) =>
                          setState(() => _beatsPerChord = v.roundToDouble()),
                    ),
                  ),
                ],
              ),
              Row(
                children: [
                  const Text('重复次数', style: TextStyle(fontSize: 12)),
                  Expanded(
                    child: Slider(
                      value: _repeat.toDouble(),
                      min: 1,
                      max: 8,
                      divisions: 7,
                      label: '$_repeat',
                      onChanged: (v) => setState(() => _repeat = v.round()),
                    ),
                  ),
                  Checkbox(
                    value: _replaceExisting,
                    onChanged: (v) =>
                        setState(() => _replaceExisting = v ?? false),
                  ),
                  const Text('替换原有音符', style: TextStyle(fontSize: 12)),
                ],
              ),

              const Divider(),

              // Roman numeral preview
              Wrap(
                spacing: 6,
                runSpacing: 4,
                children: [
                  for (final (deg, type) in ChordService
                          .progressions[_progression] ??
                      const <(int, String)>[])
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 8, vertical: 3),
                      decoration: BoxDecoration(
                        color: cs.primaryContainer,
                        borderRadius: BorderRadius.circular(10),
                      ),
                      child: Text(
                        '${ChordService.noteNames[(_keyRoot + ChordService.degreeToSemitone(deg, mode: _mode)) % 12]} '
                        '${ChordService.romanName(deg, type, mode: _mode)}',
                        style: const TextStyle(fontSize: 11),
                      ),
                    ),
                ],
              ),
              const SizedBox(height: 8),
              Text(
                '将插入 ${notes.length} 个音符,总长 ${endSec.toStringAsFixed(1)}s'
                '(起始 ${_snap(widget.insertSec).toStringAsFixed(2)}s)',
                style: TextStyle(
                    fontSize: 11,
                    color: cs.onSurfaceVariant,
                    fontStyle: FontStyle.italic),
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton.icon(
          onPressed: notes.isEmpty
              ? null
              : () {
                  widget.onInsert(_replaceExisting ? notes : notes);
                  Navigator.pop(context, _replaceExisting);
                },
          icon: const Icon(Icons.add, size: 16),
          label: const Text('插入'),
        ),
      ],
    );
  }

  String _progressionLabel(String id) {
    switch (id) {
      case 'pop':
        return '流行 1-5-6-4 ($_modeLabel)';
      case 'sensitive':
        return '卡农进行 6-4-1-5';
      case '50s':
        return '50年代 1-6-4-5';
      case 'canon':
        return '八和弦 1-5-6-3-4-1-4-5';
      case 'jazz':
        return '爵士 2-5-1';
      case 'sad_lofi':
        return 'Lo-fi 1m-4m-6m-5';
      case 'minor_pop':
        return '小调流行 1m-6-3-7';
      case 'blues':
        return '布鲁斯 12小节';
      default:
        return id;
    }
  }
}

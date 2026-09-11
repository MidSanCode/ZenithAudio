import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';

import '../../models/note.dart';
import '../../models/project.dart';
import '../../services/chord_service.dart';

/// Auto-chord generator dialog.
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
  final double gridResolution; // beats per grid step (for snapping)
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
  int _keyRoot = 0;
  String _mode = 'major';
  String _progression = 'pop';
  String _pattern = 'block';
  int _octave = 3;
  double _beatsPerChord = 4;
  int _repeat = 2;
  bool _replaceExisting = false;
  bool _melodyMode = false;

  @override
  void initState() {
    super.initState();
    final ks = widget.project.keySignature; // e.g. 'C', 'Am', 'F#'
    _mode = ks.contains('m') ? 'minor' : 'major';
    final root = ks.replaceAll('m', '').trim();
    final idx = ChordService.noteNames.indexOf(root.isEmpty ? 'C' : root);
    _keyRoot = idx < 0 ? 0 : idx;
  }

  String get _modeLabel => _mode == 'major' ? 'chord.major'.tr() : 'chord.minor'.tr();

  double _snap(double sec) {
    if (!widget.snapToGrid) return sec;
    final step = widget.gridResolution * widget.secPerBeat;
    return (sec / step).round() * step;
  }

  List<({int degree, String type, double start, double duration, int octave})>
      _melodyPlan(double start) {
    return ChordService.harmonizePlan(
      keyRootPc: _keyRoot,
      mode: _mode,
      progressionId: _progression,
      melodyNotes: widget.existingNotes,
      startSec: start,
      secPerBeat: widget.secPerBeat,
      beatsPerChord: _beatsPerChord,
      octave: _octave,
    );
  }

  List<Note> _build() {
    final start = _snap(widget.insertSec);
    if (_melodyMode) {
      return ChordService.notesFromPlan(
        _melodyPlan(start),
        keyRootPc: _keyRoot,
        mode: _mode,
        patternId: _pattern,
      );
    }
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
          Expanded(child: Text('chord.title'.tr(), style: const TextStyle(fontSize: 16))),
        ],
      ),
      content: SizedBox(
        width: 420,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              // Source mode: template progression vs melody harmonization
              SegmentedButton<bool>(
                segments: [
                  ButtonSegment(
                      value: false,
                      label: Text('chord.source.progression'.tr(),
                          style: const TextStyle(fontSize: 12))),
                  ButtonSegment(
                      value: true,
                      label: Text('chord.source.melody'.tr(),
                          style: const TextStyle(fontSize: 12))),
                ],
                selected: {_melodyMode},
                onSelectionChanged: (s) =>
                    setState(() => _melodyMode = s.first),
              ),
              const SizedBox(height: 10),

              // Key + mode
              Row(
                children: [
                  Text('chord.key'.tr(), style: const TextStyle(fontSize: 12)),
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
                    segments: [
                      ButtonSegment(value: 'major', label: Text('chord.major'.tr())),
                      ButtonSegment(value: 'minor', label: Text('chord.minor'.tr())),
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
                decoration: InputDecoration(
                  labelText: 'chord.progression'.tr(),
                  border: const OutlineInputBorder(),
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
                decoration: InputDecoration(
                  labelText: 'chord.pattern'.tr(),
                  border: const OutlineInputBorder(),
                  isDense: true,
                ),
                items: [
                  DropdownMenuItem(value: 'block', child: Text('chord.pattern.block'.tr())),
                  DropdownMenuItem(value: 'arpUp', child: Text('chord.pattern.arpUp'.tr())),
                  DropdownMenuItem(value: 'arpDown', child: Text('chord.pattern.arpDown'.tr())),
                  DropdownMenuItem(value: 'alberti', child: Text('chord.pattern.alberti'.tr())),
                  DropdownMenuItem(value: 'strum', child: Text('chord.pattern.strum'.tr())),
                  DropdownMenuItem(value: 'bassChord', child: Text('chord.pattern.bassChord'.tr())),
                ],
                onChanged: (v) => setState(() => _pattern = v ?? 'block'),
              ),
              const SizedBox(height: 10),

              // Octave / beats / repeat
              Row(
                children: [
                  Text('chord.octave'.tr(), style: const TextStyle(fontSize: 12)),
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
                  Text('chord.beatsPerChord'.tr(), style: const TextStyle(fontSize: 12)),
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
                  if (!_melodyMode) ...[
                    Text('chord.repeat'.tr(), style: const TextStyle(fontSize: 12)),
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
                    Text('chord.replaceExisting'.tr(), style: const TextStyle(fontSize: 12)),
                  ] else
                    Expanded(
                      child: Text(
                        widget.existingNotes.isEmpty
                            ? 'chord.melodyEmpty'.tr()
                            : 'chord.melodyHint'.tr(namedArgs: {
                                'n': '${widget.existingNotes.length}'
                              }),
                        style: TextStyle(
                            fontSize: 11, color: cs.onSurfaceVariant),
                      ),
                    ),
                ],
              ),

              const Divider(),

              // Roman numeral preview (template) / chosen chords (melody)
              Wrap(
                spacing: 6,
                runSpacing: 4,
                children: _melodyMode
                    ? [
                        for (final step in _melodyPlan(_snap(widget.insertSec)))
                          Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 8, vertical: 3),
                            decoration: BoxDecoration(
                              color: cs.primaryContainer,
                              borderRadius: BorderRadius.circular(10),
                            ),
                            child: Text(
                              '${ChordService.noteNames[(_keyRoot + ChordService.degreeToSemitone(step.degree, mode: _mode)) % 12]} '
                              '${ChordService.romanName(step.degree, step.type, mode: _mode)}',
                              style: const TextStyle(fontSize: 11),
                            ),
                          ),
                      ]
                    : [
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
                'chord.insertSummary'.tr(namedArgs: {
                  'n': '${notes.length}',
                  'duration': endSec.toStringAsFixed(1),
                  'start': _snap(widget.insertSec).toStringAsFixed(2),
                }),
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
          child: Text('common.cancel'.tr()),
        ),
        FilledButton.icon(
          onPressed: notes.isEmpty
              ? null
              : () {
                  widget.onInsert(notes);
                  // Melody mode always merges (never replaces the melody).
                  Navigator.pop(context, _melodyMode ? false : _replaceExisting);
                },
          icon: const Icon(Icons.add, size: 16),
          label: Text('chord.insert'.tr()),
        ),
      ],
    );
  }

  String _progressionLabel(String id) {
    switch (id) {
      case 'pop':
        return 'chord.progression.pop'.tr(namedArgs: {'mode': _modeLabel});
      case 'sensitive':
        return 'chord.progression.sensitive'.tr();
      case '50s':
        return 'chord.progression.50s'.tr();
      case 'canon':
        return 'chord.progression.canon'.tr();
      case 'jazz':
        return 'chord.progression.jazz'.tr();
      case 'sad_lofi':
        return 'chord.progression.sadLofi'.tr();
      case 'minor_pop':
        return 'chord.progression.minorPop'.tr();
      case 'blues':
        return 'chord.progression.blues'.tr();
      default:
        return id;
    }
  }
}

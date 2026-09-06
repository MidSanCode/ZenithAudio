import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:media_kit/media_kit.dart' hide Track;

import '../../models/envelope.dart';
import '../../models/instrument.dart';
import '../../models/track.dart' show Track;
import '../../providers/floating_window_provider.dart';
import '../../providers/project_provider.dart';
import '../../providers/settings_provider.dart';
import '../../services/audio_url.dart';
import '../../services/synth_service.dart';
import 'envelope_editor.dart';

/// Persists a synth edit: built-in `syn_` presets become a new `custom_` copy;
/// customs overwrite in place. Returns the saved preset id (may differ from
/// [edited].id for built-ins).
Future<String> saveSynthEdit(InstrumentPreset edited) async {
  final InstrumentPreset toSave;
  if (edited.id.startsWith('syn_')) {
    toSave =
        edited.copyWith(id: 'custom_${DateTime.now().millisecondsSinceEpoch}');
  } else {
    toSave = edited;
  }
  await InstrumentPreset.saveUserCustom(toSave);
  return toSave.id;
}

/// Shared launcher for synth-editor entry points.
class SynthEditorLauncher {
  SynthEditorLauncher._();

  /// True when [preset] can be edited in the synth editor.
  static bool isEditable(InstrumentPreset preset) =>
      preset.category == InstrumentCategory.synth ||
      preset.id.startsWith('syn_') ||
      preset.id.startsWith('custom_');

  /// Open the synth editor for [track] — floating window when the user's
  /// editor mode is `float` (and [preferFloat]), otherwise the modal dialog.
  static void openForTrack(
    BuildContext context,
    WidgetRef ref,
    Track track, {
    bool preferFloat = true,
  }) {
    final name = track.instrumentName;
    final preset = name == null ? null : InstrumentPreset.fromIdOrNull(name);
    if (preset == null || !isEditable(preset)) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('当前音轨的乐器不支持合成器编辑(请先更换为合成器音色)'),
          duration: Duration(seconds: 3),
        ),
      );
      return;
    }

    void applySaved(InstrumentPreset edited) async {
      final savedId = await saveSynthEdit(edited);
      ref.read(projectProvider.notifier).setTrackInstrument(track.id, savedId);
    }

    final settings = ref.read(settingsProvider);
    if (preferFloat && settings.editorMode == 'float') {
      final notifier = ref.read(floatingWindowProvider.notifier);
      late final String windowId;
      windowId = notifier.open(
        title: '合成器: ${preset.name}',
        size: const Size(520, 560),
        builder: (_) => SynthEditorPanel(
          preset: preset,
          onSaved: applySaved,
          onClose: () => notifier.close(windowId),
        ),
      );
    } else {
      SynthEditorDialog.show(context, preset, applySaved);
    }
  }
}

/// Editor body for a synth-engine instrument: engine selector, engine params,
/// multi-point amplitude envelope, and live preview.
///
/// Embeddable: used both by [SynthEditorDialog] (modal) and directly inside a
/// floating window ([FloatingWindow] passes [onClose] to wire the close).
class SynthEditorPanel extends StatefulWidget {
  final InstrumentPreset preset;
  final ValueChanged<InstrumentPreset> onSaved;

  /// When non-null the panel runs in floating-window mode: it renders its own
  /// header/action bar and invokes this after save/cancel to close the window.
  final VoidCallback? onClose;

  const SynthEditorPanel({
    super.key,
    required this.preset,
    required this.onSaved,
    this.onClose,
  });

  bool get _isFloating => onClose != null;

  @override
  State<SynthEditorPanel> createState() => _SynthEditorPanelState();
}

class _SynthEditorPanelState extends State<SynthEditorPanel> {
  late InstrumentPreset _draft;
  Player? _player;
  bool _isPreviewing = false;

  static const engines = {
    'additive': '加法 (Additive)',
    'subtractive': '减法 (Subtractive)',
    'wavetable': '波表 (Wavetable)',
    'fm': 'FM (2-op)',
    'sample': '采样 (SoundFont)',
    'granular': '粒子 (Granular)',
  };

  @override
  void initState() {
    super.initState();
    _draft = widget.preset;
  }

  @override
  void dispose() {
    _player?.dispose();
    super.dispose();
  }

  void _update(InstrumentPreset Function(InstrumentPreset) fn) {
    setState(() => _draft = fn(_draft));
  }

  Future<void> _preview() async {
    final synth = SynthService();
    final wav = synth.renderPreviewWav(
      _draft,
      pitch: 60,
      duration: 1.2,
      velocity: 100,
    );

    await _player?.dispose();
    final player = Player();
    _player = player;
    if (!mounted) return;
    setState(() => _isPreviewing = true);
    player.stream.completed.listen((completed) {
      if (completed && mounted) setState(() => _isPreviewing = false);
    });

    final uri = await createAudioUrl(wav);
    await player.open(Media(uri));
    player.play();
  }

  void _commit() {
    widget.onSaved(_draft);
    if (widget._isFloating) {
      widget.onClose!();
    } else {
      Navigator.pop(context);
    }
  }

  void _cancel() {
    if (widget._isFloating) {
      widget.onClose!();
    } else {
      Navigator.pop(context);
    }
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final p = _draft;
    final engine = p.synthEngine ?? 'additive';

    final body = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (widget._isFloating) ...[
          Row(
            children: [
              const Icon(Icons.tune, size: 16),
              const SizedBox(width: 6),
              Expanded(
                child: Text('合成器: ${p.name}',
                    style: const TextStyle(fontSize: 13)),
              ),
            ],
          ),
          const SizedBox(height: 10),
        ],
        DropdownButtonFormField<String>(
          initialValue: engine,
          decoration: const InputDecoration(
            labelText: '合成引擎',
            border: OutlineInputBorder(),
            isDense: true,
          ),
          items: engines.entries
              .map((e) =>
                  DropdownMenuItem(value: e.key, child: Text(e.value)))
              .toList(),
          onChanged: (v) => _update((d) => d.copyWith(synthEngine: v)),
        ),
        const SizedBox(height: 12),

        if (engine == 'subtractive') ...[
          _slider('滤波器截止', p.filterCutoff, 60, 8000,
              (v) => _update((d) => d.copyWith(filterCutoff: v)),
              format: (v) => '${v.round()} Hz'),
          _slider('共振', p.filterResonance, 0.3, 10,
              (v) => _update((d) => d.copyWith(filterResonance: v))),
          _slider('滤波包络量', p.filterEnvAmount, 0, 10,
              (v) => _update((d) => d.copyWith(filterEnvAmount: v))),
          _slider('滤波衰减', p.filterDecay, 0.02, 2,
              (v) => _update((d) => d.copyWith(filterDecay: v))),
          _slider('滤波延音', p.filterSustain, 0, 1,
              (v) => _update((d) => d.copyWith(filterSustain: v))),
        ],
        if (engine == 'wavetable')
          _slider('Morph 周期', p.morphRate, 0, 12,
              (v) => _update((d) => d.copyWith(morphRate: v)),
              format: (v) => v < 0.05 ? '静止' : '${v.toStringAsFixed(1)}s'),
        if (engine == 'fm') ...[
          _slider('FM 比率', p.fmRatio, 0.25, 8,
              (v) => _update((d) => d.copyWith(fmRatio: v)), fraction: 2),
          _slider('FM 指数', p.fmIndex, 0, 10,
              (v) => _update((d) => d.copyWith(fmIndex: v))),
          _slider('指数衰减', p.fmDecay, 0.05, 3,
              (v) => _update((d) => d.copyWith(fmDecay: v))),
        ],
        if (engine == 'granular')
          Text('粒子合成:短音粒 + 随机微失谐 + 窗口化叠加,适合氛围/噪性质感。',
              style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant)),
        if (engine == 'sample')
          Text('采样引擎:program ${p.programNumber}。需先在乐器页加载 .sf2 音色库。',
              style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant)),

        const SizedBox(height: 8),
        _slider('失谐 (cents)', p.detuneCents, 0, 50,
            (v) => _update((d) => d.copyWith(detuneCents: v)),
            fraction: 1),

        const Divider(height: 20),

        Text('ADSR',
            style: TextStyle(fontSize: 11, color: cs.onSurfaceVariant)),
        Row(children: [
          _adsrField('A', p.attack,
              (v) => _update((d) => d.copyWith(attack: v))),
          _adsrField(
              'D', p.decay, (v) => _update((d) => d.copyWith(decay: v))),
          _adsrField('S', p.sustain,
              (v) => _update((d) => d.copyWith(sustain: v))),
          _adsrField('R', p.release,
              (v) => _update((d) => d.copyWith(release: v))),
        ]),

        const SizedBox(height: 8),

        Row(
          children: [
            Text('包络曲线',
                style:
                    TextStyle(fontSize: 11, color: cs.onSurfaceVariant)),
            const Spacer(),
            TextButton(
              onPressed: () => _update(
                  (d) => d.copyWith(envCurve: EnvelopeCurve.pluck())),
              child: const Text('拨弦', style: TextStyle(fontSize: 11)),
            ),
            TextButton(
              onPressed: () =>
                  _update((d) => d.copyWith(envCurve: EnvelopeCurve.pad())),
              child: const Text('铺底', style: TextStyle(fontSize: 11)),
            ),
            TextButton(
              onPressed: () => _update((d) => d.copyWith(envCurve: null)),
              child: const Text('用 ADSR', style: TextStyle(fontSize: 11)),
            ),
          ],
        ),
        Container(
          decoration: BoxDecoration(
            color: Colors.black26,
            borderRadius: BorderRadius.circular(6),
            border: Border.all(color: cs.outlineVariant),
          ),
          child: p.envCurve != null
              ? EnvelopeEditor(
                  curve: p.envCurve!,
                  onChanged: (c) =>
                      _update((d) => d.copyWith(envCurve: c)),
                )
              : SizedBox(
                  height: 100,
                  child: Center(
                    child: Text('未启用曲线包络(使用 ADSR)',
                        style:
                            TextStyle(fontSize: 11, color: cs.outline)),
                  ),
                ),
        ),
      ],
    );

    final scrollable = SingleChildScrollView(
      child: Padding(
        padding: EdgeInsets.zero,
        child: body,
      ),
    );

    if (!widget._isFloating) {
      // Dialog mode: plain scrollable body, actions live in the AlertDialog.
      return scrollable;
    }

    // Floating mode: own header + fixed bottom action bar.
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(child: scrollable),
        const Divider(height: 1),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
          child: Row(
            children: [
              TextButton.icon(
                onPressed: _isPreviewing ? null : _preview,
                icon: Icon(_isPreviewing ? Icons.stop : Icons.play_arrow,
                    size: 16),
                label: const Text('试听'),
              ),
              const Spacer(),
              TextButton(
                onPressed: _cancel,
                child: const Text('关闭'),
              ),
              const SizedBox(width: 4),
              FilledButton(
                onPressed: _commit,
                child: const Text('保存'),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _slider(String label, double value, double min, double max,
      ValueChanged<double> onChanged,
      {int fraction = 2, String Function(double)? format}) {
    return Row(
      children: [
        SizedBox(
            width: 84, child: Text(label, style: const TextStyle(fontSize: 11))),
        Expanded(
          child: SliderTheme(
            data: const SliderThemeData(
                trackHeight: 3,
                thumbShape: RoundSliderThumbShape(enabledThumbRadius: 7)),
            child: Slider(
              value: value.clamp(min, max),
              min: min,
              max: max,
              onChanged: onChanged,
            ),
          ),
        ),
        SizedBox(
          width: 58,
          child: Text(
            format?.call(value) ?? value.toStringAsFixed(fraction),
            style: const TextStyle(fontSize: 10),
          ),
        ),
      ],
    );
  }

  Widget _adsrField(String label, double value, ValueChanged<double> onChanged) {
    return Expanded(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 3),
        child: Column(
          children: [
            Text(label, style: const TextStyle(fontSize: 10)),
            TextFormField(
              initialValue: value.toStringAsFixed(3),
              style: const TextStyle(fontSize: 11),
              decoration: const InputDecoration(
                isDense: true,
                border: OutlineInputBorder(),
                contentPadding: EdgeInsets.symmetric(horizontal: 6, vertical: 6),
              ),
              onChanged: (s) {
                final v = double.tryParse(s);
                if (v != null) onChanged(v);
              },
            ),
          ],
        ),
      ),
    );
  }
}

/// Modal wrapper around [SynthEditorPanel] — keeps the previous dialog API.
/// Reuses the panel's floating mode (self-contained header + action bar).
class SynthEditorDialog {
  const SynthEditorDialog._();

  static Future<void> show(BuildContext context, InstrumentPreset preset,
      ValueChanged<InstrumentPreset> onSaved) {
    return showDialog(
      context: context,
      builder: (dialogContext) => Dialog(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 500, maxHeight: 640),
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: SynthEditorPanel(
              preset: preset,
              onSaved: onSaved,
              onClose: () => Navigator.pop(dialogContext),
            ),
          ),
        ),
      ),
    );
  }
}

import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart' hide Track;

import '../../models/envelope.dart';
import '../../models/instrument.dart';
import '../../services/audio_url.dart';
import '../../services/synth_service.dart';
import 'envelope_editor.dart';

/// Dialog for editing a synth-engine instrument: engine selector,
/// engine params, FL-style amplitude envelope, and live preview.
class SynthEditorDialog extends StatefulWidget {
  final InstrumentPreset preset;
  final ValueChanged<InstrumentPreset> onSaved;

  const SynthEditorDialog({
    super.key,
    required this.preset,
    required this.onSaved,
  });

  static Future<void> show(BuildContext context, InstrumentPreset preset,
      ValueChanged<InstrumentPreset> onSaved) {
    return showDialog(
      context: context,
      builder: (_) => SynthEditorDialog(preset: preset, onSaved: onSaved),
    );
  }

  @override
  State<SynthEditorDialog> createState() => _SynthEditorDialogState();
}

class _SynthEditorDialogState extends State<SynthEditorDialog> {
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

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final p = _draft;
    final engine = p.synthEngine ?? 'additive';

    return AlertDialog(
      title: Row(
        children: [
          const Icon(Icons.tune, size: 20),
          const SizedBox(width: 8),
          Expanded(
              child:
                  Text('合成器: ${p.name}', style: const TextStyle(fontSize: 16))),
        ],
      ),
      content: SizedBox(
        width: 470,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
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
                  Text('包络 (FL 曲线)',
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
          ),
        ),
      ),
      actions: [
        TextButton.icon(
          onPressed: _isPreviewing ? null : _preview,
          icon: Icon(_isPreviewing ? Icons.stop : Icons.play_arrow, size: 16),
          label: const Text('试听'),
        ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () {
            widget.onSaved(_draft);
            Navigator.pop(context);
          },
          child: const Text('保存'),
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

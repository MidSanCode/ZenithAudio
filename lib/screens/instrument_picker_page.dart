import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:media_kit/media_kit.dart' hide Track;
import 'package:path_provider/path_provider.dart';
import 'package:file_picker/file_picker.dart';
import '../models/instrument.dart';
import '../services/synth_service.dart';
import '../services/soundfont_service.dart';
import '../services/instrument_pack_service.dart';
import '../widgets/editor/synth_editor_dialog.dart';

/// Full-screen instrument picker with card grid and preview/audition.
class InstrumentPickerPage extends StatefulWidget {
  final String? currentId;

  const InstrumentPickerPage({super.key, this.currentId});

  @override
  State<InstrumentPickerPage> createState() => _InstrumentPickerPageState();
}

class _InstrumentPickerPageState extends State<InstrumentPickerPage> {
  String? _selectedId;
  String? _previewingId;
  Player? _previewPlayer;
  bool _previewDisposed = false;
  final _synth = SynthService();
  final Map<String, Uint8List> _previewCache = {};
  bool _gmLoaded = false;

  @override
  void initState() {
    super.initState();
    _selectedId = widget.currentId;
    _loadGmPresets();
  }

  Future<void> _loadGmPresets() async {
    try {
      final gm = await InstrumentPackService.loadFromAsset('assets/instruments/gm_presets.json');
      InstrumentPreset.addUserPresets(gm);
    } catch (_) {}
    await InstrumentPreset.loadPersisted();
    if (mounted) setState(() => _gmLoaded = true);
  }

  Future<void> _loadSoundFont() async {
    final result = await FilePicker.platform.pickFiles(
      type: FileType.custom,
      allowedExtensions: ['sf2'],
      withData: kIsWeb,
    );
    if (result == null || result.files.isEmpty) return;

    try {
      Uint8List? bytes;
      final label = result.files.first.name;
      if (kIsWeb) {
        bytes = result.files.first.bytes;
      } else {
        final path = result.files.first.path;
        if (path != null) bytes = await File(path).readAsBytes();
      }
      if (bytes == null) throw Exception('无法读取文件内容');

      SoundFontService.instance.loadBytes(bytes, label: label);
      InstrumentPreset.registerSoundFontPresets(SoundFontService.instance.presets
          .map((p) => (program: p.program, bank: p.bank, name: p.name))
          .toList());
      _previewCache.clear();
      if (mounted) {
        setState(() {});
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('instrumentPicker.sf2LoadedToast'.tr(namedArgs: {
              'label': SoundFontService.instance.sourceLabel,
              'count': '${SoundFontService.instance.presets.length}',
            })),
          ),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('instrumentPicker.sf2LoadFailed'.tr(namedArgs: {'error': '$e'}))),
        );
      }
    }
  }

  Future<void> _importFromZip() async {
    final result = await FilePicker.platform.pickFiles(
      type: FileType.custom,
      allowedExtensions: ['zip'],
    );
    if (result == null || result.files.isEmpty) return;
    final path = result.files.first.path;
    if (path == null) return;

    try {
      final presets = await InstrumentPackService.loadFromZip(path);
      if (presets.isEmpty) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('instrumentPicker.zipNoInstruments'.tr())),
          );
        }
        return;
      }
      InstrumentPreset.addUserPresets(presets);
      if (mounted) {
        setState(() {});
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('instrumentPicker.zipImported'.tr(namedArgs: {'n': '${presets.length}'}))),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('instrumentPicker.zipImportFailed'.tr(namedArgs: {'error': '$e'}))),
        );
      }
    }
  }

  @override
  void dispose() {
    _stopPreview();
    super.dispose();
  }

  Future<void> _preview(String id) async {
    if (_previewingId == id) {
      await _stopPreview();
      return;
    }

    // Stop any existing preview
    await _stopPreview();

    final inst = InstrumentPreset.fromId(id);
    Uint8List wavBytes;
    if (_previewCache.containsKey(id)) {
      wavBytes = _previewCache[id]!;
    } else {
      wavBytes = _synth.renderPreviewWav(
        inst,
        pitch: _previewPitchFor(inst),
        duration: 1.6,
        velocity: 100,
      );
      _previewCache[id] = wavBytes;
    }

    if (kIsWeb) {
      await _playPreviewWeb(wavBytes, id);
    } else {
      await _playPreviewDesktop(wavBytes, id);
    }
  }

  /// Audition each instrument in its own comfortable register so family
  /// timbre (bass warmth, wind breathiness) is actually audible.
  int _previewPitchFor(InstrumentPreset inst) {
    switch (inst.category) {
      case InstrumentCategory.string:
        // GM programs 32-39 are the bass family — two octaves below C4.
        return (inst.programNumber >= 32 && inst.programNumber <= 39)
            ? 40 // E2
            : 57; // A3
      case InstrumentCategory.wind:
        return 62; // D4
      default:
        return 60; // C4
    }
  }

  Future<void> _playPreviewDesktop(Uint8List wavBytes, String id) async {
    final dir = await getTemporaryDirectory();
    final filePath = '${dir.path}/preview_$id.wav';
    await File(filePath).writeAsBytes(wavBytes);

    final player = Player();
    _previewPlayer = player;
    _previewDisposed = false;
    if (mounted) setState(() => _previewingId = id);

    // Auto-stop on completion.
    // media_kit emits `completed=false` while open() resets the internal
    // playlist; only react to actual playback completion (true).
    player.stream.completed.listen((completed) {
      if (completed) {
        _onPreviewEnd(id, player, filePath);
      }
    });
    player.stream.error.listen((_) {
      _onPreviewEnd(id, player, filePath);
    });

    try {
      final uri = Uri.file(filePath).toString();
      await player.open(Media(uri));
      await player.setVolume(100);
      player.play();
    } catch (_) {
      _onPreviewEnd(id, player, filePath);
    }
  }

  Future<void> _onPreviewEnd(String id, Player player, String filePath) async {
    if (_previewDisposed && _previewPlayer != player) return;
    if (mounted && _previewingId == id) {
      setState(() => _previewingId = null);
    }
    _previewDisposed = true;
    player.dispose();
    if (_previewPlayer == player) _previewPlayer = null;
    try { await File(filePath).delete(); } catch (_) {}
  }

  Future<void> _playPreviewWeb(Uint8List wavBytes, String id) async {
    // NOTE: web is currently a stub — no audible playback. This file imports
    // dart:io unconditionally, so it cannot compile for web targets anyway;
    // making web audible would require moving File IO behind conditional
    // imports first.
    setState(() => _previewingId = id);
    await Future.delayed(const Duration(milliseconds: 900));
    if (mounted && _previewingId == id) {
      setState(() => _previewingId = null);
    }
  }

  Future<void> _stopPreview() async {
    await _previewPlayer?.stop();
    await _previewPlayer?.dispose();
    _previewPlayer = null;
    if (mounted) setState(() => _previewingId = null);
  }

  bool _isEditable(InstrumentPreset p) =>
      p.id.startsWith('syn_') || p.id.startsWith('custom_');

  Future<void> _openSynthEditor(InstrumentPreset preset) async {
    await SynthEditorDialog.show(context, preset, (edited) async {
      // Built-ins become a new saved copy; customs overwrite in place.
      final savedId = await saveSynthEdit(edited);
      _previewCache.clear();
      if (mounted) setState(() => _selectedId = savedId);
    });
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;

    final grouped = <String, List<InstrumentPreset>>{};
    for (final preset in InstrumentPreset.allPresets) {
      grouped.putIfAbsent(_catLabel(preset.category), () => []).add(preset);
    }

    // Separate user-imported presets for their own section
    final userGroup = InstrumentPreset.userPresets.isNotEmpty
        ? {'instrumentPicker.imported'.tr(): InstrumentPreset.userPresets}
        : <String, List<InstrumentPreset>>{};

    if (!_gmLoaded) {
      return Scaffold(
        backgroundColor: cs.surface,
        appBar: AppBar(
          backgroundColor: cs.surface,
          elevation: 0,
          leading: IconButton(
            icon: const Icon(Icons.arrow_back),
            onPressed: () => Navigator.of(context).pop(),
          ),
          title: Text('instrumentPicker.title'.tr()),
        ),
        body: const Center(child: CircularProgressIndicator()),
      );
    }

    return Scaffold(
      backgroundColor: cs.surface,
      appBar: AppBar(
        backgroundColor: cs.surface,
        elevation: 0,
        leading: IconButton(
          icon: const Icon(Icons.arrow_back),
          onPressed: () => Navigator.of(context).pop(),
        ),
        title: Text('instrumentPicker.title'.tr()),
        actions: [
          IconButton(
            icon: const Icon(Icons.library_music, size: 20),
            tooltip: 'instrumentPicker.tooltipLoadSf2'.tr(),
            onPressed: _loadSoundFont,
          ),
          IconButton(
            icon: const Icon(Icons.folder_open, size: 20),
            tooltip: 'instrumentPicker.tooltipImportPack'.tr(),
            onPressed: _importFromZip,
          ),
          if (_selectedId != null)
            TextButton(
              onPressed: () => Navigator.of(context).pop(_selectedId),
              child: Text('instrumentPicker.done'.tr(), style: TextStyle(color: cs.primary)),
            ),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 32),
        children: [
          for (final entry in grouped.entries) ...[
            Padding(
              padding: const EdgeInsets.only(top: 16, bottom: 8),
              child: Text(
                entry.key,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: cs.primary,
                  letterSpacing: 1,
                ),
              ),
            ),
            ...entry.value.map((inst) => _InstrumentCard(
              preset: inst,
              isSelected: inst.id == _selectedId,
              isPreviewing: _previewingId == inst.id,
              onTap: () => setState(() => _selectedId = inst.id),
              onPreview: () => _preview(inst.id),
              onEdit: _isEditable(inst) ? () => _openSynthEditor(inst) : null,
            )),
          ],
          // User-imported instruments section
          if (userGroup.isNotEmpty) ...[
            for (final entry in userGroup.entries) ...[
              Padding(
                padding: const EdgeInsets.only(top: 20, bottom: 8),
                child: Row(
                  children: [
                    Text(
                      entry.key,
                      style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: cs.primary,
                        letterSpacing: 1,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Icon(Icons.person, size: 12, color: cs.primary.withAlpha(150)),
                  ],
                ),
              ),
              ...entry.value.map((inst) => _InstrumentCard(
                preset: inst,
                isSelected: inst.id == _selectedId,
                isPreviewing: _previewingId == inst.id,
                onTap: () => setState(() => _selectedId = inst.id),
                onPreview: () => _preview(inst.id),
                onEdit: _isEditable(inst) ? () => _openSynthEditor(inst) : null,
              )),
            ],
          ],
          // SoundFont status banner
          if (SoundFontService.instance.isLoaded)
            Padding(
              padding: const EdgeInsets.only(top: 16),
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                decoration: BoxDecoration(
                  color: cs.primaryContainer.withAlpha(80),
                  borderRadius: BorderRadius.circular(10),
                  border: Border.all(color: cs.primary.withAlpha(120)),
                ),
                child: Row(
                  children: [
                    Icon(Icons.library_music, size: 16, color: cs.primary),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        'instrumentPicker.sf2LoadedBanner'.tr(namedArgs: {
                          'label': SoundFontService.instance.sourceLabel,
                          'count': '${SoundFontService.instance.presets.length}',
                        }),
                        style: const TextStyle(fontSize: 11),
                      ),
                    ),
                    TextButton(
                      onPressed: () {
                        SoundFontService.instance.unload();
                        InstrumentPreset.clearSoundFontPresets();
                        _previewCache.clear();
                        setState(() {});
                      },
                      child: Text('instrumentPicker.unload'.tr(), style: const TextStyle(fontSize: 11)),
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }

  String _catLabel(InstrumentCategory cat) {
    switch (cat) {
      case InstrumentCategory.keyboard: return 'instrumentPicker.cat.keyboard'.tr();
      case InstrumentCategory.string: return 'instrumentPicker.cat.string'.tr();
      case InstrumentCategory.wind: return 'instrumentPicker.cat.wind'.tr();
      case InstrumentCategory.synth: return 'instrumentPicker.cat.synth'.tr();
      case InstrumentCategory.percussion: return 'instrumentPicker.cat.percussion'.tr();
    }
  }
}

class _InstrumentCard extends StatelessWidget {
  final InstrumentPreset preset;
  final bool isSelected;
  final bool isPreviewing;
  final VoidCallback onTap;
  final VoidCallback onPreview;
  final VoidCallback? onEdit;

  const _InstrumentCard({
    required this.preset,
    required this.isSelected,
    required this.isPreviewing,
    required this.onTap,
    required this.onPreview,
    this.onEdit,
  });

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;

    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 150),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
          decoration: BoxDecoration(
            color: isSelected
                ? cs.primaryContainer.withAlpha(100)
                : cs.surfaceContainerLow,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
              color: isSelected
                  ? cs.primary
                  : cs.outlineVariant.withAlpha(100),
              width: isSelected ? 1.5 : 1,
            ),
          ),
          child: Row(
            children: [
              Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(
                  color: isSelected
                      ? cs.primary.withAlpha(30)
                      : cs.surfaceContainerHighest,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: Icon(
                  preset.icon,
                  size: 22,
                  color: isSelected ? cs.primary : cs.onSurfaceVariant,
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      preset.name,
                      style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w500,
                        color: cs.onSurface,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      preset.description,
                      style: TextStyle(fontSize: 10, color: cs.onSurfaceVariant),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ],
                ),
              ),
              IconButton(
                icon: Icon(
                  isPreviewing ? Icons.stop_circle_outlined : Icons.play_circle_outline,
                  size: 28,
                  color: cs.primary,
                ),
                onPressed: onPreview,
                tooltip: 'instrumentPicker.preview'.tr(),
              ),
              if (onEdit != null)
                IconButton(
                  icon: const Icon(Icons.tune, size: 20),
                  onPressed: onEdit,
                  tooltip: 'instrumentPicker.tooltipSynthEdit'.tr(),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

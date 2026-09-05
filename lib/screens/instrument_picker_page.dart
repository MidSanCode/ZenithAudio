import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart' hide Track;
import 'package:path_provider/path_provider.dart';
import 'package:file_picker/file_picker.dart';
import '../models/instrument.dart';
import '../services/synth_service.dart';
import '../services/instrument_pack_service.dart';

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
    if (mounted) setState(() => _gmLoaded = true);
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
            const SnackBar(content: Text('No instrument definitions found in ZIP.')),
          );
        }
        return;
      }
      InstrumentPreset.addUserPresets(presets);
      if (mounted) {
        setState(() {});
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Imported ${presets.length} instruments.')),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Failed to import: $e')),
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
      wavBytes = _synth.renderPreviewWav(inst, pitch: 72, duration: 0.8, velocity: 100);
      _previewCache[id] = wavBytes;
    }

    if (kIsWeb) {
      await _playPreviewWeb(wavBytes, id);
    } else {
      await _playPreviewDesktop(wavBytes, id);
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

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;

    final grouped = <String, List<InstrumentPreset>>{};
    for (final preset in InstrumentPreset.allPresets) {
      grouped.putIfAbsent(_catLabel(preset.category), () => []).add(preset);
    }

    // Separate user-imported presets for their own section
    final userGroup = InstrumentPreset.userPresets.isNotEmpty
        ? {'Imported': InstrumentPreset.userPresets}
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
          title: const Text('Select Instrument'),
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
        title: const Text('Select Instrument'),
        actions: [
          IconButton(
            icon: const Icon(Icons.folder_open, size: 20),
            tooltip: 'Import SoundFont / Instrument Pack (ZIP)',
            onPressed: _importFromZip,
          ),
          if (_selectedId != null)
            TextButton(
              onPressed: () => Navigator.of(context).pop(_selectedId),
              child: Text('Done', style: TextStyle(color: cs.primary)),
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
              )),
            ],
          ],
        ],
      ),
    );
  }

  String _catLabel(InstrumentCategory cat) {
    switch (cat) {
      case InstrumentCategory.keyboard: return 'KEYBOARDS';
      case InstrumentCategory.string: return 'STRINGS';
      case InstrumentCategory.wind: return 'BRASS & WIND';
      case InstrumentCategory.synth: return 'SYNTHS';
      case InstrumentCategory.percussion: return 'PERCUSSION';
    }
  }
}

class _InstrumentCard extends StatelessWidget {
  final InstrumentPreset preset;
  final bool isSelected;
  final bool isPreviewing;
  final VoidCallback onTap;
  final VoidCallback onPreview;

  const _InstrumentCard({
    required this.preset,
    required this.isSelected,
    required this.isPreviewing,
    required this.onTap,
    required this.onPreview,
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
                tooltip: 'Preview',
              ),
            ],
          ),
        ),
      ),
    );
  }
}

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/theme_colors.dart';
import '../../core/instrument_picker.dart';
import '../../providers/project_provider.dart';
import '../../providers/playback_provider.dart';
import '../editor/synth_editor_dialog.dart' show SynthEditorLauncher;
import '../../services/file_service.dart';
import '../../services/audio_converter.dart';
import '../../screens/settings_page.dart';
import '../../screens/about_dialog.dart' as app;
import '../../core/utils/logger.dart';

class AudioMenuBar extends ConsumerWidget {
  const AudioMenuBar({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      height: AppConstants.menuBarHeight,
      decoration: BoxDecoration(
        color: cs.surface,
        border: Border(
          bottom: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          _MenuButton(
            label: 'menu.file'.tr(),
            items: [
              MenuItem(
                label: 'menu.file.workspace'.tr(),
                onTap: () async {
                  final navigator = Navigator.of(context);
                  if (!navigator.canPop()) return;
                  // Ask about unsaved changes before leaving the editor.
                  final ok = await ref.read(projectProvider.notifier).confirmDiscard(context);
                  if (ok) navigator.pop();
                },
              ),
              MenuItem.separator(),
              MenuItem(
                label: 'menu.file.newProject'.tr(),
                shortcut: 'shortcut.newProject'.tr(),
                onTap: () async {
                  await ref.read(projectProvider.notifier).tryNewProject(context);
                  AppLogger.i('New project created');
                },
              ),
              MenuItem(
                label: 'menu.file.openProject'.tr(),
                shortcut: 'shortcut.openProject'.tr(),
                onTap: () async {
                  final notifier = ref.read(projectProvider.notifier);
                  await notifier.openProject();
                },
              ),
              MenuItem(
                label: 'menu.file.saveProject'.tr(),
                shortcut: 'shortcut.saveProject'.tr(),
                onTap: () async {
                  final notifier = ref.read(projectProvider.notifier);
                  await notifier.saveProject();
                },
              ),
              MenuItem.separator(),
              MenuItem(
                label: 'menu.file.exportProject'.tr(),
                shortcut: 'shortcut.exportProject'.tr(),
                onTap: () async {
                  await ref.read(projectProvider.notifier).exportProject();
                },
              ),
              MenuItem.separator(),
              MenuItem(
                label: 'menu.file.settings'.tr(),
                onTap: () => _showSettings(context),
              ),
              MenuItem.separator(),
              MenuItem(
                label: 'menu.file.exit'.tr(),
                shortcut: 'shortcut.exit'.tr(),
              ),
            ],
          ),
          _MenuButton(
            label: 'menu.edit'.tr(),
            items: [
              MenuItem(
                label: 'menu.edit.undo'.tr(),
                shortcut: 'shortcut.undo'.tr(),
                onTap: () => ref.read(projectProvider.notifier).undo(),
              ),
              MenuItem(
                label: 'menu.edit.redo'.tr(),
                shortcut: 'shortcut.redo'.tr(),
                onTap: () => ref.read(projectProvider.notifier).redo(),
              ),
              const MenuItem.separator(),
              MenuItem(label: 'menu.edit.deleteTrack'.tr()),
            ],
          ),
          _MenuButton(
            label: 'menu.track'.tr(),
            items: [
              MenuItem(
                label: 'menu.track.importAudio'.tr(),
                shortcut: 'shortcut.importAudio'.tr(),
                onTap: () async {
                  final fileService = FileService();
                  final result = await fileService.pickAudioFile();
                  if (result != null && context.mounted) {
                    String? audioPath = result.audioSource;
                    if (!isWavFile(audioPath)) {
                      audioPath = await convertToWav(audioPath);
                      if (audioPath == null) {
                        if (context.mounted) {
                          ScaffoldMessenger.of(context).showSnackBar(
                            SnackBar(
                              content: Text('menu.wavOnly'.tr()),
                              duration: const Duration(seconds: 5),
                            ),
                          );
                        }
                        return;
                      }
                    }
                    final trackIndex = ref.read(projectProvider).tracks.length + 1;
                    final name = 'track.defaultName'.tr(namedArgs: {'n': '$trackIndex'});
                    ref.read(projectProvider.notifier).addTrack(
                          name: name,
                          audioFilePath: audioPath,
                        );
                    AppLogger.i('Imported audio: ${result.name}');
                  }
                },
              ),
              MenuItem(
                label: 'menu.track.importMidi'.tr(),
                shortcut: 'shortcut.importMidi'.tr(),
                onTap: () async {
                  AppLogger.i('MIDI import not yet implemented');
                },
              ),
              const MenuItem.separator(),
              MenuItem(
                label: 'menu.track.addInstrument'.tr(),
                onTap: () async {
                  final instrument = await showInstrumentPicker(context);
                  if (instrument == null) return;
                  final trackIndex = ref.read(projectProvider).tracks.length + 1;
                  final name = 'Track $trackIndex';
                  ref.read(projectProvider.notifier).addInstrumentTrack(name: name, instrumentName: instrument);
                  AppLogger.i('Added instrument track: $name ($instrument)');
                },
              ),
              MenuItem(
                label: 'menu.track.addSynth'.tr(),
                onTap: () {
                  final trackIndex = ref.read(projectProvider).tracks.length + 1;
                  final name = 'Synth $trackIndex';
                  final notifier = ref.read(projectProvider.notifier);
                  notifier.addInstrumentTrack(name: name, instrumentName: 'syn_supersaw');
                  final newTrack = ref.read(projectProvider).tracks.last;
                  SynthEditorLauncher.openForTrack(context, ref, newTrack);
                  AppLogger.i('Added synth track: $name');
                },
              ),
              MenuItem.disabled(
                label: 'menu.track.chooseTemplate'.tr(),
                disabledHint: 'menu.track.comingSoon'.tr(),
              ),
              const MenuItem.separator(),
              MenuItem(
                label: 'menu.track.add'.tr(),
                shortcut: 'shortcut.addTrack'.tr(),
                onTap: () {
                  final trackIndex = ref.read(projectProvider).tracks.length + 1;
                  final name = 'track.defaultName'.tr(namedArgs: {'n': '$trackIndex'});
                  ref.read(projectProvider.notifier).addTrack(name: name);
                  AppLogger.i('Added track: $name');
                },
              ),
              MenuItem(label: 'menu.track.rename'.tr()),
              const MenuItem.separator(),
              MenuItem(label: 'menu.track.muteAll'.tr()),
              MenuItem(label: 'menu.track.unmuteAll'.tr()),
            ],
          ),
          _MenuButton(
            label: 'menu.view'.tr(),
            items: [
              MenuItem(
                label: 'menu.view.zoomIn'.tr(),
                onTap: () {
                  final cur = ref.read(pixelsPerSecondProvider);
                  ref.read(pixelsPerSecondProvider.notifier).state =
                      (cur * 1.25).clamp(10, 500);
                },
              ),
              MenuItem(
                label: 'menu.view.zoomOut'.tr(),
                onTap: () {
                  final cur = ref.read(pixelsPerSecondProvider);
                  ref.read(pixelsPerSecondProvider.notifier).state =
                      (cur / 1.25).clamp(10, 500);
                },
              ),
              MenuItem(label: 'menu.view.fitWindow'.tr()),
            ],
          ),
          _MenuButton(
            label: 'menu.help'.tr(),
            items: [
              MenuItem(
                label: 'menu.help.about'.tr(),
                onTap: () => showDialog(
                  context: context,
                  builder: (_) => const app.AboutDialog(),
                ),
              ),
            ],
          ),
          const Spacer(),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Text(
              AppConstants.appNameEn,
              style: TextStyle(
                color: cs.primary,
                fontSize: 11,
                fontWeight: FontWeight.w700,
                letterSpacing: 2,
              ),
            ),
          ),
        ],
      ),
    );
  }

  void _showSettings(BuildContext context) {
    Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const SettingsPage()),
    );
  }
}

class MenuItem {
  final String label;
  final String? shortcut;
  final VoidCallback? onTap;
  final bool isSeparator;
  final bool isDisabled;
  final String? disabledHint;

  const MenuItem({
    required this.label,
    this.shortcut,
    this.onTap,
    this.isSeparator = false,
    this.isDisabled = false,
    this.disabledHint,
  });

  const MenuItem.separator()
      : label = '',
        shortcut = null,
        onTap = null,
        isSeparator = true,
        isDisabled = false,
        disabledHint = null;

  const MenuItem.disabled({
    required this.label,
    this.disabledHint,
  }) : shortcut = null,
       onTap = null,
       isSeparator = false,
       isDisabled = true;
}

class _MenuButton extends StatefulWidget {
  final String label;
  final List<MenuItem> items;

  const _MenuButton({required this.label, required this.items});

  @override
  State<_MenuButton> createState() => _MenuButtonState();
}

class _MenuButtonState extends State<_MenuButton> {
  bool _isHovered = false;
  bool _isOpen = false;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return MouseRegion(
      onEnter: (_) => setState(() => _isHovered = true),
      onExit: (_) => setState(() => _isHovered = false),
      child: GestureDetector(
        onTap: () {
          setState(() => _isOpen = !_isOpen);
          if (_isOpen) {
            _showMenu(context);
          }
        },
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: _isHovered ? context.surfaceHigh : Colors.transparent,
          ),
          child: Text(
            widget.label,
            style: TextStyle(
              color: _isHovered ? cs.onSurface : cs.onSurfaceVariant,
              fontSize: 12,
            ),
          ),
        ),
      ),
    );
  }

  void _showMenu(BuildContext context) async {
    await showMenu<MenuItem>(
      context: context,
      position: RelativeRect.fromLTRB(
        _getPosition(), 0, _getPosition() + 100, 0,
      ),
      initialValue: null,
      items: [
        for (final item in widget.items)
          if (item.isSeparator)
            const PopupMenuDivider(height: 1)
          else
            PopupMenuItem<MenuItem>(
              value: item,
              enabled: !item.isDisabled && item.onTap != null,
              onTap: item.onTap,
              height: 28,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (item.isDisabled && item.disabledHint != null)
                    Padding(
                      padding: const EdgeInsets.only(right: 4),
                      child: Icon(Icons.lock_outline, size: 12,
                          color: context.outline.withAlpha(128)),
                    ),
                  Text(
                    item.label,
                    style: TextStyle(
                      fontSize: 12,
                      color: item.isDisabled
                          ? context.outline.withAlpha(128)
                          : null,
                    ),
                  ),
                  if (item.shortcut != null) ...[
                    const Spacer(),
                    Text(
                      item.shortcut!,
                      style: TextStyle(fontSize: 10,
                          color: context.outline.withAlpha(item.isDisabled ? 128 : 255)),
                    ),
                  ],
                ],
              ),
            ),
      ],
    );
    setState(() => _isOpen = false);
  }

  double _getPosition() {
    final box = context.findRenderObject() as RenderBox?;
    if (box == null) return 0;
    return box.localToGlobal(Offset.zero).dx;
  }
}

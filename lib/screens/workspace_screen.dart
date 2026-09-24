import 'dart:io' show Platform;

import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../core/constants/app_constants.dart';
import '../core/constants/app_config.dart';
import '../core/utils/theme_colors.dart';
import '../providers/project_provider.dart';
import '../providers/workspace_provider.dart';
import '../services/workspace_service.dart';
import 'about_dialog.dart' as app;
import 'editor_screen.dart';
import 'settings_page.dart';

/// Photoshop-style home screen shown when the app starts.
///
/// Left column: branding + New / Open actions.
/// Right area: recent projects stored in the app workspace.
class WorkspaceScreen extends ConsumerWidget {
  const WorkspaceScreen({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    return Scaffold(
      backgroundColor: cs.surface,
      body: SafeArea(
        child: Column(
          children: [
            const _WorkspaceTopBar(),
            Divider(height: 1, color: Theme.of(context).dividerColor, thickness: 0.5),
            Expanded(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const _ActionColumn(),
                  VerticalDivider(width: 1, thickness: 0.5, color: Theme.of(context).dividerColor),
                  const Expanded(child: _RecentProjects()),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ──── Top bar ────

class _WorkspaceTopBar extends ConsumerWidget {
  const _WorkspaceTopBar();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      height: 34,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      color: cs.surfaceContainerLow,
      child: Row(
        children: [
          Icon(Icons.waves, size: 15, color: cs.primary),
          const SizedBox(width: 7),
          Text(
            AppConstants.appNameEn,
            style: TextStyle(
              color: cs.primary,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 2,
            ),
          ),
          const Spacer(),
          _TopBarAction(
            icon: Icons.tune_outlined,
            tooltip: 'menu.file.settings'.tr(),
            onTap: () => Navigator.of(context).push(
              MaterialPageRoute(builder: (_) => const SettingsPage()),
            ),
          ),
          _TopBarAction(
            icon: Icons.info_outline_rounded,
            tooltip: 'menu.help.about'.tr(),
            onTap: () => showDialog(
              context: context,
              builder: (_) => const app.AboutDialog(),
            ),
          ),
        ],
      ),
    );
  }
}

class _TopBarAction extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;

  const _TopBarAction({required this.icon, required this.tooltip, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return IconButton(
      icon: Icon(icon, size: 15),
      color: cs.onSurfaceVariant,
      tooltip: tooltip,
      padding: EdgeInsets.zero,
      constraints: const BoxConstraints.tightFor(width: 30, height: 30),
      onPressed: onTap,
    );
  }
}

// ──── Left action column ────

class _ActionColumn extends ConsumerWidget {
  const _ActionColumn();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      width: 260,
      color: cs.surfaceContainerLow,
      padding: const EdgeInsets.fromLTRB(20, 26, 20, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Brand block
          Row(
            children: [
              Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(
                  color: cs.primary,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Icon(Icons.waves, size: 26, color: Colors.white),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      AppConstants.appName,
                      style: TextStyle(
                        color: cs.onSurface,
                        fontSize: 18,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    Text(
                      AppConstants.appNameEn,
                      style: TextStyle(
                        color: cs.primary,
                        fontSize: 9,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 1.6,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            'workspace.version'.tr(namedArgs: {'v': AppConfig.appVersion}),
            style: TextStyle(color: context.outline, fontSize: 10),
          ),
          const SizedBox(height: 28),

          // Primary actions
          _PrimaryAction(
            icon: Icons.add_rounded,
            label: 'workspace.newProject'.tr(),
            filled: true,
            onTap: () async {
              await ref.read(projectProvider.notifier).forceNewProject();
              if (!context.mounted) return;
              await Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const EditorScreen()),
              );
              refreshWorkspace(ref);
            },
          ),
          const SizedBox(height: 10),
          _PrimaryAction(
            icon: Icons.folder_open_rounded,
            label: 'workspace.openProject'.tr(),
            filled: false,
            onTap: () async {
              final ok = await ref.read(projectProvider.notifier).openProject();
              if (!ok || !context.mounted) return;
              await Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const EditorScreen()),
              );
              refreshWorkspace(ref);
            },
          ),
          const SizedBox(height: 6),
          _PrimaryAction(
            icon: Icons.drive_file_move_outline,
            label: 'workspace.openFolder'.tr(),
            filled: false,
            compact: true,
            onTap: () async {
              final ok =
                  await ref.read(projectProvider.notifier).openProjectFolder();
              if (!ok || !context.mounted) return;
              await Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const EditorScreen()),
              );
              refreshWorkspace(ref);
            },
          ),

          const Spacer(),
          Text(
            'workspace.formatHint'.tr(),
            style: TextStyle(color: context.outline, fontSize: 9, height: 1.4),
          ),
          const SizedBox(height: 6),
          if (_isIOS)
            Text(
              'workspace.filesAppHint'.tr(),
              style: TextStyle(color: cs.primary.withAlpha(180), fontSize: 9, height: 1.4),
            ),
          if (_isIOS) const SizedBox(height: 6),
          Divider(color: Theme.of(context).dividerColor, thickness: 0.5),
          const SizedBox(height: 10),
          Text(
            'workspace.location'.tr(),
            style: TextStyle(color: context.outline, fontSize: 9, fontWeight: FontWeight.w600, letterSpacing: 1),
          ),
          const SizedBox(height: 4),
          const _WorkspaceLocation(),
        ],
      ),
    );
  }

  static bool get _isIOS {
    if (kIsWeb) return false;
    try {
      return Platform.isIOS;
    } catch (_) {
      return false;
    }
  }
}

class _WorkspaceLocation extends StatelessWidget {
  const _WorkspaceLocation();

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return FutureBuilder<String>(
      future: WorkspaceService().directoryPath(),
      builder: (context, snapshot) => Text(
        snapshot.data ?? '',
        maxLines: 3,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(color: cs.onSurfaceVariant, fontSize: 9, height: 1.35),
      ),
    );
  }
}

class _PrimaryAction extends StatefulWidget {
  final IconData icon;
  final String label;
  final bool filled;
  final VoidCallback onTap;

  /// Compact actions render shorter and with a lighter weight.
  final bool compact;

  const _PrimaryAction({
    required this.icon,
    required this.label,
    required this.filled,
    required this.onTap,
    this.compact = false,
  });

  @override
  State<_PrimaryAction> createState() => _PrimaryActionState();
}

class _PrimaryActionState extends State<_PrimaryAction> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final bg = widget.filled
        ? (_hovered ? cs.primary.withAlpha(230) : cs.primary)
        : (_hovered ? cs.surfaceContainerHighest : cs.surfaceContainerHigh);
    final fg = widget.filled ? Colors.white : cs.onSurface;

    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: widget.compact ? 32 : 40,
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(6),
            border: widget.filled ? null : Border.all(color: cs.outlineVariant),
          ),
          child: Row(
            children: [
              SizedBox(width: widget.compact ? 12 : 14),
              Icon(widget.icon, size: widget.compact ? 14 : 16, color: fg),
              const SizedBox(width: 10),
              Text(
                widget.label,
                style: TextStyle(
                  color: fg,
                  fontSize: widget.compact ? 12 : 13,
                  fontWeight: widget.compact ? FontWeight.w500 : FontWeight.w600,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// ──── Recent projects ────

class _RecentProjects extends ConsumerWidget {
  const _RecentProjects();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final cs = Theme.of(context).colorScheme;
    final projects = ref.watch(workspaceProjectsProvider);

    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text(
                'workspace.recent'.tr(),
                style: TextStyle(
                  color: cs.onSurface,
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Spacer(),
              IconButton(
                icon: const Icon(Icons.refresh_rounded, size: 16),
                color: cs.onSurfaceVariant,
                tooltip: 'workspace.refresh'.tr(),
                onPressed: () => refreshWorkspace(ref),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Expanded(
            child: projects.when(
              loading: () => const Center(
                child: SizedBox(width: 22, height: 22, child: CircularProgressIndicator(strokeWidth: 2)),
              ),
              error: (_, _) => Center(
                child: Text('workspace.error'.tr(), style: TextStyle(color: cs.onSurfaceVariant, fontSize: 12)),
              ),
              data: (files) => files.isEmpty
                  ? const _EmptyWorkspace()
                  : _ProjectGrid(files: files),
            ),
          ),
        ],
      ),
    );
  }
}

class _ProjectGrid extends StatelessWidget {
  final List<WorkspaceProjectFile> files;
  const _ProjectGrid({required this.files});

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        // Cards keep a comfortable width and reflow with the window.
        const targetWidth = 190.0;
        final columns = (constraints.maxWidth / targetWidth).floor().clamp(1, 6);
        return GridView.builder(
          padding: const EdgeInsets.only(bottom: 8),
          gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: columns,
            mainAxisSpacing: 12,
            crossAxisSpacing: 12,
            mainAxisExtent: 132,
          ),
          itemCount: files.length,
          itemBuilder: (context, index) => _ProjectCard(file: files[index]),
        );
      },
    );
  }
}

class _ProjectCard extends ConsumerStatefulWidget {
  final WorkspaceProjectFile file;
  const _ProjectCard({required this.file});

  @override
  ConsumerState<_ProjectCard> createState() => _ProjectCardState();
}

class _ProjectCardState extends ConsumerState<_ProjectCard> {
  bool _hovered = false;

  Future<void> _open() async {
    final ok = await ref.read(projectProvider.notifier).openWorkspaceProject(widget.file.path);
    if (!ok) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(
        content: Text('workspace.openFailed'.tr()),
        duration: const Duration(seconds: 2),
      ));
      return;
    }
    if (!mounted) return;
    await Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const EditorScreen()),
    );
    refreshWorkspace(ref);
  }

  Future<void> _export() async {
    final ok = await ref.read(projectProvider.notifier).exportWorkspaceFile(widget.file.path);
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(
      content: Text(ok
          ? 'workspace.exportDone'.tr(namedArgs: {'path': widget.file.label})
          : 'workspace.exportFailed'.tr()),
      duration: const Duration(seconds: 2),
    ));
  }

  Future<void> _delete() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text('workspace.deleteTitle'.tr()),
        content: Text('workspace.deleteConfirm'.tr(namedArgs: {'name': widget.file.label})),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text('common.cancel'.tr()),
          ),
          FilledButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: Text('common.delete'.tr()),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    await ref.read(projectProvider.notifier).deleteWorkspaceFile(widget.file.path);
    refreshWorkspace(ref);
  }

  String get _date {
    final d = widget.file.modified;
    return '${d.year}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')} '
        '${d.hour.toString().padLeft(2, '0')}:${d.minute.toString().padLeft(2, '0')}';
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final sizeKb = (widget.file.size / 1024).ceil();

    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: _open,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          decoration: BoxDecoration(
            color: _hovered ? cs.surfaceContainerHigh : cs.surfaceContainerLow,
            borderRadius: BorderRadius.circular(8),
            border: Border.all(
              color: _hovered ? cs.primary.withAlpha(140) : cs.outlineVariant,
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // Thumbnail strip
              Expanded(
                child: Container(
                  decoration: BoxDecoration(
                    color: Colors.black.withAlpha(60),
                    borderRadius: const BorderRadius.vertical(top: Radius.circular(7)),
                  ),
                  child: Stack(
                    children: [
                      Center(
                        child: Icon(
                          widget.file.isDirectory
                              ? Icons.folder_rounded
                              : Icons.folder_zip_rounded,
                          size: 26,
                          color: cs.primary.withAlpha(_hovered ? 200 : 110),
                        ),
                      ),
                      if (_hovered)
                        Positioned(
                          top: 4,
                          right: 4,
                          child: Row(
                            children: [
                              _CardAction(
                                icon: Icons.file_upload_outlined,
                                tooltip: 'workspace.export'.tr(),
                                onTap: _export,
                              ),
                              const SizedBox(width: 2),
                              _CardAction(
                                icon: Icons.delete_outline_rounded,
                                tooltip: 'workspace.delete'.tr(),
                                onTap: _delete,
                              ),
                            ],
                          ),
                        ),
                    ],
                  ),
                ),
              ),
              // Caption
              Padding(
                padding: const EdgeInsets.fromLTRB(9, 7, 9, 8),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      widget.file.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: cs.onSurface,
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 3),
                    Text(
                      '$_date · $sizeKb KB',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(color: cs.onSurfaceVariant, fontSize: 9),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _CardAction extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;

  const _CardAction({required this.icon, required this.tooltip, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Tooltip(
      message: tooltip,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            color: cs.surfaceContainerHighest.withAlpha(230),
            borderRadius: BorderRadius.circular(4),
          ),
          child: Icon(icon, size: 13, color: cs.onSurface),
        ),
      ),
    );
  }
}

class _EmptyWorkspace extends StatelessWidget {
  const _EmptyWorkspace();

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.library_music_outlined, size: 40, color: cs.onSurfaceVariant.withAlpha(120)),
          const SizedBox(height: 12),
          Text(
            'workspace.emptyTitle'.tr(),
            style: TextStyle(color: cs.onSurfaceVariant, fontSize: 13),
          ),
          const SizedBox(height: 5),
          Text(
            'workspace.emptyHint'.tr(),
            style: TextStyle(color: context.outline, fontSize: 11),
          ),
        ],
      ),
    );
  }
}

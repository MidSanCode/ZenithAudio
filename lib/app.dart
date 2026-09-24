import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:window_manager/window_manager.dart';
import 'core/theme/app_theme.dart';
import 'core/utils/logger.dart';
import 'providers/project_provider.dart';
import 'providers/settings_provider.dart';
import 'screens/editor_screen.dart';
import 'screens/workspace_screen.dart';
import 'services/project_serializer.dart';
import 'services/single_instance.dart';

class ZenithAudioApp extends ConsumerStatefulWidget {
  const ZenithAudioApp({super.key, this.initialProjectPath});

  /// Project archive handed to us by the OS file association, if any.
  final String? initialProjectPath;

  @override
  ConsumerState<ZenithAudioApp> createState() => _ZenithAudioAppState();
}

class _ZenithAudioAppState extends ConsumerState<ZenithAudioApp>
    with WindowListener {
  final GlobalKey<NavigatorState> _rootNavKey = GlobalKey<NavigatorState>();
  bool _closeDialogOpen = false;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    // A later launch hands its project over instead of opening a new window.
    SingleInstance.startWatching(_onHandoff);
    final path = widget.initialProjectPath;
    if (path != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _openProjectPath(path));
    }
  }

  /// Opens a project handed over by a later launch.
  void _onHandoff(String path) {
    if (!mounted) return;
    _openProjectPath(path);
  }

  /// Opens a project path from the OS and shows it in the editor.
  Future<void> _openProjectPath(String path) async {
    final navigator = _rootNavKey.currentState;
    if (navigator == null) return;
    try {
      final notifier = ref.read(projectProvider.notifier);
      final isDirectory = await Directory(path).exists();
      final opened = isDirectory
          ? await notifier.openWorkspaceProject(path)
          : await _openArchive(notifier, path);
      if (!opened) {
        AppLogger.w('Could not open project path: $path');
        return;
      }
      if (!mounted) return;
      navigator.push(MaterialPageRoute(builder: (_) => const EditorScreen()));
    } catch (e) {
      AppLogger.e('Failed to open project path', e);
    }
  }

  /// Opens an archive file (`.zaproj` / `.lgdf` / `.zap`).
  Future<bool> _openArchive(ProjectNotifier notifier, String path) async {
    final serialized =
        await const ProjectSerializer().deserialize(await File(path).readAsBytes());
    if (serialized == null) return false;
    return notifier.loadSerializedProject(serialized);
  }

  @override
  void dispose() {
    windowManager.removeListener(this);
    SingleInstance.release();
    super.dispose();
  }

  /// OS close button pressed (desktop, preventClose enabled in main()).
  /// Ask to save unsaved changes before actually destroying the window.
  @override
  void onWindowClose() async {
    if (_closeDialogOpen) return;
    _closeDialogOpen = true;
    try {
      final navCtx = _rootNavKey.currentContext;
      if (navCtx == null) {
        // No navigator yet (early shutdown) — close without asking.
        await windowManager.setPreventClose(false);
        await windowManager.destroy();
        return;
      }
      final confirmed =
          await ref.read(projectProvider.notifier).confirmDiscard(navCtx);
      if (!confirmed) return; // cancelled — keep the app open
      AppLogger.i('窗口关闭:已确认');
      await windowManager.setPreventClose(false);
      await windowManager.destroy();
    } catch (e) {
      AppLogger.e('窗口关闭处理失败', e);
    } finally {
      _closeDialogOpen = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final settings = ref.watch(settingsProvider);

    return MaterialApp(
      title: 'app.nameFull'.tr(),
      debugShowCheckedModeBanner: false,
      navigatorKey: _rootNavKey,
      theme: AppTheme.lightTheme,
      darkTheme: AppTheme.darkTheme,
      themeMode: settings.themeMode,
      localizationsDelegates: context.localizationDelegates,
      supportedLocales: context.supportedLocales,
      locale: context.locale,
      // The app opens on the workspace home screen (recent projects, new/open)
      // and pushes the editor on top of it once a project is loaded.
      home: const WorkspaceScreen(),
    );
  }
}

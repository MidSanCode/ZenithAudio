import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:window_manager/window_manager.dart';
import 'core/theme/app_theme.dart';
import 'core/utils/logger.dart';
import 'providers/project_provider.dart';
import 'providers/settings_provider.dart';
import 'screens/editor_screen.dart';

class ZenithAudioApp extends ConsumerStatefulWidget {
  const ZenithAudioApp({super.key});

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
  }

  @override
  void dispose() {
    windowManager.removeListener(this);
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
      home: const EditorScreen(),
    );
  }
}

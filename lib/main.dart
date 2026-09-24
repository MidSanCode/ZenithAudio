import 'dart:io' show Platform, exit;
import 'dart:ui' as ui;
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import 'package:media_kit/media_kit.dart';
import 'package:window_manager/window_manager.dart';
import 'core/utils/logger.dart';
import 'services/file_association_service.dart';
import 'services/launcher_args.dart';
import 'services/single_instance.dart';
import 'app.dart';

bool get _isDesktop =>
    !kIsWeb &&
    (Platform.isWindows || Platform.isMacOS || Platform.isLinux);

void main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  MediaKit.ensureInitialized();

  await EasyLocalization.ensureInitialized();

  // Desktop: intercept the OS close button so we can ask to save first.
  if (_isDesktop) {
    await windowManager.ensureInitialized();
    await windowManager.setPreventClose(true);
  }

  // File association: a `.zaproj` opened from the OS arrives as an argument.
  final initialProjectPath = LauncherArgs.initialProjectPath(args);

  // If the app is already running, hand the project over and exit instead of
  // opening a second window.
  if (await SingleInstance.handOffToExisting(initialProjectPath)) {
    exit(0);
  }

  // Make `.zaproj` files open with us (per-user, no admin rights needed).
  if (_isDesktop) {
    await FileAssociationService.ensureRegistered();
  }

  AppLogger.i('卓声 ZENITH AUDIO 启动');

  SystemChrome.setPreferredOrientations([
    DeviceOrientation.landscapeLeft,
    DeviceOrientation.landscapeRight,
  ]);

  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);

  SystemChrome.setSystemUIOverlayStyle(const SystemUiOverlayStyle(
    statusBarColor: Colors.transparent,
    statusBarIconBrightness: Brightness.light,
    systemNavigationBarColor: Colors.transparent,
    systemNavigationBarIconBrightness: Brightness.light,
  ));

  FlutterError.onError = (details) {
    AppLogger.e('Flutter 错误', details.exception, details.stack);
    FlutterError.presentError(details);
  };

  ui.PlatformDispatcher.instance.onError = (error, stack) {
    AppLogger.e('平台错误', error, stack);
    return true;
  };

  runApp(
    EasyLocalization(
      supportedLocales: const [Locale('zh'), Locale('en')],
      path: 'assets/translations',
      fallbackLocale: const Locale('zh'),
      startLocale: const Locale('zh'),
      child: ProviderScope(
        child: ZenithAudioApp(initialProjectPath: initialProjectPath),
      ),
    ),
  );
}

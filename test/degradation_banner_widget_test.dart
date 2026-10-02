import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:zenith_audio/providers/web_degradation_provider.dart';
import 'package:zenith_audio/widgets/degrade/audio_degradation_banner.dart';

/// S1.5: the persistent degradation banner renders and reacts to policy state.
///
/// This is the widget nothing built before: the strategy and the provider had
/// tests, but the banner itself was never pumped. A banner that fails to build
/// would only surface at runtime, so it is pinned here.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() async {
    // EasyLocalization reads a stored locale on init; the test binding has no
    // real plugin, so provide an in-memory store *before* initialising it.
    SharedPreferences.setMockInitialValues({});
    await EasyLocalization.ensureInitialized();
  });

  Future<void> pumpBanner(
    WidgetTester tester,
    ProviderContainer container, {
    VoidCallback? onLowerQuality,
  }) async {
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: EasyLocalization(
          supportedLocales: const [Locale('en'), Locale('zh')],
          path: 'assets/translations',
          fallbackLocale: const Locale('en'),
          child: MaterialApp(
            home: Scaffold(
              body: AudioDegradationBanner(onLowerQuality: onLowerQuality),
            ),
          ),
        ),
      ),
    );
  }

  testWidgets('renders nothing at full health', (tester) async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    await pumpBanner(tester, container);
    expect(find.byIcon(Icons.graphic_eq), findsNothing);
  });

  testWidgets('shows the warning once the policy degrades', (tester) async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    await pumpBanner(tester, container);

    // Drive the policy to L2.
    final notifier = container.read(webDegradationProvider.notifier);
    for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
      notifier.updateFromStatus(cpuLoad: 0.95, xrunCount: 0);
    }
    await tester.pump();

    expect(find.byIcon(Icons.graphic_eq), findsOneWidget);
    // The "still enable" action is offered while not yet forced.
    expect(find.byType(TextButton), findsWidgets);
  });

  testWidgets('the lower-quality action fires the injected callback',
      (tester) async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    var called = false;
    await pumpBanner(tester, container, onLowerQuality: () => called = true);

    final notifier = container.read(webDegradationProvider.notifier);
    for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
      notifier.updateFromStatus(cpuLoad: 0.95, xrunCount: 0);
    }
    await tester.pump();

    await tester.tap(find.byIcon(Icons.tune));
    await tester.pump();
    expect(called, isTrue);
  });

  testWidgets('after enableAll the still-enable button disappears',
      (tester) async {
    final container = ProviderContainer();
    addTearDown(container.dispose);
    await pumpBanner(tester, container);

    final notifier = container.read(webDegradationProvider.notifier);
    for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
      notifier.updateFromStatus(cpuLoad: 0.95, xrunCount: 0);
    }
    notifier.enableAll();
    await tester.pump();

    // The banner stays (the warning is sticky) but the override button is gone.
    expect(find.byIcon(Icons.graphic_eq), findsOneWidget);
    final state = container.read(webDegradationProvider);
    expect(state.userForced, isTrue);
    expect(state.warningActive, isTrue);
  });
}

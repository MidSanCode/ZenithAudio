import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/providers/web_degradation_provider.dart';

/// S1.5: the pure degradation policy. No engine, no timer — each test feeds a
/// sequence of status polls and asserts the resulting tier.
void main() {
  const healthy = 0.3;
  const heavy = 0.95;

  group('healthy operation', () {
    test('stays at full', () {
      final policy = DegradationPolicy();
      for (var i = 0; i < 20; i++) {
        final d = policy.evaluate(cpuLoad: healthy, xrunCount: 0);
        expect(d.level, DegradationLevel.full);
        expect(d.warningActive, isFalse);
      }
    });

    test('a single load spike does not degrade', () {
      final policy = DegradationPolicy();
      policy.evaluate(cpuLoad: heavy, xrunCount: 0);
      // One bad poll is not enough; the next healthy poll resets the counter.
      final d = policy.evaluate(cpuLoad: healthy, xrunCount: 0);
      expect(d.level, DegradationLevel.full);
    });
  });

  group('degradation', () {
    test('sustained load drops to reduced and raises the warning', () {
      final policy = DegradationPolicy();
      DegradationDecision? last;
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        last = policy.evaluate(cpuLoad: 0.75, xrunCount: 0);
      }
      expect(last!.level, DegradationLevel.reduced);
      expect(last.warningActive, isTrue);
    });

    test('severe load drops all the way to minimal', () {
      final policy = DegradationPolicy();
      DegradationDecision? last;
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        last = policy.evaluate(cpuLoad: heavy, xrunCount: 0);
      }
      expect(last!.level, DegradationLevel.minimal);
    });

    test('the warning is sticky and never auto-clears', () {
      final policy = DegradationPolicy();
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        policy.evaluate(cpuLoad: 0.75, xrunCount: 0);
      }
      // A long healthy stretch must NOT clear the warning.
      for (var i = 0; i < 100; i++) {
        final d = policy.evaluate(cpuLoad: healthy, xrunCount: 0);
        expect(d.warningActive, isTrue, reason: 'poll $i');
      }
    });

    test('a new xrun while already reduced pushes on to minimal', () {
      final policy = DegradationPolicy();
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        policy.evaluate(cpuLoad: 0.75, xrunCount: 0);
      }
      expect(policy.level, DegradationLevel.reduced);
      // Now an underrun arrives (counter increments) at a moderate load.
      DegradationDecision? last;
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        last = policy.evaluate(cpuLoad: 0.7, xrunCount: 1);
      }
      expect(last!.level, DegradationLevel.minimal);
    });
  });

  group('recovery', () {
    test('is offered only after a sustained healthy window', () {
      final policy = DegradationPolicy();
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        policy.evaluate(cpuLoad: 0.75, xrunCount: 0);
      }
      // A few healthy polls is not enough.
      for (var i = 0; i < DegradationThresholds.recoveryPolls - 1; i++) {
        policy.evaluate(cpuLoad: healthy, xrunCount: 0);
      }
      expect(policy.recoveryAvailable, isFalse);
      policy.evaluate(cpuLoad: healthy, xrunCount: 0);
      expect(policy.recoveryAvailable, isTrue);
    });

    test('steps back up one tier at a time', () {
      final policy = DegradationPolicy();
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        policy.evaluate(cpuLoad: heavy, xrunCount: 0);
      }
      expect(policy.level, DegradationLevel.minimal);
      expect(policy.recover(), DegradationLevel.reduced);
      expect(policy.recover(), DegradationLevel.full);
      // Full clears the warning, because the user explicitly restored it.
      expect(policy.warningActive, isFalse);
    });
  });

  group('user override', () {
    test('forceFull restores everything but keeps the warning', () {
      final policy = DegradationPolicy();
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        policy.evaluate(cpuLoad: heavy, xrunCount: 0);
      }
      policy.forceFull();
      expect(policy.level, DegradationLevel.full);
      expect(policy.warningActive, isTrue, reason: 'the warning must remain');
      expect(policy.userForced, isTrue);
    });

    test('after an override, further bad polls do not re-degrade', () {
      final policy = DegradationPolicy();
      policy.forceFull();
      for (var i = 0; i < 50; i++) {
        final d = policy.evaluate(cpuLoad: heavy, xrunCount: 5);
        expect(d.level, DegradationLevel.full, reason: 'the user said still enable');
      }
      expect(policy.warningActive, isTrue);
    });
  });

  group('the feature map', () {
    test('L0 disables nothing, L2 disables everything L1 does and more', () {
      final l0 = kDisabledFeaturesByLevel[DegradationLevel.full]!;
      final l1 = kDisabledFeaturesByLevel[DegradationLevel.reduced]!;
      final l2 = kDisabledFeaturesByLevel[DegradationLevel.minimal]!;
      expect(l0, isEmpty);
      expect(l1, isNotEmpty);
      expect(l2.containsAll(l1), isTrue, reason: 'L2 must be a superset of L1');
      expect(l2.length, greaterThan(l1.length));
    });

    test('the state reports disabled features per level', () {
      const reduced = WebDegradationState(level: DegradationLevel.reduced);
      expect(reduced.isDisabled(DegradedFeature.convolutionReverb), isTrue);
      expect(reduced.isDisabled(DegradedFeature.sendBuses), isFalse);

      const minimal = WebDegradationState(level: DegradationLevel.minimal);
      expect(minimal.isDisabled(DegradedFeature.sendBuses), isTrue);
    });
  });

  group('the notifier', () {
    test('mirrors policy decisions into state', () {
      final container = ProviderContainer();
      addTearDown(container.dispose);
      final notifier = container.read(webDegradationProvider.notifier);
      WebDegradationState? state;
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        state = notifier.updateFromStatus(cpuLoad: 0.9, xrunCount: 0);
      }
      expect(state!.level, DegradationLevel.minimal);
      expect(state.warningActive, isTrue);
    });

    test('enableAll keeps the warning and marks the session forced', () {
      final container = ProviderContainer();
      addTearDown(container.dispose);
      final notifier = container.read(webDegradationProvider.notifier);
      for (var i = 0; i < DegradationThresholds.confirmPolls; i++) {
        notifier.updateFromStatus(cpuLoad: 0.9, xrunCount: 0);
      }
      notifier.enableAll();
      final state = container.read(webDegradationProvider);
      expect(state.level, DegradationLevel.full);
      expect(state.warningActive, isTrue);
      expect(state.userForced, isTrue);
    });
  });
}

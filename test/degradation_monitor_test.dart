import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/engine/degradation_monitor.dart';
import 'package:zenith_audio/providers/web_degradation_provider.dart';

/// S1.5: the health monitor that polls an engine and feeds the policy.
void main() {
  test('pollOnce forwards the source sample to the sink', () {
    final received = <EngineHealthSample>[];
    final monitor = DegradationMonitor(
      source: () => const EngineHealthSample(cpuLoad: 0.4, xrunCount: 7),
      interval: const Duration(milliseconds: 10),
      sink: received.add,
    );
    final sample = monitor.pollOnce();
    expect(sample.cpuLoad, 0.4);
    expect(sample.xrunCount, 7);
    expect(received, hasLength(1));
  });

  test('a throwing source is treated as healthy rather than degrading', () {
    final received = <EngineHealthSample>[];
    final monitor = DegradationMonitor(
      source: () => throw StateError('device went away'),
      interval: const Duration(milliseconds: 10),
      sink: received.add,
    );
    final sample = monitor.pollOnce();
    expect(sample.cpuLoad, 0.0);
    expect(sample.xrunCount, 0);
    expect(received.single.cpuLoad, 0.0);
  });

  test('start then stop toggles the timer', () {
    final monitor = DegradationMonitor(
      source: () => EngineHealthSample.healthy,
      interval: const Duration(hours: 1),
      sink: (_) {},
    );
    expect(monitor.isRunning, isFalse);
    monitor.start();
    expect(monitor.isRunning, isTrue);
    monitor.stop();
    expect(monitor.isRunning, isFalse);
    // Idempotent.
    monitor.stop();
    expect(monitor.isRunning, isFalse);
  });

  test('a sustained unhealthy sequence drives the policy to degrade', () {
    // A source that reports overload three times, then healthy.
    var poll = 0;
    final policy = DegradationPolicy();
    final monitor = DegradationMonitor(
      source: () {
        poll++;
        return poll <= 3
            ? const EngineHealthSample(cpuLoad: 0.95, xrunCount: 0)
            : EngineHealthSample.healthy;
      },
      interval: const Duration(milliseconds: 1),
      sink: (sample) => policy.evaluate(
        cpuLoad: sample.cpuLoad,
        xrunCount: sample.xrunCount,
      ),
    );
    for (var i = 0; i < 5; i++) {
      monitor.pollOnce();
    }
    expect(policy.level, DegradationLevel.minimal);
    expect(policy.warningActive, isTrue);
  });
}

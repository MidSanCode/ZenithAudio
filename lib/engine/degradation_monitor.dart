import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../providers/web_degradation_provider.dart';
/// One sample of the engine's health, as the monitor needs it.
///
/// Deliberately smaller than the full `NativeEngineStatus`: the degradation
/// policy only needs the load and the xrun counter, and keeping the interface
/// this narrow is what lets the monitor be tested with a three-line fake.
class EngineHealthSample {
  /// Real-time load, `0..1` (DSP time / available time).
  final double cpuLoad;

  /// Buffer underruns so far. A monotonic counter; the policy compares two
  /// consecutive samples to detect a *new* underrun.
  final int xrunCount;

  const EngineHealthSample({required this.cpuLoad, required this.xrunCount});

  /// A perfectly healthy sample.
  static const EngineHealthSample healthy =
      EngineHealthSample(cpuLoad: 0.0, xrunCount: 0);
}

/// Polls an engine's health and feeds it into the degradation policy.
///
/// ## Why polling, not a callback
///
/// ABI §5.3 forbids the audio thread calling back into Dart: a `NativeCallable`
/// from the audio thread would introduce GC and scheduling jitter into the very
/// path that must stay deterministic. So the engine writes a lock-free snapshot
/// and this monitor polls it.
///
/// ## Cadence
///
/// The plan specifies ~1 Hz on web (the cost of `postMessage` there) and a
/// faster rate on desktop. The interval is injected so both are the same code.
///
/// ## Testability
///
/// The source is injected and [pollOnce] is separable from the timer, so a test
/// can drive a sequence of samples deterministically without a real engine or a
/// real clock.
class DegradationMonitor {
  /// Reads one health sample from the engine.
  final EngineHealthSample Function() source;

  /// How often to poll.
  final Duration interval;

  /// Consumes each sample. Defaults to the app's degradation provider.
  final void Function(EngineHealthSample) sink;

  /// Whether the engine is running in a browser (affects nothing today but is
  /// passed through so a future web-specific threshold has a hook).
  final bool isWeb;

  Timer? _timer;

  DegradationMonitor({
    required this.source,
    required this.interval,
    required this.sink,
    this.isWeb = false,
  });

  /// Whether the monitor is running.
  bool get isRunning => _timer != null;

  /// Starts polling.
  void start() {
    _timer ??= Timer.periodic(interval, (_) => pollOnce());
  }

  /// Stops polling. Idempotent.
  void stop() {
    _timer?.cancel();
    _timer = null;
  }

  /// Reads one sample and hands it to the sink. Never throws: a failing source
  /// (a device that went away) is treated as a healthy sample so a transient
  /// read error cannot silently degrade the session.
  EngineHealthSample pollOnce() {
    EngineHealthSample sample;
    try {
      sample = source();
    } catch (_) {
      sample = EngineHealthSample.healthy;
    }
    sink(sample);
    return sample;
  }
}

/// The engine-health source used by the app.
///
/// Overridden with a real reader where an engine exists. The default is a
/// healthy stub, so the monitor is inert until an engine is wired rather than
/// degrading the session from a missing source.
final engineHealthSourceProvider =
    Provider<EngineHealthSample Function()>((ref) => () => EngineHealthSample.healthy);

/// A monitor bound to [engineHealthSourceProvider], created on demand.
///
/// The owner calls `start()` when playback begins and `stop()` on dispose; the
/// provider disposes it if the container does.
final degradationMonitorProvider = Provider<DegradationMonitor>((ref) {
  final monitor = DegradationMonitor(
    source: ref.watch(engineHealthSourceProvider),
    interval: const Duration(seconds: 1),
    sink: (sample) => ref.read(webDegradationProvider.notifier).updateFromStatus(
          cpuLoad: sample.cpuLoad,
          xrunCount: sample.xrunCount,
        ),
  );
  ref.onDispose(monitor.stop);
  return monitor;
});

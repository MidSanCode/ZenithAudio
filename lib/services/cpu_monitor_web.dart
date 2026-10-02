import 'package:flutter_riverpod/flutter_riverpod.dart';

/// Web stub for the CPU monitor.
///
/// The browser never reports system CPU usage through a portable API, and the
/// desktop implementation cannot even compile here (it imports `dart:ffi` and
/// `dart:io`). This stub keeps the same public surface so a web build compiles
/// and the toolbar simply reads `0` / unsupported.
///
/// The real degradation signal on the web comes from the engine's own
/// real-time load and xrun counters (see `web_degradation_provider.dart`), not
/// from this monitor — so a zero here is honest, not a missing feature.

/// Always `0` on the web.
final cpuUsageProvider = StateProvider<double>((ref) => 0);

/// A monitor that supports nothing.
final cpuMonitorProvider = Provider<CpuMonitor>((ref) {
  final monitor = CpuMonitor();
  ref.onDispose(monitor.stop);
  return monitor;
});

/// A no-op CPU monitor for the web.
class CpuMonitor {
  /// Always `false`.
  bool get supported => false;

  /// Always `0`.
  double get usage => 0;

  /// Receives no updates.
  void Function(double usage)? onUsage;

  /// No-op: there is no system CPU reading to poll.
  void start() {}

  /// No-op.
  void stop() {}
}

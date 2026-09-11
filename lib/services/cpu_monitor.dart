import 'dart:async';
import 'dart:ffi' as ffi;
import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

/// Live system CPU usage (0.0 – 1.0). Windows-only via kernel32
/// GetSystemTimes; stays 0 and reports unsupported elsewhere.
final cpuUsageProvider = StateProvider<double>((ref) => 0);

final cpuMonitorProvider = Provider<CpuMonitor>((ref) {
  final monitor = CpuMonitor();
  monitor.onUsage = (u) => ref.read(cpuUsageProvider.notifier).state = u;
  ref.onDispose(monitor.stop);
  return monitor;
});

typedef _GetSystemTimesC = ffi.Int32 Function(
    ffi.Pointer<ffi.Uint32> idle,
    ffi.Pointer<ffi.Uint32> kernel,
    ffi.Pointer<ffi.Uint32> user);
typedef _GetSystemTimesD = int Function(
    ffi.Pointer<ffi.Uint32> idle,
    ffi.Pointer<ffi.Uint32> kernel,
    ffi.Pointer<ffi.Uint32> user);

typedef _VirtualAllocC = ffi.Pointer<ffi.Uint32> Function(
    ffi.Pointer<ffi.Uint32> addr,
    ffi.Uint64 size,
    ffi.Uint32 type,
    ffi.Uint32 protect);
typedef _VirtualAllocD = ffi.Pointer<ffi.Uint32> Function(
    ffi.Pointer<ffi.Uint32>, ffi.Uint64, ffi.Uint32, ffi.Uint32);

class CpuMonitor {
  static const int _memCommitReserve = 0x3000; // MEM_COMMIT | MEM_RESERVE
  static const int _pageReadWrite = 0x04;

  Timer? _timer;
  bool _ok = false;
  bool get supported => _ok;

  late _GetSystemTimesD _getSystemTimes;
  // 3 FILETIMEs (idle, kernel, user) = 6 x Uint32 = 24 bytes.
  ffi.Pointer<ffi.Uint32>? _mem;
  int _prevTotal = 0;
  int _prevIdle = 0;

  void start() {
    if (!Platform.isWindows || _timer != null) return;
    try {
      final k32 = ffi.DynamicLibrary.open('kernel32.dll');
      final virtualAlloc =
          k32.lookupFunction<_VirtualAllocC, _VirtualAllocD>('VirtualAlloc');
      _getSystemTimes = k32
          .lookupFunction<_GetSystemTimesC, _GetSystemTimesD>('GetSystemTimes');
      final mem = virtualAlloc(
          ffi.nullptr, 24, _memCommitReserve, _pageReadWrite);
      if (mem == ffi.nullptr) return;
      _mem = mem;
      _ok = true;
      _tick(); // prime the baseline
      _timer = Timer.periodic(const Duration(seconds: 1), (_) => _tick());
    } catch (_) {
      _ok = false;
    }
  }

  int _read64(int dwordOffset) {
    final m = _mem!;
    final lo = m[dwordOffset];
    final hi = m[dwordOffset + 1];
    return lo | (hi << 32);
  }

  void _tick() {
    if (!_ok) return;
    final m = _mem!;
    // GetSystemTimes(LPFILETIME idle, LPFILETIME kernel, LPFILETIME user)
    final ok = _getSystemTimes(
        m.elementAt(0), m.elementAt(2), m.elementAt(4));
    if (ok == 0) return;

    final idle = _read64(0);
    // On Windows, kernel time includes idle time.
    final total = _read64(2) + _read64(4);

    final isFirst = _prevTotal == 0;
    final dTotal = total - _prevTotal;
    final dIdle = idle - _prevIdle;
    _prevTotal = total;
    _prevIdle = idle;

    // First sample only primes the baseline.
    if (isFirst || dTotal <= 0) return;
    final usage = 1.0 - (dIdle / dTotal);
    _usage = usage.clamp(0.0, 1.0);
    onUsage?.call(_usage);
  }

  void Function(double usage)? onUsage;

  double _usage = 0;
  double get usage => _usage;

  void stop() {
    _timer?.cancel();
    _timer = null;
  }
}

/// Live system CPU usage, with a platform split so a web build can compile.
///
/// ## Why the split exists
///
/// The desktop implementation reads CPU time through `dart:ffi` + `dart:io`
/// (`kernel32` on Windows). Neither library exists on the web, and an
/// unconditional import of either makes `flutter build web` fail to compile —
/// which would take the whole web target down for a *toolbar readout*. The
/// conditional export below routes the web build to a stub that always reports
/// `0` and `supported == false`.
///
/// ## Why not just guard with `kIsWeb`
///
/// A `kIsWeb` check is a runtime test; the compiler still has to resolve
/// `dart:ffi` and `dart:io` and fails before any code runs. The split has to be
/// at the import level, which is what a conditional export does.
library;

export 'cpu_monitor_io.dart'
    if (dart.library.html) 'cpu_monitor_web.dart';

/// `dart:ffi` bindings for the ZENITH AUDIO Rust core.
///
/// This is the only place in the Dart codebase that knows the C ABI shape.
/// Everything else talks to `engine/engine.dart` or `automation/parameter.dart`,
/// so the ABI can grow without a ripple through the app.
///
/// ## Binding discipline
///
/// * Signatures here must match `docs/ABI.md` and `native/zenith_core/src/lib.rs`
///   exactly. A mismatch is a memory-safety bug, not a compile error.
/// * Every lookup is lazy: importing this file must never load a library, so a
///   test run or a web build that never touches the engine pays nothing.
/// * Loading failure is reported as [isAvailable] returning `false` rather than
///   throwing from a getter — callers decide whether the engine is required.
library;

import 'dart:ffi';
import 'dart:io';

// `Pointer<Utf8>.toDartString()` lives in package:ffi's extension.
import 'package:ffi/ffi.dart';

/// ABI version this Dart code was written against, in the same
/// `major<<16 | minor<<8 | patch` layout the Rust side uses.
///
/// `0.4.0`. The minor counter is a **single linear sequence shared by every
/// agent** adding to the ABI, not a per-stage number. S2 (parameter and
/// automation) claimed `0.2.0`; S3 (mixer) then claimed `0.3.0`; S5 (the
/// built-in effect suite's query surface) claimed `0.4.0` by bumping the same
/// `ABI_VERSION` constant. This must therefore track whatever the core
/// currently reports, not "the S2 number" — the runtime check is an exact
/// comparison, so a stale constant fails loudly by design (ABI §2.3).
///
/// Registered as C-002 / C-009 / C-011 in `docs/COORDINATION.md`.
const int kExpectedAbiVersion = 0x000400; // 0.4.0

// ── C signatures ──

typedef _VersionNative = Uint32 Function();
typedef _VersionDart = int Function();

typedef _VersionMatchNative = Uint32 Function(Uint32);
typedef _VersionMatchDart = int Function(int);

typedef _VersionStringNative = Pointer<Utf8> Function();
typedef _VersionStringDart = Pointer<Utf8> Function();

/// Access to the native core.
///
/// Stateless by design: S0 only proves the link works, and a stateful wrapper
/// would have to be rewritten when the real engine lands in S1.
abstract final class ZenithCore {
  static DynamicLibrary? _library;
  static bool _loadAttempted = false;

  /// The ABI version these bindings were compiled against.
  static int get expectedAbiVersion => kExpectedAbiVersion;

  /// Loads the native library, caching the attempt.
  ///
  /// Returns `null` when the library cannot be found, which is the normal state
  /// before a `cargo build` or a full `flutter build` has run.
  static DynamicLibrary? _load() {
    if (_loadAttempted) return _library;
    _loadAttempted = true;

    // On Windows the hook registers the DLL next to the executable; on Linux
    // and macOS it is loaded from the bundle's library directory. Resolving by
    // name is what the native-assets mechanism expects.
    for (final name in _candidateNames()) {
      try {
        _library = DynamicLibrary.open(name);
        return _library;
      } on ArgumentError {
        continue;
      } on Object {
        continue;
      }
    }
    return null;
  }

  /// Library file names to try, in order.
  static List<String> _candidateNames() {
    if (Platform.isWindows) {
      return ['zenith_core.dll'];
    }
    if (Platform.isMacOS) {
      return ['libzenith_core.dylib', 'zenith_core.dylib'];
    }
    return ['libzenith_core.so', 'zenith_core.so'];
  }

  /// Whether the native core is loadable in this process.
  ///
  /// Never throws — a missing or broken library is reported as `false`.
  static bool isAvailable() {
    try {
      return _load() != null;
    } on Object {
      return false;
    }
  }

  /// The loaded library, or `null` when it is unavailable.
  ///
  /// Exposed for the parameter and automation bindings, which resolve their own
  /// (much larger) symbol set and need the library handle to do it. Prefer
  /// [isAvailable] for a yes/no question; this exists so those bindings do not
  /// have to duplicate the library-loading and candidate-name logic.
  static DynamicLibrary? libraryOrNull() => _load();

  static _VersionDart? _version;
  static _VersionMatchDart? _versionMatch;
  static _VersionStringDart? _versionString;

  /// The loaded library's ABI version.
  ///
  /// Throws [StateError] when the core is not available; callers that cannot
  /// guarantee availability must check [isAvailable] first.
  static int version() {
    final lib = _load();
    if (lib == null) {
      throw StateError(
        'zenith_core is not available — build the Rust core first '
        '(`cargo build` or `flutter build windows`).',
      );
    }
    _version ??= lib.lookupFunction<_VersionNative, _VersionDart>('zenith_version');
    return _version!();
  }

  /// Whether the loaded library's ABI matches [expected].
  static bool versionMatches(int expected) {
    final lib = _load();
    if (lib == null) {
      throw StateError('zenith_core is not available.');
    }
    _versionMatch ??= lib
        .lookupFunction<_VersionMatchNative, _VersionMatchDart>('zenith_version_match');
    return _versionMatch!(expected) == 1;
  }

  /// The loaded library's human-readable version string.
  static String versionString() {
    final lib = _load();
    if (lib == null) {
      throw StateError('zenith_core is not available.');
    }
    _versionString ??= lib
        .lookupFunction<_VersionStringNative, _VersionStringDart>('zenith_version_string');
    final pointer = _versionString!();
    if (pointer == nullptr) return '';
    return pointer.toDartString();
  }

  /// Fails loudly when the linked library does not match these bindings.
  ///
  /// Called once at engine startup so an ABI mismatch surfaces as a clear
  /// message instead of corrupted audio later.
  static void assertCompatible() {
    if (!isAvailable()) {
      throw StateError(
        'zenith_core is not available — build the Rust core first.',
      );
    }
    final actual = version();
    if (actual != kExpectedAbiVersion) {
      throw StateError(
        'zenith_core ABI mismatch: Dart expects $kExpectedAbiVersion '
        '(0.4.0), the library reports $actual. Rebuild the Rust core.',
      );
    }
  }
}

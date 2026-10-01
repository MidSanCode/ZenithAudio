import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/native/zenith_core.dart';

/// S0: the FFI link probe.
///
/// The Rust core is only useful if these tests run — they exercise the C ABI
/// through the same generated bindings the app uses, so a broken `hook/build.dart`
/// or a missing `cargo build` fails here rather than at first playback.
///
/// When the native library is absent (a plain `flutter test` with no build step
/// run), the suite skips loudly instead of failing: the ABI is still unproven,
/// but the Dart side must not be blocked by a missing binary in development.
void main() {
  final available = ZenithCore.isAvailable();

  group('zenith_core FFI', () {
    test('the native library is loadable', () {
      if (!available) {
        markTestSkipped(
          'zenith_core native library not built — run `cargo build` or '
          '`flutter build windows --debug` to exercise this test.',
        );
        return;
      }
      expect(ZenithCore.isAvailable(), isTrue);
    });

    test('zenith_version returns a non-zero ABI stamp', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      final version = ZenithCore.version();
      expect(version, isNot(0));
      // Derived from the single constant rather than hard-coded: the ABI minor
      // counter is shared by every stage adding to it, so a literal here goes
      // stale on someone else's commit and fails a test that is not about them.
      // The *value* is pinned by `lib.rs`'s own test; this asserts only that
      // the Dart constant and the loaded library agree.
      expect(version, ZenithCore.expectedAbiVersion);
    });

    test('the Dart and Rust ABI versions agree', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      // A mismatch means the Dart bindings and the linked library were built
      // from incompatible sources, which is a silent-corruption risk.
      expect(
        ZenithCore.versionMatches(ZenithCore.expectedAbiVersion),
        isTrue,
        reason: 'Dart expects ABI ${ZenithCore.expectedAbiVersion}, '
            'library reports ${ZenithCore.version()}',
      );
    });

    test('an unknown ABI version is rejected', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      expect(ZenithCore.versionMatches(0), isFalse);
      expect(ZenithCore.versionMatches(0xFFFFFF), isFalse);
    });

    test('the version string is readable and NUL-terminated', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      // Compared against the constant for the same reason as above, plus a
      // shape check: the string must render the stamp's major/minor/patch so a
      // mismatch report is actually readable.
      final stamp = ZenithCore.expectedAbiVersion;
      final expected = '${stamp >> 16}.${(stamp >> 8) & 0xFF}.${stamp & 0xFF}';
      expect(ZenithCore.versionString(), expected);
    });
  });
}

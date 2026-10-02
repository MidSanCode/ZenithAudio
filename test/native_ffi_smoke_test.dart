import 'dart:ffi';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/engine/ffi/edit_bindings.dart';
import 'package:zenith_audio/engine/ffi/engine_bindings.dart';
import 'package:zenith_audio/engine/ffi/engine_types.dart';
import 'package:zenith_audio/native/zenith_core.dart';
import 'package:zenith_audio/services/wav_encoder.dart';

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

    test('[S1] the engine struct mirrors match the core', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      final sizes = EngineStructSizes.read();
      expect(sizes, isNotNull, reason: 'the S1 sizeof helpers must resolve');
      // Dart's idea of each layout against the core's, so a field reorder or a
      // width change surfaces as a loud failure rather than silent misreads.
      expect(sizes!.engineConfig, sizeOf<ZenithEngineConfig>());
      expect(sizes.engineStatus, sizeOf<ZenithEngineStatus>());
      expect(sizes.musicalTime, sizeOf<ZenithMusicalTime>());
    });

    test('[S1] an engine can be created, driven and destroyed', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      final engine = ZenithEngineHandle.create(sampleRate: 48000, blockSize: 256);
      expect(engine, isNotNull);
      addTearDown(engine!.dispose);

      expect(engine.play(), isTrue);
      // One beat at 120 BPM is 960 ticks = 24000 frames at 48 kHz.
      expect(engine.seekTicks(960), isTrue);
      final status = engine.readStatus();
      expect(status.playheadFrames, 24000);
      expect(status.sampleRate, 48000);
      expect(status.isPlaying, isTrue);

      final out = Float32List.fromList(List<double>.filled(256 * 2, 0));
      expect(engine.render(out, 256), isTrue);
    });

    test('[S1] the offline driver is reported as supported', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }
      expect(engineDriverSupported(ZenithDriverKind.offline), isTrue);
    });

    test('[S4] offline rendering returns a WAV-encodable buffer', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      final engine = ZenithEngineHandle.create(sampleRate: 48000, blockSize: 256);
      expect(engine, isNotNull);
      addTearDown(engine!.dispose);

      // Render one beat (960 ticks) at 120 BPM = 24000 frames.
      final samples = engine.renderOffline(startTicks: 0, endTicks: 960);
      expect(samples, isNotNull);
      expect(samples!.length, 24000 * 2, reason: 'interleaved stereo');

      final wav = encodeWav(samples, channels: 2, sampleRate: 48000);
      final header = probeWavHeader(wav);
      expect(header, isNotNull);
      expect(header!.channels, 2);
      expect(header.sampleRate, 48000);
      expect(header.dataBytes, 24000 * 2 * 2);

      expect(engine.pdcLatency(), 0, reason: 'no effects => no PDC delay');
    });

    test('[S8] audio-edit bindings run through the FFI', () {
      if (!available) {
        markTestSkipped('native library not built');
        return;
      }

      final input = Float32List.fromList(
        List<double>.generate(4000, (i) => (i % 100 < 50) ? 0.5 : -0.5),
      );

      final stretched = AudioEditBindings.timeStretch(input, 2.0);
      expect(stretched, isNotNull);
      expect((stretched!.length - 8000).abs() <= 1, isTrue);

      final shifted = AudioEditBindings.pitchShift(input, 12.0);
      expect(shifted, isNotNull);
      expect((shifted!.length / input.length - 1.0).abs() < 0.05, isTrue);

      final transients = AudioEditBindings.detectTransients(input);
      expect(transients, isA<List<int>>());

      final cross = AudioEditBindings.crossfade(
        Float32List.fromList(List.filled(100, 1.0)),
        Float32List.fromList(List.filled(80, 2.0)),
        fade: 40,
      );
      expect(cross, isNotNull);
      expect(cross!.length, 140);
    });
  });
}

import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/core/constants/app_constants.dart';
import 'package:zenith_audio/models/project.dart';
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/launcher_args.dart';
import 'package:zenith_audio/services/lgdf_format.dart';
import 'package:zenith_audio/services/project_serializer_io.dart';
import 'package:zenith_audio/services/workspace_service.dart';

/// Covers the `.zaproj` container rename (contents stay LGDF) and the OS file
/// association plumbing.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  /// `deserialize` extracts to the temp directory, so satisfy path_provider's
  /// channel with a real directory instead of leaving it unimplemented.
  late Directory tempForPlatform;

  setUpAll(() async {
    tempForPlatform = await Directory.systemTemp.createTemp('zaproj_tmp_');
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
      const MethodChannel('plugins.flutter.io/path_provider'),
      (call) async {
        if (call.method == 'getTemporaryDirectory' ||
            call.method == 'getApplicationSupportDirectory') {
          return tempForPlatform.path;
        }
        return null;
      },
    );
  });

  tearDownAll(() async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
      const MethodChannel('plugins.flutter.io/path_provider'),
      null,
    );
    if (await tempForPlatform.exists()) {
      await tempForPlatform.delete(recursive: true);
    }
  });
  group('archive extension', () {
    test('the container extension is .zaproj', () {
      expect(Lgdf.extension, '.zaproj');
      expect(AppConstants.lgdfExtension, '.zaproj');
    });

    test('the format identifier is still lgdf', () {
      // Renaming the container must not change what is inside it.
      expect(Lgdf.format, 'lgdf');
    });

    test('the picker accepts the current and legacy extensions', () {
      expect(AppConstants.projectOpenExtensions, contains('zaproj'));
      expect(AppConstants.projectOpenExtensions, contains('lgdf'));
      expect(AppConstants.projectOpenExtensions, contains('zap'));
    });

    test('legacy archives are recognised for reading', () {
      expect(Lgdf.knownArchiveExtensions, containsAll(['.zaproj', '.lgdf', '.zap']));
    });

    test('stripArchiveExtension removes any known container extension', () {
      expect(Lgdf.stripArchiveExtension('song.zaproj'), 'song');
      expect(Lgdf.stripArchiveExtension('song.lgdf'), 'song');
      expect(Lgdf.stripArchiveExtension('song.zap'), 'song');
      expect(Lgdf.stripArchiveExtension('song.wav'), 'song.wav');
      expect(Lgdf.stripArchiveExtension('no-extension'), 'no-extension');
      // Case-insensitive, and keeps the original casing of the stem.
      expect(Lgdf.stripArchiveExtension('My Song.ZAPROJ'), 'My Song');
    });
  });

  group('short project ids', () {
    test('shortId never throws on ids shorter than its prefix length', () {
      expect(Lgdf.shortId('p1'), 'p1');
      expect(Lgdf.shortId(''), 'new');
      expect(Lgdf.shortId('abcdefgh'), 'abcdefgh');
      expect(Lgdf.shortId('abcdefghij'), 'abcdefgh');
    });

    test('projectSlug falls back safely for a short id', () {
      expect(Lgdf.projectSlug('', 'p1'), 'project-p1');
      expect(Lgdf.projectSlug('My Song', 'p1'), 'my-song');
      // Non-ASCII names slug to nothing, so the fallback must carry the name.
      expect(Lgdf.projectSlug('中文名', 'p1'), 'project-p1');
    });
  });

  group('workspace labels', () {
    test('archive entries show without their extension', () {
      final entry = WorkspaceProjectFile(
        path: '/ws/song.zaproj',
        name: 'song.zaproj',
        modified: DateTime(2026, 1, 1),
        size: 10,
        isDirectory: false,
      );
      expect(entry.label, 'song');
    });

    test('a legacy .zap archive also reads clean', () {
      final entry = WorkspaceProjectFile(
        path: '/ws/old.zap',
        name: 'old.zap',
        modified: DateTime(2026, 1, 1),
        size: 10,
        isDirectory: false,
      );
      expect(entry.label, 'old');
    });

    test('directory projects keep their folder name', () {
      final entry = WorkspaceProjectFile(
        path: '/ws/my-song',
        name: 'my-song',
        modified: DateTime(2026, 1, 1),
        size: 10,
      );
      expect(entry.label, 'my-song');
    });

    test('the LGDF display name wins when present', () {
      final entry = WorkspaceProjectFile(
        path: '/ws/my-song',
        name: 'my-song',
        displayName: 'My Song',
        modified: DateTime(2026, 1, 1),
        size: 10,
      );
      expect(entry.label, 'My Song');
    });
  });

  group('launcher arguments', () {
    late Directory tempRoot;

    setUp(() async {
      tempRoot = await Directory.systemTemp.createTemp('zaproj_args_');
    });

    tearDown(() async {
      if (await tempRoot.exists()) await tempRoot.delete(recursive: true);
    });

    test('returns null when launched without arguments', () {
      expect(LauncherArgs.initialProjectPath(const []), isNull);
    });

    test('ignores Flutter switches and unrelated arguments', () {
      expect(
        LauncherArgs.initialProjectPath(const ['--dart-entrypoint-args', 'stuff']),
        isNull,
      );
    });

    test('finds an existing .zaproj file', () async {
      final file = File('${tempRoot.path}/song.zaproj');
      await file.writeAsString('x');
      expect(LauncherArgs.initialProjectPath([file.path]), file.path);
    });

    test('ignores a project path that does not exist on disk', () {
      expect(
        LauncherArgs.initialProjectPath(['${tempRoot.path}/ghost.zaproj']),
        isNull,
      );
    });

    test('also accepts legacy archive arguments', () async {
      final file = File('${tempRoot.path}/old.zap');
      await file.writeAsString('x');
      expect(LauncherArgs.initialProjectPath([file.path]), file.path);
    });

    test('ignores non-project files', () async {
      final file = File('${tempRoot.path}/audio.wav');
      await file.writeAsString('x');
      expect(LauncherArgs.initialProjectPath([file.path]), isNull);
    });
  });

  group('.zaproj round trip', () {
    test('a project exported as .zaproj still reads back', () async {
      final tempRoot = await Directory.systemTemp.createTemp('zaproj_rt_');
      addTearDown(() async {
        if (await tempRoot.exists()) await tempRoot.delete(recursive: true);
      });

      final projectDir = Directory('${tempRoot.path}/song');
      final project = const Project(
        id: 'p1',
        name: 'Round Trip',
        bpm: 140,
        tracks: [
          Track(id: 't1', name: 'Lead', type: TrackType.synth, instrumentName: 'saw'),
        ],
      );
      final serializer = const ProjectSerializer();
      await serializer.writeProjectDirectory(project, projectDir);

      // Write the packed archive out under the new container name.
      final bytes = await serializer.packProjectDirectory(projectDir);
      final archive = File('${tempRoot.path}/song${Lgdf.extension}');
      await archive.writeAsBytes(bytes);
      expect(archive.path, endsWith('.zaproj'));

      // Reading it back ignores the container extension and reads the LGDF
      // contents inside.
      final read = await serializer.deserialize(await archive.readAsBytes());
      expect(read, isNotNull);
      expect(read!.project.name, 'Round Trip');
      expect(read.project.bpm, 140);
      expect(read.project.tracks.single.instrumentName, 'saw');
      // Contents are LGDF regardless of the outer extension: info.json's name
      // is the project slug, not the folder name.
      expect(read.lgdfInfo?.name, 'round-trip');
      expect(read.lgdfInfo?.displayName, 'Round Trip');
    });
  });
}

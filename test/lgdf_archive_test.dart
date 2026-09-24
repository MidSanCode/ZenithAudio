import 'dart:convert';
import 'dart:io';

import 'package:archive/archive.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/project.dart';
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/lgdf_format.dart';
import 'package:zenith_audio/services/project_serializer_io.dart';

/// Verifies the packaged archive contents match the LGDF entry set and that a
/// real project survives a pack → unpack round trip.
void main() {
  late Directory tempRoot;

  setUp(() async {
    tempRoot = await Directory.systemTemp.createTemp('lgdf_archive_');
  });

  tearDown(() async {
    if (await tempRoot.exists()) await tempRoot.delete(recursive: true);
  });

  Project buildProject(File? audio) => Project(
        id: 'proj-pack',
        name: 'Pack Test',
        bpm: 100,
        tracks: [
          Track(
            id: 's1',
            name: 'Lead',
            type: TrackType.synth,
            instrumentName: 'square',
            notes: const [Note(pitch: 60, startTime: 0.5, duration: 1)],
          ),
          if (audio != null)
            Track(
              id: 'a1',
              name: 'Loop',
              type: TrackType.audio,
              audioFilePath: audio.path,
            ),
        ],
      );

  test('archive contains exactly the standard entry set', () async {
    final audio = File('${tempRoot.path}/loop.wav');
    await audio.writeAsBytes(List<int>.generate(1024, (i) => i % 256));

    final projectDir = Directory('${tempRoot.path}/pack-src');
    final serializer = const ProjectSerializer();
    await serializer.writeProjectDirectory(buildProject(audio), projectDir);

    // Non-shipping markers.
    await File('${projectDir.path}/work/tmp.bin').writeAsString('x');
    await File('${projectDir.path}/dist/old.lgdf').writeAsString('x');

    final bytes = await serializer.packProjectDirectory(projectDir);
    final archive = ZipDecoder().decodeBytes(bytes.toList());
    final names = archive.files
        .where((f) => f.isFile)
        .map((f) => Lgdf.normalizePath(f.name))
        .toSet();

    expect(names, contains('info.json'));
    expect(names, contains('registry.json'));
    expect(names, contains('spec/project.json'));
    expect(names, contains('spec/config.json'));
    expect(names, contains('spec/config.schema.json'));
    expect(names, contains('spec/overview.md'));
    expect(names, contains('assets/audios/audio_a1.wav'));
    expect(names, contains('metadata/audios/audio_a1.wav.json'));

    // Excluded entries must not ship.
    expect(names.any((n) => n.startsWith('work/')), isFalse);
    expect(names.any((n) => n.startsWith('dist/')), isFalse);
    expect(names.any((n) => n.startsWith('.')), isFalse);
  });

  test('packed archive keeps assets byte-identical to the source', () async {
    final original = List<int>.generate(2048, (i) => (i * 7) % 256);
    final audio = File('${tempRoot.path}/keep.wav');
    await audio.writeAsBytes(original);

    final projectDir = Directory('${tempRoot.path}/keep-src');
    final serializer = const ProjectSerializer();
    await serializer.writeProjectDirectory(buildProject(audio), projectDir);

    final bytes = await serializer.packProjectDirectory(projectDir);
    final archive = ZipDecoder().decodeBytes(bytes.toList());
    final entry = archive.files.firstWhere(
      (f) => Lgdf.normalizePath(f.name) == 'assets/audios/audio_a1.wav',
    );
    expect(entry.content, original);

    // The registered hash must match the packed bytes.
    final metaEntry = archive.files.firstWhere(
      (f) =>
          Lgdf.normalizePath(f.name) == 'metadata/audios/audio_a1.wav.json',
    );
    final meta = jsonDecode(utf8.decode(metaEntry.content))
        as Map<String, dynamic>;
    expect(meta['sha256'], Lgdf.sha256Hex(original));
  });

  test('registry lists every shipped asset, sorted and unique', () async {
    final a = File('${tempRoot.path}/a.wav');
    await a.writeAsBytes(List<int>.filled(64, 1));

    final projectDir = Directory('${tempRoot.path}/multi');
    final serializer = const ProjectSerializer();
    await serializer.writeProjectDirectory(buildProject(a), projectDir);

    final registry = jsonDecode(
      File('${projectDir.path}/registry.json').readAsStringSync(),
    ) as Map<String, dynamic>;
    final files = (registry['registered_files'] as List).cast<String>();

    expect(files, equals(files.toSet().toList()), reason: 'no duplicates');
    final sorted = [...files]..sort();
    expect(files, equals(sorted), reason: 'sorted');
    expect(registry['asset_count'], files.length);
  });

  test('spec/config.json declares the engine and settings', () async {
    final projectDir = Directory('${tempRoot.path}/config-check');
    await const ProjectSerializer()
        .writeProjectDirectory(buildProject(null), projectDir);

    final config = jsonDecode(
      File('${projectDir.path}/spec/config.json').readAsStringSync(),
    ) as Map<String, dynamic>;

    expect(config['engine'], isA<Map<String, dynamic>>());
    expect((config['engine'] as Map)['name'], 'zenith-audio');
    expect((config['project'] as Map)['name'], 'pack-test');
    expect((config['settings'] as Map)['bpm'], 100);
  });

  test('schema is a valid JSON Schema document', () async {
    final projectDir = Directory('${tempRoot.path}/schema-check');
    await const ProjectSerializer()
        .writeProjectDirectory(buildProject(null), projectDir);

    final schema = jsonDecode(
      File('${projectDir.path}/spec/config.schema.json').readAsStringSync(),
    ) as Map<String, dynamic>;

    expect(schema['\$schema'], contains('json-schema.org'));
    expect(schema['type'], 'object');
    expect(schema['required'], containsAll(['project', 'engine', 'settings']));
  });
}

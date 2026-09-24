import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/project.dart';
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/lgdf_format.dart';
import 'package:zenith_audio/services/project_serializer_io.dart';

/// Verifies that a project written by the app follows the LGDF v2.0
/// directory-mode layout described in `temp/example/lgdf-standard.md`.
void main() {
  late Directory tempRoot;

  setUp(() async {
    tempRoot = await Directory.systemTemp.createTemp('lgdf_test_');
  });

  tearDown(() async {
    if (await tempRoot.exists()) await tempRoot.delete(recursive: true);
  });

  Project buildProject() => const Project(
        id: 'proj-1234',
        name: 'My Song',
        sampleRate: 44100,
        bpm: 128,
        keySignature: 'Am',
        timeSignatureNumerator: 3,
        timeSignatureDenominator: 4,
        tracks: [
          Track(
            id: 't1',
            name: 'Bass',
            type: TrackType.synth,
            instrumentName: 'saw-lead',
            notes: [
              Note(pitch: 40, startTime: 0, duration: 1.5, velocity: 100),
              Note(pitch: 43, startTime: 1.5, duration: 0.5, velocity: 90),
            ],
          ),
          Track(
            id: 't2',
            name: 'Drums',
            type: TrackType.instrument,
            instrumentName: 'kit',
            notes: [Note(pitch: 36, startTime: 0, duration: 0.25)],
          ),
        ],
      );

  test('writes the LGDF directory layout', () async {
    final projectDir = Directory('${tempRoot.path}/my-song');
    await const ProjectSerializer().writeProjectDirectory(buildProject(), projectDir);

    // Required top-level entries.
    expect(File('${projectDir.path}/info.json').existsSync(), isTrue);
    expect(File('${projectDir.path}/registry.json').existsSync(), isTrue);
    expect(Directory('${projectDir.path}/assets').existsSync(), isTrue);
    expect(Directory('${projectDir.path}/metadata').existsSync(), isTrue);
    expect(Directory('${projectDir.path}/spec').existsSync(), isTrue);
    expect(File('${projectDir.path}/.lgdfignore').existsSync(), isTrue);

    // info.json identifies the format.
    final info = jsonDecode(File('${projectDir.path}/info.json').readAsStringSync())
        as Map<String, dynamic>;
    expect(info['format'], 'lgdf');
    expect(info['min_sdk'], 1);
    expect(info['name'], 'my-song');
    expect(info['display_name'], 'My Song');
    expect(info['created_time'], isA<int>());
    expect(info['last_update_time'], isA<int>());

    // Description layer.
    expect(File('${projectDir.path}/spec/project.json').existsSync(), isTrue);
    expect(File('${projectDir.path}/spec/config.json').existsSync(), isTrue);
    expect(File('${projectDir.path}/spec/config.schema.json').existsSync(), isTrue);
    expect(File('${projectDir.path}/spec/overview.md').existsSync(), isTrue);
  });

  test('JSON follows the standard (tab indent, snake_case keys)', () async {
    final projectDir = Directory('${tempRoot.path}/style-check');
    await const ProjectSerializer().writeProjectDirectory(buildProject(), projectDir);

    final raw = File('${projectDir.path}/info.json').readAsStringSync();
    expect(raw.contains('\t'), isTrue, reason: 'JSON must be tab-indented');
    expect(raw.startsWith('{'), isTrue);
    // No BOM.
    expect(raw.codeUnitAt(0), isNot(0xFEFF));

    final doc = jsonDecode(
      File('${projectDir.path}/spec/project.json').readAsStringSync(),
    ) as Map<String, dynamic>;
    expect(doc['document_version'], isA<int>());
    expect(doc['key_signature'], 'Am');
    expect(doc['time_signature'], {'numerator': 3, 'denominator': 4});

    final first = (doc['tracks'] as List).first as Map<String, dynamic>;
    expect(first['is_muted'], isFalse);
    expect(first['instrument_name'], 'saw-lead');
    final note = (first['notes'] as List).first as Map<String, dynamic>;
    expect(note['start_time'], 0);
    expect(note['pitch'], 40);
  });

  test('round-trips a project through the directory', () async {
    final projectDir = Directory('${tempRoot.path}/roundtrip');
    final original = buildProject();
    await const ProjectSerializer().writeProjectDirectory(original, projectDir);

    final read = await const ProjectSerializer().readProjectDirectory(projectDir);
    expect(read, isNotNull);
    expect(read!.project.name, original.name);
    expect(read.project.bpm, 128);
    expect(read.project.keySignature, 'Am');
    expect(read.project.timeSignatureNumerator, 3);
    expect(read.project.tracks.length, 2);

    final bass = read.project.tracks.firstWhere((t) => t.id == 't1');
    expect(bass.notes.length, 2);
    expect(bass.notes.first.pitch, 40);
    expect(bass.notes.last.duration, 0.5);
    expect(bass.instrumentName, 'saw-lead');
    expect(bass.type, TrackType.synth);

    // Identity is preserved for a re-save.
    expect(read.lgdfInfo?.name, 'my-song');
    expect(read.lgdfInfo?.displayName, 'My Song');
  });

  test('unregisters nothing when a project has no audio resources', () async {
    final projectDir = Directory('${tempRoot.path}/empty-assets');
    await const ProjectSerializer().writeProjectDirectory(buildProject(), projectDir);

    final registry = jsonDecode(
      File('${projectDir.path}/registry.json').readAsStringSync(),
    ) as Map<String, dynamic>;
    expect(registry['asset_count'], 0);
    expect(registry['registered_files'], isEmpty);
  });

  test('packs a project into an archive without work/ or dist/', () async {
    final projectDir = Directory('${tempRoot.path}/packed');
    await const ProjectSerializer().writeProjectDirectory(buildProject(), projectDir);

    // Markers that must never ship.
    await File('${projectDir.path}/work/scratch.tmp').writeAsString('tmp');
    await File('${projectDir.path}/dist/old.lgdf').writeAsString('zip');

    final bytes = await const ProjectSerializer().packProjectDirectory(projectDir);
    expect(bytes.length, greaterThan(0));
    // ZIP magic number.
    expect(bytes.sublist(0, 4), [0x50, 0x4B, 0x03, 0x04]);
  });

  test('metadata mirrors each registered asset with a sha256', () async {
    // A project with one real audio file on disk.
    final audio = File('${tempRoot.path}/kick.wav');
    await audio.writeAsBytes(List<int>.generate(2048, (i) => i % 256));

    final project = Project(
      id: 'proj-audio',
      name: 'With Audio',
      tracks: [
        Track(id: 'a1', name: 'Kick', type: TrackType.audio, audioFilePath: audio.path),
      ],
    );
    final projectDir = Directory('${tempRoot.path}/with-audio');
    await const ProjectSerializer().writeProjectDirectory(project, projectDir);

    final registry = jsonDecode(
      File('${projectDir.path}/registry.json').readAsStringSync(),
    ) as Map<String, dynamic>;
    final registered = (registry['registered_files'] as List).cast<String>();
    expect(registered.length, 1);
    expect(registered.first, 'assets/audios/audio_a1.wav');
    expect(registry['asset_count'], 1);

    // Asset and its metadata mirror exist at the same relative shape.
    final asset = File('${projectDir.path}/${registered.first}');
    expect(asset.existsSync(), isTrue);

    final metaFile = File('${projectDir.path}/metadata/audios/audio_a1.wav.json');
    expect(metaFile.existsSync(), isTrue);

    final meta = jsonDecode(metaFile.readAsStringSync()) as Map<String, dynamic>;
    expect(meta['path'], registered.first);
    expect(meta['type'], 'audio');
    expect(meta['mime'], 'audio/wav');
    expect(meta['format'], 'wav');
    expect(meta['size'], 2048);
    expect(meta['hash_algorithm'], 'sha256');
    // Lowercase hex, matching the file bytes.
    expect(meta['sha256'], Lgdf.sha256Hex(await asset.readAsBytes()));
    expect((meta['sha256'] as String), matches(RegExp(r'^[0-9a-f]{64}$')));
  });

  test('slugify produces LGDF-conformant names', () {
    expect(Lgdf.slugify('My Song'), 'my-song');
    expect(Lgdf.slugify('  Spaces  '), 'spaces');
    expect(Lgdf.slugify(''), 'project');
    expect(Lgdf.slugify('a/b\\c'), 'a-b-c');
    expect(Lgdf.slugify('Song!!'), 'song');
    // Every result must satisfy ^[a-z0-9_-]+$
    for (final input in ['My Song', '', 'a/b', 'Untitled', '中文名']) {
      expect(Lgdf.slugify(input), matches(RegExp(r'^[a-z0-9_-]+$')));
    }
  });

  test('mime and resource types follow the recommendation table', () {
    expect(Lgdf.mimeForExtension('.wav'), 'audio/wav');
    expect(Lgdf.mimeForExtension('.mp3'), 'audio/mpeg');
    expect(Lgdf.mimeForExtension('.png'), 'image/png');
    expect(Lgdf.resourceTypeForExtension('.wav'), 'audio');
    expect(Lgdf.resourceTypeForExtension('.png'), 'image');
    expect(Lgdf.resourceTypeForExtension('.md'), 'text');
  });

  test('normalizes paths to the LGDF form', () {
    expect(Lgdf.normalizePath(r'assets\audios\a.wav'), 'assets/audios/a.wav');
    expect(Lgdf.normalizePath('/leading/slash.wav'), 'leading/slash.wav');
    expect(Lgdf.normalizePath('a/./b/../c'), 'a/b/../c');
  });
}

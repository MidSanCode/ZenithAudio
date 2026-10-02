import 'dart:convert';
import 'dart:typed_data';

import 'package:archive/archive.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/services/project_migration.dart';

/// S9: project-format detection behind the migration wizard.
void main() {
  Uint8List zip(Map<String, String> files) {
    final archive = Archive();
    files.forEach((name, content) {
      final bytes = utf8.encode(content);
      archive.addFile(ArchiveFile(name, bytes.length, bytes));
    });
    return Uint8List.fromList(ZipEncoder().encode(archive));
  }

  group('detection', () {
    test('a spec/ overview marks LGDF v2.0', () {
      final bytes = zip({
        'info.json': '{"name":"Song","document_version":2}',
        'spec/overview.md': '# Song',
        'registry.json': '{"resources":[]}',
      });
      final probe = ProjectMigrationService.inspect(bytes, fileName: 'song.zaproj');
      expect(probe.format, ProjectFormat.lgdfV2);
      expect(probe.name, 'Song');
      expect(probe.documentVersion, 2);
      expect(probe.needsMigration, isFalse);
      expect(probe.isOpenable, isTrue);
    });

    test('registry without a spec layer marks legacy LGDF', () {
      final bytes = zip({
        'info.json': '{"name":"Old","version":1}',
        'registry.json': '{"resources":[]}',
        'assets/audios/a.wav': 'x',
      });
      final probe = ProjectMigrationService.inspect(bytes, fileName: 'old.lgdf');
      expect(probe.format, ProjectFormat.lgdfLegacy);
      expect(probe.needsMigration, isTrue);
    });

    test('a bare info.json marks legacy .zap', () {
      final bytes = zip({
        'info.json': '{"name":"Ancient","version":1}',
      });
      final probe = ProjectMigrationService.inspect(bytes, fileName: 'ancient.zap');
      expect(probe.format, ProjectFormat.zapLegacy);
      expect(probe.needsMigration, isTrue);
      expect(probe.name, 'Ancient');
    });

    test('detection is by content, so a renamed archive still opens', () {
      // A v2.0 archive with a .zap extension must still be seen as v2.0.
      final bytes = zip({
        'info.json': '{"name":"Renamed"}',
        'spec/overview.md': '',
      });
      final probe = ProjectMigrationService.inspect(bytes, fileName: 'misnamed.zap');
      expect(probe.format, ProjectFormat.lgdfV2);
      expect(probe.extension, '.zap');
    });
  });

  group('robustness', () {
    test('a non-zip byte stream reports unknown rather than throwing', () {
      final probe = ProjectMigrationService.inspect(
        Uint8List.fromList(List.filled(64, 0xAB)),
        fileName: 'garbage.zaproj',
      );
      expect(probe.format, ProjectFormat.unknown);
      expect(probe.isOpenable, isFalse);
    });

    test('a zip that is not a project reports unknown', () {
      final bytes = zip({'readme.txt': 'hello'});
      final probe = ProjectMigrationService.inspect(bytes);
      expect(probe.format, ProjectFormat.unknown);
    });

    test('a corrupt info.json does not lose the format detection', () {
      final bytes = zip({
        'info.json': '{ this is not json',
        'spec/overview.md': '',
      });
      final probe = ProjectMigrationService.inspect(bytes);
      expect(probe.format, ProjectFormat.lgdfV2);
      expect(probe.name, isNull);
    });
  });

  group('version guard', () {
    test('a file from a newer app is flagged', () {
      expect(
        ProjectMigrationService.isFromNewerApp(99, 2),
        isTrue,
      );
      expect(
        ProjectMigrationService.isFromNewerApp(2, 2),
        isFalse,
      );
      expect(ProjectMigrationService.isFromNewerApp(null, 2), isFalse);
    });
  });
}

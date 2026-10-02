/// Project-format detection and migration reporting (PLAN §3.S9 item 4).
///
/// ## What this is for
///
/// The app has shipped three on-disk shapes over its life:
///
/// | Format | Container | Layout |
/// |---|---|---|
/// | `.zap` (legacy) | ZIP | `info.json` holds the whole project inline |
/// | `.lgdf` (legacy) | ZIP | directory layout, pre-`spec/` |
/// | `.zaproj` (current) | ZIP | LGDF v2.0 directory layout |
///
/// Reading is already backward compatible — `ProjectSerializer.deserialize`
/// migrates a legacy archive on read. What was missing is a way to tell the user
/// **what** they are about to open and **whether it will be upgraded**, which is
/// what a migration wizard needs. This service answers exactly that, and it is
/// pure (bytes in, a report out) so it can be tested without touching disk.
///
/// ## Why detection is by content, not extension
///
/// A `.zaproj` renamed to `.zap` should still open correctly, and users do
/// rename files. The format is therefore decided by which entries the archive
/// actually contains, and the extension is reported only as a hint.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'package:archive/archive.dart';

import 'lgdf_format.dart';

/// The on-disk shape of a project archive.
enum ProjectFormat {
  /// Current LGDF v2.0 with a `spec/` description layer.
  lgdfV2,

  /// Legacy LGDF directory layout without the spec layer.
  lgdfLegacy,

  /// Legacy `.zap`: a single `info.json` holding everything.
  zapLegacy,

  /// Not a recognisable project archive.
  unknown,
}

/// What a reader found in an archive.
class ProjectProbe {
  /// The detected format.
  final ProjectFormat format;

  /// The project's display name, when the archive exposes one.
  final String? name;

  /// The `document_version` / `version` field, when present.
  final int? documentVersion;

  /// The archive's entry names (paths), for diagnostics.
  final List<String> entries;

  /// The file extension the archive was opened under, lower-cased, with dot.
  final String? extension;

  const ProjectProbe({
    required this.format,
    this.name,
    this.documentVersion,
    this.entries = const [],
    this.extension,
  });

  /// Whether opening this archive will run the legacy migration path.
  bool get needsMigration =>
      format == ProjectFormat.zapLegacy || format == ProjectFormat.lgdfLegacy;

  /// Whether the archive is a project this build can open at all.
  bool get isOpenable => format != ProjectFormat.unknown;

  /// A human-readable summary of the format.
  String get formatLabel => switch (format) {
        ProjectFormat.lgdfV2 => 'LGDF v2.0 (.zaproj)',
        ProjectFormat.lgdfLegacy => 'Legacy LGDF',
        ProjectFormat.zapLegacy => 'Legacy .zap',
        ProjectFormat.unknown => 'Unrecognised',
      };

  /// Whether [entry] (a path) exists in the archive.
  bool has(String entry) => entries.contains(entry);
}

/// Inspects project archives and reports their format.
abstract final class ProjectMigrationService {
  /// Inspects [bytes] as a project archive, returning what was found.
  ///
  /// Never throws: a corrupt or non-project byte stream reports
  /// [ProjectFormat.unknown] rather than propagating a parse error, because a
  /// wizard's first job is to say "this is not a project" clearly.
  static ProjectProbe inspect(Uint8List bytes, {String? fileName}) {
    final extension = _extensionOf(fileName);
    Archive archive;
    try {
      archive = ZipDecoder().decodeBytes(bytes);
    } catch (_) {
      return ProjectProbe(
        format: ProjectFormat.unknown,
        entries: const [],
        extension: extension,
      );
    }

    final entries = archive.files
        .where((f) => f.isFile)
        .map((f) => f.name)
        .toList(growable: false);

    final hasOverview = entries.contains('${Lgdf.specDir}/overview.md');
    final hasProjectSpec = entries.contains('${Lgdf.specDir}/project.json');
    final hasRegistry = entries.contains(Lgdf.registryFile);
    final hasInfo = entries.contains(Lgdf.infoFile);
    final hasAudioAssets = entries.any((e) => e.startsWith('${Lgdf.audioAssetsDir}/'));

    // Parse info.json for the name and version, whichever format this is.
    String? name;
    int? documentVersion;
    if (hasInfo) {
      final info = _readJson(archive, Lgdf.infoFile);
      if (info != null) {
        name = info['name'] as String?;
        documentVersion = (info['document_version'] as num?)?.toInt() ??
            (info['version'] as num?)?.toInt();
      }
    }

    final ProjectFormat format;
    if (hasOverview || hasProjectSpec) {
      // The description layer only exists in v2.0.
      format = ProjectFormat.lgdfV2;
    } else if (hasRegistry || hasAudioAssets || hasInfo) {
      // A directory-style archive without the spec layer is legacy LGDF; a bare
      // `info.json` with no registry is the old single-file `.zap`.
      format = hasRegistry || hasAudioAssets
          ? ProjectFormat.lgdfLegacy
          : ProjectFormat.zapLegacy;
    } else {
      format = ProjectFormat.unknown;
    }

    return ProjectProbe(
      format: format,
      name: name,
      documentVersion: documentVersion,
      entries: entries,
      extension: extension,
    );
  }

  /// Whether `version` requires a newer app than this build.
  ///
  /// Mirrors the codec's own check so a wizard can warn before the open fails.
  static bool isFromNewerApp(int? version, int currentFormatVersion) {
    if (version == null) return false;
    return version > currentFormatVersion;
  }

  static String? _extensionOf(String? fileName) {
    if (fileName == null) return null;
    final dot = fileName.lastIndexOf('.');
    if (dot < 0) return null;
    return fileName.substring(dot).toLowerCase();
  }

  static Map<String, dynamic>? _readJson(Archive archive, String path) {
    for (final file in archive.files) {
      if (file.name == path && file.isFile) {
        try {
          final text = utf8.decode(file.content);
          final decoded = jsonDecode(text);
          if (decoded is Map<String, dynamic>) return decoded;
        } catch (_) {
          return null;
        }
      }
    }
    return null;
  }
}

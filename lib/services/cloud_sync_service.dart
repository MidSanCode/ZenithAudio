import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import '../core/utils/logger.dart';
import 'cloud_sync_config.dart';
import 'lgdf_format.dart';
import 'project_serializer.dart';
import 'webdav_client.dart';

/// Per-project sync status shown in the workspace.
enum SyncStatus {
  /// Never uploaded to the cloud.
  notSynced,

  /// Local and cloud hold the same content.
  synced,

  /// Local changed since the last sync; cloud did not. Upload needed.
  ahead,

  /// Cloud changed since the last sync; local did not. Download needed.
  behind,

  /// Both sides changed — the user must choose which version to keep.
  conflict,

  /// The last sync operation failed (network/auth/quota).
  failed,
}

/// Sync record persisted per project in the workspace `.sync-state.json`.
class ProjectSyncRecord {
  final String slug;

  /// sha256 of the local project package at the last successful sync.
  final String lastLocalHash;

  /// sha256 recorded in the cloud meta at the last successful sync.
  final String lastRemoteHash;

  /// When the last successful sync happened (epoch seconds).
  final int lastSyncedAt;

  final SyncStatus status;

  /// Human-readable failure reason when [status] is [SyncStatus.failed].
  final String? lastError;

  const ProjectSyncRecord({
    required this.slug,
    this.lastLocalHash = '',
    this.lastRemoteHash = '',
    this.lastSyncedAt = 0,
    this.status = SyncStatus.notSynced,
    this.lastError,
  });

  ProjectSyncRecord copyWith({
    SyncStatus? status,
    String? lastLocalHash,
    String? lastRemoteHash,
    int? lastSyncedAt,
    String? lastError,
    bool clearError = false,
  }) {
    return ProjectSyncRecord(
      slug: slug,
      lastLocalHash: lastLocalHash ?? this.lastLocalHash,
      lastRemoteHash: lastRemoteHash ?? this.lastRemoteHash,
      lastSyncedAt: lastSyncedAt ?? this.lastSyncedAt,
      status: status ?? this.status,
      lastError: clearError ? null : (lastError ?? this.lastError),
    );
  }

  Map<String, dynamic> toJson() => {
        'slug': slug,
        'last_local_hash': lastLocalHash,
        'last_remote_hash': lastRemoteHash,
        'last_synced_at': lastSyncedAt,
        'status': status.name,
        if (lastError != null) 'last_error': lastError,
      };

  static ProjectSyncRecord fromJson(Map<String, dynamic> json) {
    return ProjectSyncRecord(
      slug: json['slug'] as String,
      lastLocalHash: json['last_local_hash'] as String? ?? '',
      lastRemoteHash: json['last_remote_hash'] as String? ?? '',
      lastSyncedAt: json['last_synced_at'] as int? ?? 0,
      status: SyncStatus.values.firstWhere(
        (s) => s.name == json['status'],
        orElse: () => SyncStatus.notSynced,
      ),
      lastError: json['last_error'] as String?,
    );
  }
}

/// What a status check found for one project.
class SyncCheck {
  final SyncStatus status;
  final String localHash;
  final String remoteHash;
  const SyncCheck({
    required this.status,
    required this.localHash,
    required this.remoteHash,
  });
}

/// The user's choice when local and cloud have both changed.
enum ConflictChoice { keepLocal, keepRemote, cancel }

/// Sync engine: packs local LGDF project directories into `.zaproj` archives
/// and mirrors them to a WebDAV backend (MSC cloud or a third-party server).
///
/// Remote layout (relative to the WebDAV root):
/// ```
/// zenith-audio/<slug>.zaproj          packed project
/// zenith-audio/<slug>.meta.json       {"sha256","synced_at","name"}
/// ```
///
/// The sidecar meta is the source of truth for the remote hash — PROPFIND is
/// deliberately avoided so the engine works with minimal WebDAV servers.
class CloudSyncService {
  static const String remoteRoot = 'zenith-audio';

  final SyncConfig config;
  final WebDavClient _client;

  CloudSyncService(this.config) : _client = config.createClient();

  /// Verifies server reachability and credentials.
  Future<void> testConnection() => _client.testConnection();

  // ── Remote paths ──

  static String remoteArchive(String slug) => '$remoteRoot/$slug.zaproj';
  static String remoteMeta(String slug) => '$remoteRoot/$slug.meta.json';

  // ── Status ──

  /// Deterministic content hash of a project directory.
  ///
  /// sha256 over the sorted list of `"<path>:<file-sha256>"` lines, using the
  /// same exclusion rules as packing (`work/`, `dist/`, `.tmp/`, dotfiles).
  /// Deliberately NOT the hash of the packed archive: zip entry timestamps
  /// make archive bytes unstable for identical content.
  Future<String> contentHash(Directory projectDir) async {
    final rootPath = projectDir.path.replaceAll('\\', '/');
    final entries = <String>[];

    await for (final entity
        in projectDir.list(recursive: true, followLinks: false)) {
      if (entity is! File) continue;
      final full = entity.path.replaceAll('\\', '/');
      final rel =
          full.startsWith('$rootPath/') ? full.substring(rootPath.length + 1) : full;
      final normalized = Lgdf.normalizePath(rel);

      final top = normalized.split('/').first;
      if (Lgdf.excludedFromPack.contains(top)) continue;
      if (normalized.startsWith('.')) continue;

      final fileHash = Lgdf.sha256Hex(await entity.readAsBytes());
      entries.add('$normalized:$fileHash');
    }

    entries.sort();
    return Lgdf.sha256Hex(utf8.encode(entries.join('\n')));
  }

  /// Compares local content, remote meta, and the stored sync record.
  Future<SyncCheck> check(
    Directory projectDir,
    String slug,
    ProjectSyncRecord record,
  ) async {
    final local = await contentHash(projectDir);
    final remote = await _remoteHash(slug);

    if (remote == null) {
      final status = record.lastSyncedAt == 0
          ? SyncStatus.notSynced
          : SyncStatus.ahead; // remote deleted → treat as local-ahead
      return SyncCheck(status: status, localHash: local, remoteHash: '');
    }

    final localChanged =
        record.lastLocalHash.isNotEmpty && record.lastLocalHash != local;
    final remoteChanged =
        record.lastRemoteHash.isNotEmpty && record.lastRemoteHash != remote;
    final neverSynced = record.lastSyncedAt == 0;

    final SyncStatus status;
    if (local == remote) {
      status = SyncStatus.synced;
    } else if (neverSynced) {
      // Uploaded before by another install, or record lost.
      status = SyncStatus.conflict;
    } else if (localChanged && remoteChanged) {
      status = SyncStatus.conflict;
    } else if (localChanged) {
      status = SyncStatus.ahead;
    } else if (remoteChanged) {
      status = SyncStatus.behind;
    } else {
      // Hashes differ but neither changed vs. the record: a first sync of
      // pre-existing content on both sides.
      status = SyncStatus.conflict;
    }
    return SyncCheck(status: status, localHash: local, remoteHash: remote);
  }

  Future<String?> _remoteHash(String slug) async {
    if (!await _client.exists(remoteArchive(slug))) return null;
    try {
      final bytes = await _client.download(remoteMeta(slug));
      final meta = jsonDecode(utf8.decode(bytes)) as Map<String, dynamic>;
      return meta['sha256'] as String?;
    } catch (_) {
      // Archive exists but meta is unreadable: treat as unknown remote state.
      return '';
    }
  }

  // ── Operations ──

  /// Uploads the local project, overwriting the cloud copy.
  Future<ProjectSyncRecord> push(
    Directory projectDir,
    String slug,
    ProjectSyncRecord record,
  ) async {
    try {
      final bytes =
          await const ProjectSerializer().packProjectDirectory(projectDir);
      // Hash the content (not the archive bytes) — see contentHash.
      final hash = await contentHash(projectDir);
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;

      await _client.upload(remoteArchive(slug), bytes);
      await _client.upload(
        remoteMeta(slug),
        utf8.encode(jsonEncode({
          'sha256': hash,
          'synced_at': now,
          'name': projectDir.uri.pathSegments
              .lastWhere((s) => s.isNotEmpty, orElse: () => slug),
        })),
      );
      AppLogger.i('Synced project to cloud: $slug');
      return record.copyWith(
        status: SyncStatus.synced,
        lastLocalHash: hash,
        lastRemoteHash: hash,
        lastSyncedAt: now,
        clearError: true,
      );
    } catch (e) {
      AppLogger.e('Cloud upload failed: $slug', e);
      return record.copyWith(status: SyncStatus.failed, lastError: '$e');
    }
  }

  /// Downloads the cloud archive bytes and the hash from its meta sidecar.
  ///
  /// The caller materializes the archive into a project directory and then
  /// recomputes [contentHash] on it to update the sync record — the meta hash
  /// alone is not trusted for record-keeping.
  Future<Uint8List> pull(String slug) => _client.download(remoteArchive(slug));

  /// Removes the project (archive + meta) from the cloud.
  Future<void> removeRemote(String slug) async {
    await _client.delete(remoteArchive(slug));
    await _client.delete(remoteMeta(slug));
  }
}

/// Persists per-project sync records to `<workspace>/.sync-state.json`.
class SyncStateStore {
  final File _file;

  SyncStateStore(Directory workspaceDir)
      : _file = File('${workspaceDir.path}/.sync-state.json');

  Future<Map<String, ProjectSyncRecord>> load() async {
    try {
      if (!await _file.exists()) return {};
      final json = jsonDecode(await _file.readAsString()) as List<dynamic>;
      return {
        for (final entry in json.cast<Map<String, dynamic>>())
          entry['slug'] as String: ProjectSyncRecord.fromJson(entry),
      };
    } catch (e) {
      AppLogger.w('Could not read sync state: $e');
      return {};
    }
  }

  Future<void> save(Map<String, ProjectSyncRecord> records) async {
    try {
      final list = records.values.map((r) => r.toJson()).toList()
        ..sort((a, b) => (a['slug'] as String).compareTo(b['slug'] as String));
      await _file.writeAsString(const JsonEncoder.withIndent('\t').convert(list));
    } catch (e) {
      AppLogger.w('Could not write sync state: $e');
    }
  }
}

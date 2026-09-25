import 'dart:io';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../core/utils/logger.dart';
import '../services/cloud_sync_config.dart';
import '../services/cloud_sync_service.dart';
import '../services/project_serializer.dart';
import '../services/workspace_service.dart';
import 'workspace_provider.dart';

/// The saved cloud sync configuration (null = not configured).
final syncConfigProvider =
    AsyncNotifierProvider<SyncConfigNotifier, SyncConfig?>(
  SyncConfigNotifier.new,
);

class SyncConfigNotifier extends AsyncNotifier<SyncConfig?> {
  @override
  Future<SyncConfig?> build() => SyncConfigStore.load();

  Future<void> save(SyncConfig config) async {
    await SyncConfigStore.save(config);
    state = AsyncData(config);
  }

  Future<void> clear() async {
    await SyncConfigStore.clear();
    state = const AsyncData(null);
  }

  /// Throws on failure — the settings dialog shows the error text.
  Future<void> testConnection(SyncConfig config) =>
      CloudSyncService(config).testConnection();
}

/// Per-project sync records (slug → record), persisted in the workspace.
final syncStateProvider =
    AsyncNotifierProvider<SyncStateNotifier, Map<String, ProjectSyncRecord>>(
  SyncStateNotifier.new,
);

/// The outcome of one sync attempt, surfaced to the UI.
enum SyncOutcome { synced, conflict, failed, skipped }

class SyncStateNotifier extends AsyncNotifier<Map<String, ProjectSyncRecord>> {
  late Directory _workspaceDir;

  @override
  Future<Map<String, ProjectSyncRecord>> build() async {
    _workspaceDir = Directory(await WorkspaceService().directoryPath());
    return SyncStateStore(_workspaceDir).load();
  }

  Future<void> _persist() async {
    final current = state.value;
    if (current != null) await SyncStateStore(_workspaceDir).save(current);
  }

  ProjectSyncRecord _recordFor(Map<String, ProjectSyncRecord> map, String slug) =>
      map[slug] ?? ProjectSyncRecord(slug: slug);

  /// Refreshes the status of one project (network call against the cloud).
  Future<SyncStatus> refreshStatus(String projectPath, String slug) async {
    final config = ref.read(syncConfigProvider).value;
    final map = state.value ?? {};
    if (config == null || !config.isComplete || kIsWeb) {
      return _recordFor(map, slug).status;
    }
    final service = CloudSyncService(config);
    final record = _recordFor(map, slug);
    try {
      final check = await service.check(Directory(projectPath), slug, record);
      final updated = record.copyWith(status: check.status, clearError: true);
      state = AsyncData({...map, slug: updated});
      await _persist();
      return check.status;
    } catch (e) {
      final updated =
          record.copyWith(status: SyncStatus.failed, lastError: '$e');
      state = AsyncData({...map, slug: updated});
      await _persist();
      return SyncStatus.failed;
    }
  }

  /// Uploads one project. When both sides changed, returns
  /// [SyncOutcome.conflict] without touching anything — the UI must ask the
  /// user which version to keep, then call [resolveConflict].
  Future<SyncOutcome> syncProject(String projectPath, String slug) async {
    final config = ref.read(syncConfigProvider).value;
    final map = state.value ?? {};
    if (config == null || !config.isComplete || kIsWeb) {
      return SyncOutcome.skipped;
    }

    final service = CloudSyncService(config);
    final record = _recordFor(map, slug);
    try {
      final check = await service.check(Directory(projectPath), slug, record);
      switch (check.status) {
        case SyncStatus.synced:
          state = AsyncData({...map, slug: record.copyWith(status: SyncStatus.synced)});
          return SyncOutcome.synced;
        case SyncStatus.conflict:
          state = AsyncData(
              {...map, slug: record.copyWith(status: SyncStatus.conflict)});
          await _persist();
          return SyncOutcome.conflict;
        case SyncStatus.notSynced:
        case SyncStatus.ahead:
          return _finishPush(service, projectPath, slug, record);
        case SyncStatus.behind:
          return _finishPull(service, projectPath, slug, record);
        case SyncStatus.failed:
          return SyncOutcome.failed;
      }
    } catch (e) {
      AppLogger.e('Sync failed: $slug', e);
      state = AsyncData({
        ...map,
        slug: record.copyWith(status: SyncStatus.failed, lastError: '$e'),
      });
      await _persist();
      return SyncOutcome.failed;
    }
  }

  /// Applies the user's choice after a conflict was reported.
  Future<SyncOutcome> resolveConflict(
    String projectPath,
    String slug,
    ConflictChoice choice,
  ) async {
    if (choice == ConflictChoice.cancel) return SyncOutcome.skipped;
    final config = ref.read(syncConfigProvider).value;
    final map = state.value ?? {};
    if (config == null || !config.isComplete) return SyncOutcome.skipped;

    final service = CloudSyncService(config);
    final record = _recordFor(map, slug);
    return choice == ConflictChoice.keepLocal
        ? _finishPush(service, projectPath, slug, record)
        : _finishPull(service, projectPath, slug, record);
  }

  Future<SyncOutcome> _finishPush(
    CloudSyncService service,
    String projectPath,
    String slug,
    ProjectSyncRecord record,
  ) async {
    final updated =
        await service.push(Directory(projectPath), slug, record);
    state = AsyncData({...(state.value ?? {}), slug: updated});
    await _persist();
    return updated.status == SyncStatus.synced
        ? SyncOutcome.synced
        : SyncOutcome.failed;
  }

  /// Downloads the cloud copy and replaces the local project directory.
  ///
  /// The old directory is renamed to `<slug>.bak` first and only removed once
  /// the new content is fully written, so a failed pull never destroys data.
  Future<SyncOutcome> _finishPull(
    CloudSyncService service,
    String projectPath,
    String slug,
    ProjectSyncRecord record,
  ) async {
    try {
      final bytes = await service.pull(slug);
      final target = Directory(projectPath);
      final backup = Directory('$projectPath.bak');
      if (await backup.exists()) await backup.delete(recursive: true);
      if (await target.exists()) await target.rename(backup.path);

      try {
        await const ProjectSerializer().unpackProjectArchive(bytes, target);
      } catch (e) {
        // Roll back: restore the backup.
        if (await target.exists()) await target.delete(recursive: true);
        if (await backup.exists()) await backup.rename(projectPath);
        rethrow;
      }
      if (await backup.exists()) await backup.delete(recursive: true);

      final hash = await service.contentHash(target);
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      final updated = record.copyWith(
        status: SyncStatus.synced,
        lastLocalHash: hash,
        lastRemoteHash: hash,
        lastSyncedAt: now,
        clearError: true,
      );
      state = AsyncData({...(state.value ?? {}), slug: updated});
      await _persist();

      // The workspace list shows the replaced project.
      ref.invalidate(workspaceProjectsProvider);
      AppLogger.i('Pulled cloud version into local project: $slug');
      return SyncOutcome.synced;
    } catch (e) {
      AppLogger.e('Cloud download failed: $slug', e);
      state = AsyncData({
        ...(state.value ?? {}),
        slug: record.copyWith(status: SyncStatus.failed, lastError: '$e'),
      });
      await _persist();
      return SyncOutcome.failed;
    }
  }

  /// Marks a project as not-synced after it is deleted locally / remotely.
  Future<void> forget(String slug) async {
    final map = {...(state.value ?? {})}..remove(slug);
    state = AsyncData(map);
    await _persist();
  }
}

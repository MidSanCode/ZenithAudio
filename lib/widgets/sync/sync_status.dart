import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../providers/sync_provider.dart';
import '../../services/cloud_sync_service.dart';

/// Small status chip shown on workspace project cards.
class SyncStatusBadge extends StatelessWidget {
  final SyncStatus status;
  const SyncStatusBadge({super.key, required this.status});

  @override
  Widget build(BuildContext context) {
    final (icon, color, label) = switch (status) {
      SyncStatus.synced => (Icons.cloud_done_rounded, const Color(0xFF4CAF50), 'sync.state.synced'),
      SyncStatus.notSynced => (Icons.cloud_off_rounded, const Color(0xFF9E9E9E), 'sync.state.notSynced'),
      SyncStatus.ahead => (Icons.cloud_upload_rounded, const Color(0xFF2196F3), 'sync.state.ahead'),
      SyncStatus.behind => (Icons.cloud_download_rounded, const Color(0xFF2196F3), 'sync.state.behind'),
      SyncStatus.conflict => (Icons.cloud_sync_rounded, const Color(0xFFFF9800), 'sync.state.conflict'),
      SyncStatus.failed => (Icons.cloud_off_rounded, const Color(0xFFF44336), 'sync.state.failed'),
    };

    return Tooltip(
      message: label.tr(),
      child: Icon(icon, size: 13, color: color),
    );
  }
}

/// Runs the sync flow for one workspace project, including the conflict
/// dialog. Call from the workspace card's sync button.
///
/// Returns true when the project ended up synced.
Future<bool> syncProjectWithConflictResolution(
  BuildContext context,
  WidgetRef ref, {
  required String projectPath,
  required String slug,
  required String displayName,
}) async {
  final notifier = ref.read(syncStateProvider.notifier);
  final outcome = await notifier.syncProject(projectPath, slug);

  switch (outcome) {
    case SyncOutcome.synced:
      return true;
    case SyncOutcome.conflict:
      if (!context.mounted) return false;
      final choice = await _askConflictChoice(context, displayName);
      if (choice == null || choice == ConflictChoice.cancel) return false;
      if (!context.mounted) return false;
      final resolved = await notifier.resolveConflict(projectPath, slug, choice);
      return resolved == SyncOutcome.synced;
    case SyncOutcome.failed:
      if (context.mounted) {
        final error = ref.read(syncStateProvider).value?[slug]?.lastError;
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(
          content: Text('sync.failed'.tr(namedArgs: {
            'error': (error == null || error.isEmpty) ? '-' : error,
          })),
          duration: const Duration(seconds: 3),
        ));
      }
      return false;
    case SyncOutcome.skipped:
      return false;
  }
}

/// Conflict dialog: both sides changed — which version wins?
Future<ConflictChoice?> _askConflictChoice(
  BuildContext context,
  String displayName,
) {
  return showDialog<ConflictChoice>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text('sync.conflict.title'.tr()),
      content: Text(
        'sync.conflict.message'.tr(namedArgs: {'name': displayName}),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(ConflictChoice.cancel),
          child: Text('common.cancel'.tr()),
        ),
        TextButton.icon(
          icon: const Icon(Icons.cloud_download_outlined, size: 16),
          onPressed: () => Navigator.of(ctx).pop(ConflictChoice.keepRemote),
          label: Text('sync.conflict.keepRemote'.tr()),
        ),
        FilledButton.icon(
          icon: const Icon(Icons.upload_rounded, size: 16),
          onPressed: () => Navigator.of(ctx).pop(ConflictChoice.keepLocal),
          label: Text('sync.conflict.keepLocal'.tr()),
        ),
      ],
    ),
  );
}

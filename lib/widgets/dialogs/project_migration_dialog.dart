import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';

import '../../services/project_migration.dart';

/// Shows the migration/inspection summary before an archive is opened.
///
/// Returns `true` to proceed, `false` to cancel. The dialog is informational,
/// not a gate on a destructive action — the migration itself is non-destructive
/// (the source file is never modified; the app migrates in memory and writes the
/// new format only on save), so the copy says so plainly rather than alarming
/// the user.
Future<bool> confirmProjectMigration(
  BuildContext context,
  ProjectProbe probe,
) async {
  final result = await showDialog<bool>(
    context: context,
    builder: (context) {
      final cs = Theme.of(context).colorScheme;
      return AlertDialog(
        title: Text('migration.title'.tr()),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('migration.detected'.tr(namedArgs: {
              'format': probe.formatLabel,
            })),
            if (probe.name != null) ...[
              const SizedBox(height: 6),
              Text('migration.project'.tr(namedArgs: {'name': probe.name!}),
                  style: TextStyle(color: cs.onSurfaceVariant)),
            ],
            if (probe.needsMigration) ...[
              const SizedBox(height: 10),
              Text('migration.willUpgrade'.tr()),
              const SizedBox(height: 6),
              Text('migration.nonDestructive'.tr(),
                  style: TextStyle(fontSize: 12, color: cs.onSurfaceVariant)),
            ],
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text('common.cancel'.tr()),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(
              probe.needsMigration ? 'migration.upgradeOpen'.tr() : 'migration.open'.tr(),
            ),
          ),
        ],
      );
    },
  );
  return result ?? false;
}

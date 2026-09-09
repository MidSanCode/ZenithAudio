import 'package:flutter/material.dart';
import 'package:easy_localization/easy_localization.dart';
import '../core/constants/app_constants.dart';
import '../core/constants/app_config.dart';
import '../core/utils/theme_colors.dart';

class AboutDialog extends StatelessWidget {
  const AboutDialog({super.key});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Dialog(
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
      child: Container(
        padding: const EdgeInsets.all(24),
        width: 320,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 56, height: 56,
              decoration: BoxDecoration(
                color: cs.primary,
                borderRadius: BorderRadius.circular(12),
              ),
              child: const Icon(Icons.waves, size: 32, color: Colors.white),
            ),
            const SizedBox(height: 12),
            Text(AppConstants.appName,
              style: Theme.of(context).textTheme.headlineSmall?.copyWith(
                fontWeight: FontWeight.w700,
              ),
            ),
            const SizedBox(height: 2),
            Text(AppConstants.appNameEn,
              style: TextStyle(
                color: cs.primary, fontSize: 11,
                fontWeight: FontWeight.w600, letterSpacing: 2,
              ),
            ),
            const SizedBox(height: 20),
            _InfoRow(label: 'about.version'.tr(), value: AppConfig.appVersion),
            const SizedBox(height: 6),
            _InfoRow(label: 'about.build'.tr(), value: AppConfig.appBuildNumber),
            const SizedBox(height: 20),
            Text(AppConfig.appCopyright,
              textAlign: TextAlign.center,
              style: TextStyle(color: context.outline, fontSize: 11),
            ),
            const SizedBox(height: 20),
            SizedBox(
              width: double.infinity,
              child: TextButton(
                onPressed: () => Navigator.of(context).pop(),
                style: TextButton.styleFrom(
                  backgroundColor: cs.primary,
                  foregroundColor: Colors.white,
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(6),
                  ),
                  padding: const EdgeInsets.symmetric(vertical: 10),
                ),
                child: Text('common.ok'.tr(),
                  style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 13)),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _InfoRow extends StatelessWidget {
  final String label;
  final String value;
  const _InfoRow({required this.label, required this.value});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Text('$label: ', style: TextStyle(color: context.outline, fontSize: 12)),
        Text(value,
          style: TextStyle(color: cs.onSurface, fontSize: 12, fontWeight: FontWeight.w500),
        ),
      ],
    );
  }
}

import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../providers/web_degradation_provider.dart';

/// The persistent audio-degradation warning bar (PLAN §3.S1.5).
///
/// ## Placement
///
/// Mounted once, high in the widget tree, so it appears at the top of every
/// screen. It is a sibling of the page content, not a `SnackBar`, because a
/// snack bar auto-dismisses — and the plan is explicit that this warning must
/// **not** disappear on its own. A user who never saw it flash by would conclude
/// the problem fixed itself and then be surprised by the next dropout.
///
/// ## What it offers
///
/// * **Still enable** — restore the disabled features for this session. The bar
///   stays, because the engine did not actually get faster.
/// * **Restore** — when sustained healthy operation makes a recovery available,
///   step back up one tier.
/// * **Lower sample rate / larger buffer** — the two changes that most often
///   actually help, offered as a shortcut rather than making the user find them.
class AudioDegradationBanner extends ConsumerWidget {
  const AudioDegradationBanner({super.key, this.onLowerQuality});

  /// Called when the user asks to lower the sample rate / enlarge the buffer.
  ///
  /// Injected rather than hard-wired so the settings live where they already
  /// live; the banner only knows that the user asked.
  final VoidCallback? onLowerQuality;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final state = ref.watch(webDegradationProvider);
    if (!state.warningActive) {
      return const SizedBox.shrink();
    }

    final cs = Theme.of(context).colorScheme;
    final disabled = state.disabledFeatures;

    return Material(
      color: cs.errorContainer,
      child: SafeArea(
        bottom: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 6, 8, 6),
          child: Row(
            children: [
              Icon(Icons.graphic_eq, size: 18, color: cs.onErrorContainer),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      state.userForced
                          ? 'degrade.titleForced'.tr()
                          : 'degrade.title'.tr(),
                      style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: cs.onErrorContainer,
                      ),
                    ),
                    if (!state.userForced && disabled.isNotEmpty)
                      Text(
                        'degrade.disabled'
                            .tr(namedArgs: {'features': _describe(disabled)}),
                        style: TextStyle(fontSize: 11, color: cs.onErrorContainer),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                      ),
                  ],
                ),
              ),
              if (state.recoveryAvailable && !state.userForced)
                TextButton(
                  onPressed: () => ref.read(webDegradationProvider.notifier).recover(),
                  child: Text('degrade.restore'.tr()),
                ),
              if (!state.userForced)
                TextButton(
                  onPressed: () => ref.read(webDegradationProvider.notifier).enableAll(),
                  child: Text('degrade.stillEnable'.tr()),
                ),
              if (onLowerQuality != null)
                IconButton(
                  tooltip: 'degrade.lowerQuality'.tr(),
                  onPressed: onLowerQuality,
                  icon: const Icon(Icons.tune, size: 18),
                  color: cs.onErrorContainer,
                ),
            ],
          ),
        ),
      ),
    );
  }

  /// A short, human list of the disabled features.
  ///
  /// Uses `, ` rather than localising a list separator: the feature names are
  /// already localised and a separator is cosmetic.
  String _describe(Set<DegradedFeature> features) {
    // Stable order so the message does not reshuffle between rebuilds.
    final ordered = [
      DegradedFeature.convolutionReverb,
      DegradedFeature.oversampledDistortion,
      DegradedFeature.highRatioTimeStretch,
      DegradedFeature.sendBuses,
      DegradedFeature.realtimeEffects,
      DegradedFeature.extendedPolyphony,
    ].where(features.contains);
    return ordered.map(_featureName).join(', ');
  }

  String _featureName(DegradedFeature feature) => switch (feature) {
        DegradedFeature.convolutionReverb => 'degrade.feature.convolutionReverb'.tr(),
        DegradedFeature.oversampledDistortion => 'degrade.feature.oversampledDistortion'.tr(),
        DegradedFeature.highRatioTimeStretch => 'degrade.feature.highRatioTimeStretch'.tr(),
        DegradedFeature.sendBuses => 'degrade.feature.sendBuses'.tr(),
        DegradedFeature.realtimeEffects => 'degrade.feature.realtimeEffects'.tr(),
        DegradedFeature.extendedPolyphony => 'degrade.feature.extendedPolyphony'.tr(),
      };
}

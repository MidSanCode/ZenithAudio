import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../providers/ab_compare_provider.dart';

/// The A/B comparison bar (PLAN §3.S9 item 1).
///
/// Appears in the editor when comparison mode is armed and lets the user flip
/// between two full project snapshots with one click (or `Shift+Tab`). A mix
/// decision is only answerable by listening to both options back to back, and
/// this keeps that comparison to a single gesture rather than a save/reload.
///
/// The bar also shows which side is live and whether it differs from the other,
/// because "am I on A or B?" is exactly the question a listener loses track of.
class AbCompareBar extends ConsumerWidget {
  const AbCompareBar({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final state = ref.watch(abCompareProvider);
    if (!state.active) return const SizedBox.shrink();

    final cs = Theme.of(context).colorScheme;
    final notifier = ref.read(abCompareProvider.notifier);

    return Focus(
      // Shift+Tab flips sides, so a user can compare without moving the mouse.
      onKeyEvent: (node, event) {
        if (event is KeyDownEvent &&
            event.logicalKey == LogicalKeyboardKey.tab &&
            HardwareKeyboard.instance.isShiftPressed) {
          notifier.toggle();
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      },
      child: Container(
        height: 30,
        color: cs.secondaryContainer,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(
          children: [
            Icon(Icons.compare_arrows, size: 16, color: cs.onSecondaryContainer),
            const SizedBox(width: 8),
            Text(
              'ab.label'.tr(),
              style: TextStyle(fontSize: 12, color: cs.onSecondaryContainer),
            ),
            const SizedBox(width: 12),
            _SideChip(
              label: 'A',
              selected: state.side == AbSide.a,
              onTap: () => notifier.switchTo(AbSide.a),
            ),
            const SizedBox(width: 4),
            _SideChip(
              label: 'B',
              selected: state.side == AbSide.b,
              onTap: () => notifier.switchTo(AbSide.b),
            ),
            const SizedBox(width: 12),
            TextButton(
              onPressed: notifier.copyLiveToOther,
              child: Text('ab.copyToOther'.tr()),
            ),
            const Spacer(),
            TextButton(
              onPressed: notifier.disarm,
              child: Text('ab.exit'.tr()),
            ),
          ],
        ),
      ),
    );
  }
}

class _SideChip extends StatelessWidget {
  const _SideChip({
    required this.label,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(4),
      child: Container(
        width: 28,
        height: 20,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: selected ? cs.primary : cs.surface,
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 12,
            fontWeight: FontWeight.w600,
            color: selected ? cs.onPrimary : cs.onSurface,
          ),
        ),
      ),
    );
  }
}

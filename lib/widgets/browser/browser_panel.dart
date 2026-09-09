import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/theme_colors.dart';
import '../../providers/browser_provider.dart';

class BrowserPanel extends ConsumerWidget {
  const BrowserPanel({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final visible = ref.watch(browserVisibilityProvider);
    if (!visible) return const SizedBox.shrink();

    return Container(
      width: AppConstants.browserPanelWidth,
      decoration: BoxDecoration(
        color: context.surfaceHigh,
        border: Border(
          right: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Column(
        children: [
          _BrowserHeader(),
          const _BrowserTabs(),
          const Expanded(child: _BrowserContent()),
        ],
      ),
    );
  }
}

class _BrowserHeader extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      height: AppConstants.timelineHeight,
      padding: const EdgeInsets.symmetric(horizontal: 6),
      decoration: BoxDecoration(
        color: Colors.black.withAlpha(51),
        border: Border(
          bottom: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: [
          Icon(Icons.folder_open_rounded, size: 12, color: cs.onSurfaceVariant),
          const SizedBox(width: 4),
          Text(
            'BROWSER',
            style: TextStyle(
              color: cs.onSurfaceVariant,
              fontSize: 9,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.2,
            ),
          ),
          const Spacer(),
          GestureDetector(
            onTap: () => context
                .findAncestorWidgetOfExactType<ConsumerWidget>() == null
                ? null
                : null,
            child: Icon(Icons.close_rounded, size: 12, color: cs.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

class _BrowserTabs extends ConsumerWidget {
  const _BrowserTabs();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final currentTab = ref.watch(browserTabProvider);
    final cs = Theme.of(context).colorScheme;

    final tabs = [
      (BrowserTab.samples, 'browser.samples'.tr()),
      (BrowserTab.presets, 'browser.presets'.tr()),
      (BrowserTab.projects, 'browser.projects'.tr()),
    ];

    return Container(
      height: 22,
      decoration: BoxDecoration(
        color: Colors.black.withAlpha(38),
        border: Border(
          bottom: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Row(
        children: tabs.map((tab) {
          final isActive = tab.$1 == currentTab;
          return Expanded(
            child: GestureDetector(
              onTap: () => ref.read(browserTabProvider.notifier).state = tab.$1,
              child: Container(
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  border: Border(
                    bottom: BorderSide(
                      color: isActive ? AppColors.accent : Colors.transparent,
                      width: 1.5,
                    ),
                  ),
                ),
                child: Text(
                  tab.$2,
                  style: TextStyle(
                    color: isActive ? cs.onSurface : cs.onSurfaceVariant,
                    fontSize: 8,
                    fontWeight: isActive ? FontWeight.w600 : FontWeight.w400,
                  ),
                ),
              ),
            ),
          );
        }).toList(),
      ),
    );
  }
}

class _BrowserContent extends ConsumerWidget {
  const _BrowserContent();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final tab = ref.watch(browserTabProvider);

    return Container(
      color: Colors.black.withAlpha(26),
      child: Column(
        children: [
          _BrowserSearchBar(),
          Expanded(child: _BrowserTree(tab: tab)),
        ],
      ),
    );
  }
}

class _BrowserSearchBar extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      height: 24,
      padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
      child: TextField(
        style: TextStyle(color: cs.onSurface, fontSize: 10),
        decoration: InputDecoration(
          isDense: true,
          hintText: 'browser.searchHint'.tr(),
          hintStyle: TextStyle(color: cs.onSurfaceVariant.withAlpha(128), fontSize: 10),
          prefixIcon: Icon(Icons.search_rounded, size: 12, color: cs.onSurfaceVariant),
          filled: true,
          fillColor: Colors.black.withAlpha(51),
          contentPadding: const EdgeInsets.symmetric(horizontal: 6, vertical: 4),
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(3),
            borderSide: BorderSide.none,
          ),
        ),
      ),
    );
  }
}

class _BrowserTree extends ConsumerWidget {
  final BrowserTab tab;
  const _BrowserTree({required this.tab});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final placeholderItems = switch (tab) {
      BrowserTab.samples => [
          _PlaceholderItem('Kick', Icons.audiotrack_rounded),
          _PlaceholderItem('Snare', Icons.audiotrack_rounded),
          _PlaceholderItem('Hi-Hat', Icons.audiotrack_rounded),
          _PlaceholderItem('Bass', Icons.audiotrack_rounded),
          _PlaceholderItem('Synth Pad', Icons.audiotrack_rounded),
        ],
      BrowserTab.presets => [
          _PlaceholderItem('Default.zap', Icons.description_rounded),
          _PlaceholderItem('Lead 1', Icons.piano_rounded),
          _PlaceholderItem('Pad 1', Icons.piano_rounded),
        ],
      BrowserTab.projects => [
          _PlaceholderItem('My Song.zap', Icons.folder_rounded),
          _PlaceholderItem('Beat Idea.zap', Icons.folder_rounded),
        ],
    };

    return ListView.builder(
      itemCount: placeholderItems.length,
      itemBuilder: (context, index) {
        final item = placeholderItems[index];
        return _BrowserListItem(
          icon: item.icon,
          label: item.label,
          isDirectory: tab == BrowserTab.projects && index == 0,
        );
      },
    );
  }
}

class _PlaceholderItem {
  final String label;
  final IconData icon;
  const _PlaceholderItem(this.label, this.icon);
}

class _BrowserListItem extends StatefulWidget {
  final IconData icon;
  final String label;
  final bool isDirectory;

  const _BrowserListItem({
    required this.icon,
    required this.label,
    this.isDirectory = false,
  });

  @override
  State<_BrowserListItem> createState() => _BrowserListItemState();
}

class _BrowserListItemState extends State<_BrowserListItem> {
  bool _isHovered = false;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _isHovered = true),
        onExit: (_) => setState(() => _isHovered = false),
        child: Material(
          color: _isHovered ? cs.primary.withAlpha(15) : Colors.transparent,
          child: InkWell(
            onTap: () {},
            child: Container(
              height: 24,
              padding: const EdgeInsets.only(left: 8),
              decoration: BoxDecoration(
                border: widget.isDirectory
                    ? null
                    : Border(
                        bottom: BorderSide(
                          color: Theme.of(context).dividerColor.withAlpha(38),
                          width: 0.5,
                        ),
                      ),
              ),
              child: Row(
                children: [
                  Icon(
                    widget.isDirectory ? Icons.folder_rounded : widget.icon,
                    size: 12,
                    color: widget.isDirectory ? AppColors.neonYellow : cs.primary,
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      widget.label,
                      style: TextStyle(color: cs.onSurface, fontSize: 10),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

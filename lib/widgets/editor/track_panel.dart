import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../../core/constants/app_constants.dart';
import '../../core/utils/theme_colors.dart';
import '../../providers/project_provider.dart';
import '../../core/utils/responsive_utils.dart';
import 'track_tile.dart';

class TrackPanel extends ConsumerWidget {
  final ScrollController? scrollController;

  const TrackPanel({super.key, this.scrollController});

  /// Width of the channel rack for the given screen size. Shared with the
  /// timeline ruler spacer so the ruler starts where the lanes start.
  static double widthFor(ScreenSize screenSize) => switch (screenSize) {
        ScreenSize.mobile => 180.0,
        ScreenSize.tablet => 200.0,
        ScreenSize.desktop => AppConstants.trackPanelWidth,
      };

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final project = ref.watch(projectProvider);
    final screenSize = getScreenSize(context);
    final panelWidth = widthFor(screenSize);

    return Container(
      width: panelWidth,
      decoration: BoxDecoration(
        color: context.surfaceHigh,
        border: Border(
          right: BorderSide(color: Theme.of(context).dividerColor, width: 0.5),
        ),
      ),
      child: Column(
        children: [
          Expanded(
            child: project.tracks.isEmpty
                ? Center(
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        Icon(
                          Icons.music_note_outlined,
                          size: 32,
                          color: context.outline.withAlpha(128),
                        ),
                        const SizedBox(height: 8),
                        Text(
                          'track.empty'.tr(),
                          style: TextStyle(color: context.outline, fontSize: 12),
                        ),
                        const SizedBox(height: 4),
                        Text(
                          'track.emptyHint'.tr(),
                          style: TextStyle(
                            color: context.outline.withAlpha(153),
                            fontSize: 10,
                          ),
                        ),
                      ],
                    ),
                  )
                : ListView.builder(
                    controller: scrollController,
                    itemCount: project.tracks.length,
                    itemExtent: AppConstants.trackTileHeight,
                    itemBuilder: (context, index) => TrackTile(
                      track: project.tracks[index],
                      index: index,
                    ),
                  ),
          ),
        ],
      ),
    );
  }
}

/// Header strip for the channel rack. Rendered in the timeline-ruler band
/// (left of the ruler) so the channel list below starts exactly where the
/// track lanes start.
class ChannelRackHeader extends ConsumerWidget {
  const ChannelRackHeader({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final project = ref.watch(projectProvider);
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
          const SizedBox(width: 8),
          Icon(Icons.grid_view_rounded, size: 12, color: cs.onSurfaceVariant),
          const SizedBox(width: 4),
          Text(
            'CHANNELS',
            style: TextStyle(
              color: cs.onSurfaceVariant,
              fontSize: 9,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.2,
            ),
          ),
          const Spacer(),
          Text(
            '${project.tracks.length}',
            style: TextStyle(
              color: context.outline,
              fontSize: 9,
            ),
          ),
          const SizedBox(width: 4),
        ],
      ),
    );
  }
}

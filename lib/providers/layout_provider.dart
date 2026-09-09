import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../core/constants/app_constants.dart';

/// Width of the left channel rack (desktop). Drag the divider between the
/// rack and the track lanes to resize.
final trackPanelWidthProvider =
    StateProvider<double>((ref) => AppConstants.trackPanelWidth);

/// Height of the expanded mixer strip area. Drag the divider above the mixer
/// header to resize.
final mixerHeightProvider =
    StateProvider<double>((ref) => AppConstants.mixerPanelHeight);

/// Mixer channel-strip widgets.
///
/// These render the [`MixerChannel`] model. They hold no audio state and never
/// touch the engine: a gesture mutates the model through a callback, and
/// whatever owns the model decides what to do with the change. That separation
/// is what lets the same strip be driven by the legacy path or by the Rust
/// engine without the widget knowing which.
///
/// The fader is deliberately a **dB** control, not a `0..1` slider
/// (PLAN §3.S3 item 6). Its travel is mapped through a curve so the useful
/// range around unity gets most of the space, which is what makes a fader
/// usable — a linear `0..1` fader spends most of its travel on inaudible
/// differences.
library;

import 'package:flutter/material.dart';

import '../../core/constants/app_constants.dart';
import '../../mixer/mixer_model.dart';
import '../layout/rotary_knob.dart';
import 'mixer_meter.dart';

/// Neutral colours for the mixer, kept local so the strips do not depend on
/// whichever palette the surrounding editor uses.
abstract final class MixerColors {
  /// Background of the strip area.
  static const Color background = Color(0xFF141416);

  /// Background of the master strip.
  static const Color master = Color(0xFF1B1B1F);

  /// An engaged mute button.
  static const Color mute = Color(0xFFD35400);

  /// An engaged solo button.
  static const Color solo = Color(0xFF2E86C1);

  /// A disengaged button.
  static const Color inactive = Color(0xFF3A3A40);
}

/// Width of one channel strip.
const double kStripWidth = 68.0;

/// Options controlling how a strip renders.
class StripOptions {
  /// Creates strip options.
  const StripOptions({
    this.isPlaybackActive = false,
    this.showSends = false,
    this.soloActiveElsewhere = false,
  });

  /// Whether transport is running, which gates meter animation.
  final bool isPlaybackActive;

  /// Whether the send section is expanded.
  final bool showSends;

  /// Whether some other channel is soloed, used to dim the excluded ones.
  final bool soloActiveElsewhere;
}

/// A single mixer channel strip.
class MixerChannelStrip extends StatelessWidget {
  /// Creates a channel strip.
  const MixerChannelStrip({
    super.key,
    required this.channel,
    required this.onChanged,
    this.options = const StripOptions(),
    this.levels,
    this.accentColor,
    this.activeEffectCount = 0,
  });

  /// The channel to render.
  final MixerChannel channel;

  /// Called with the updated channel after any gesture.
  final ValueChanged<MixerChannel> onChanged;

  /// Rendering options.
  final StripOptions options;

  /// Live meter reading, or `null` when no engine is reporting.
  final MeterReading? levels;

  /// Accent colour, usually inherited from the owning track.
  final Color? accentColor;

  /// How many effect slots are loaded, shown as a badge.
  final int activeEffectCount;

  /// The colour this strip accents with.
  Color get _accent => accentColor ?? AppColors.neonGreen;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final dimmed = options.soloActiveElsewhere && !channel.isSolo;

    return Container(
      width: kStripWidth,
      decoration: BoxDecoration(
        color: channel.isMaster ? MixerColors.master : Colors.transparent,
        border: Border(
          right: BorderSide(
            color: Theme.of(context).dividerColor.withAlpha(60),
            width: 0.5,
          ),
        ),
      ),
      child: Opacity(
        opacity: dimmed ? 0.45 : 1.0,
        child: Column(
          children: [
            _StripHeader(channel: channel, accent: _accent),
            // Meter fills the available vertical space.
            Expanded(
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                child: MixerLevelMeter(
                  reading: levels,
                  isActive: options.isPlaybackActive && !channel.isMuted,
                  accent: _accent,
                ),
              ),
            ),
            _StripControls(
              channel: channel,
              onChanged: onChanged,
              accent: _accent,
            ),
            _StripButtons(
              channel: channel,
              onChanged: onChanged,
              activeEffectCount: activeEffectCount,
            ),
            _Fader(
              channel: channel,
              onChanged: onChanged,
              accent: _accent,
            ),
            const SizedBox(height: 3),
          ],
        ),
      ),
    );
  }
}

/// The name plate at the top of a strip.
class _StripHeader extends StatelessWidget {
  const _StripHeader({required this.channel, required this.accent});

  final MixerChannel channel;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      height: 20,
      padding: const EdgeInsets.symmetric(horizontal: 4),
      color: channel.isMaster
          ? Colors.black.withAlpha(80)
          : accent.withAlpha(36),
      child: Row(
        children: [
          Container(
            width: 3,
            height: 12,
            decoration: BoxDecoration(
              color: channel.isMaster ? AppColors.neonOrange : accent,
              borderRadius: BorderRadius.circular(1),
            ),
          ),
          const SizedBox(width: 3),
          Expanded(
            child: Text(
              channel.isMaster ? 'MASTER' : channel.name,
              style: TextStyle(
                color: cs.onSurface,
                fontSize: 8,
                fontWeight: channel.isMaster ? FontWeight.w700 : FontWeight.w500,
                letterSpacing: channel.isMaster ? 0.5 : 0,
              ),
              overflow: TextOverflow.ellipsis,
              maxLines: 1,
            ),
          ),
        ],
      ),
    );
  }
}

/// Pan knob and gain readout.
class _StripControls extends StatelessWidget {
  const _StripControls({
    required this.channel,
    required this.onChanged,
    required this.accent,
  });

  final MixerChannel channel;
  final ValueChanged<MixerChannel> onChanged;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return SizedBox(
      height: 34,
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          SizedBox(
            width: 22,
            height: 22,
            child: RotaryKnob(
              // The knob is a 0..1 control; pan is -1..1 internally.
              value: (channel.pan + 1) / 2,
              min: 0,
              max: 1,
              size: 20,
              activeColor: channel.isMaster ? AppColors.neonOrange : accent,
              onChanged: (v) => onChanged(channel.copyWith(pan: (v * 2) - 1)),
            ),
          ),
          const SizedBox(width: 4),
          // The numeric readout is what makes a dB fader legible; without it
          // the user cannot tell -3 dB from -6 dB.
          SizedBox(
            width: 30,
            child: Text(
              formatGainDb(channel.gainDb),
              textAlign: TextAlign.center,
              style: TextStyle(
                color: cs.onSurfaceVariant,
                fontSize: 8,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Mute, solo, phase and effect-count buttons.
class _StripButtons extends StatelessWidget {
  const _StripButtons({
    required this.channel,
    required this.onChanged,
    required this.activeEffectCount,
  });

  final MixerChannel channel;
  final ValueChanged<MixerChannel> onChanged;
  final int activeEffectCount;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return SizedBox(
      height: 18,
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          _MiniButton(
            label: 'M',
            engaged: channel.isMuted,
            engagedColor: MixerColors.mute,
            tooltip: '静音',
            onTap: () => onChanged(channel.copyWith(isMuted: !channel.isMuted)),
          ),
          const SizedBox(width: 2),
          _MiniButton(
            label: 'S',
            engaged: channel.isSolo,
            engagedColor: MixerColors.solo,
            tooltip: '独奏',
            onTap: () => onChanged(channel.copyWith(isSolo: !channel.isSolo)),
          ),
          const SizedBox(width: 2),
          _MiniButton(
            label: 'Ø',
            engaged: channel.phaseInvert,
            engagedColor: cs.error,
            tooltip: '反相',
            onTap: () => onChanged(
              channel.copyWith(phaseInvert: !channel.phaseInvert),
            ),
          ),
          const SizedBox(width: 2),
          _EffectBadge(count: activeEffectCount),
        ],
      ),
    );
  }
}

/// A small toggle used for mute/solo/phase.
class _MiniButton extends StatelessWidget {
  const _MiniButton({
    required this.label,
    required this.engaged,
    required this.engagedColor,
    required this.onTap,
    this.tooltip,
  });

  final String label;
  final bool engaged;
  final Color engagedColor;
  final VoidCallback onTap;
  final String? tooltip;

  @override
  Widget build(BuildContext context) {
    final button = GestureDetector(
      onTap: onTap,
      child: Container(
        width: 14,
        height: 14,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: engaged ? engagedColor : MixerColors.inactive,
          borderRadius: BorderRadius.circular(2),
        ),
        child: Text(
          label,
          style: TextStyle(
            color: engaged ? Colors.white : Colors.white70,
            fontSize: 8,
            fontWeight: FontWeight.w700,
          ),
        ),
      ),
    );

    if (tooltip == null) return button;
    return Tooltip(message: tooltip!, child: button);
  }
}

/// Shows how many effects are loaded on a channel.
class _EffectBadge extends StatelessWidget {
  const _EffectBadge({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      width: 14,
      height: 14,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: count > 0 ? cs.primary.withAlpha(180) : MixerColors.inactive,
        borderRadius: BorderRadius.circular(2),
      ),
      child: Text(
        '$count',
        style: TextStyle(
          color: count > 0 ? Colors.white : Colors.white70,
          fontSize: 8,
          fontWeight: FontWeight.w700,
        ),
      ),
    );
  }
}

/// The fader: a vertical dB control.
class _Fader extends StatelessWidget {
  const _Fader({
    required this.channel,
    required this.onChanged,
    required this.accent,
  });

  final MixerChannel channel;
  final ValueChanged<MixerChannel> onChanged;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return SizedBox(
      height: 56,
      child: RotatedBox(
        quarterTurns: -1,
        child: SliderTheme(
          data: SliderTheme.of(context).copyWith(
            trackHeight: 3,
            thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
            overlayShape: const RoundSliderOverlayShape(overlayRadius: 9),
            activeTrackColor: channel.isMaster ? AppColors.neonOrange : accent,
            inactiveTrackColor: cs.surfaceContainerHighest,
            thumbColor: cs.onSurface,
          ),
          child: Slider(
            value: channel.gainDb.clamp(kMinGainDb, kMaxGainDb),
            min: kMinGainDb,
            max: kMaxGainDb,
            // The slider is linear in dB, so the travel is already the fader
            // curve; no divisions are set because dB steps are far finer than
            // the pixel grid.
            onChanged: (v) => onChanged(channel.copyWith(gainDb: clampDb(v))),
          ),
        ),
      ),
    );
  }
}

/// Formats a fader position for display.
///
/// Shows `-∞` at the floor, because that is what the user is hearing: the
/// engine maps the bottom of the travel to exact silence, and displaying
/// `-96.0 dB` would suggest a very quiet but audible signal.
String formatGainDb(double db) {
  if (db <= kMinGainDb) return '-∞';
  final sign = db > 0 ? '+' : '';
  return '$sign${db.toStringAsFixed(1)}';
}

/// A row of sends for one channel.
class MixerSendRow extends StatelessWidget {
  /// Creates a send row.
  const MixerSendRow({
    super.key,
    required this.channel,
    required this.onChanged,
    this.destinations = const [],
  });

  /// The channel whose sends are shown.
  final MixerChannel channel;

  /// Called with the updated channel.
  final ValueChanged<MixerChannel> onChanged;

  /// Selectable destination channel ids.
  final List<String> destinations;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (var i = 0; i < kMaxSendsPerChannel; i++)
          _SendRow(
            index: i,
            send: i < channel.sends.length ? channel.sends[i] : const MixerSend(),
            destinations: destinations,
            onChanged: (updated) {
              final sends = List<MixerSend>.from(channel.sends);
              // Pad so slot `i` is addressable even before it was configured.
              while (sends.length <= i) {
                sends.add(const MixerSend());
              }
              sends[i] = updated;
              onChanged(channel.copyWith(sends: sends));
            },
          ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
          child: Text(
            '${channel.sends.where((s) => s.isActive).length} 路发送',
            style: TextStyle(color: cs.onSurfaceVariant, fontSize: 8),
          ),
        ),
      ],
    );
  }
}

/// One send slot's controls.
class _SendRow extends StatelessWidget {
  const _SendRow({
    required this.index,
    required this.send,
    required this.onChanged,
    required this.destinations,
  });

  final int index;
  final MixerSend send;
  final ValueChanged<MixerSend> onChanged;
  final List<String> destinations;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 1),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => onChanged(send.copyWith(enabled: !send.enabled)),
            child: Container(
              width: 12,
              height: 12,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: send.enabled ? cs.primary : MixerColors.inactive,
                borderRadius: BorderRadius.circular(2),
              ),
              child: Text(
                '${index + 1}',
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 7,
                  fontWeight: FontWeight.w700,
                ),
              ),
            ),
          ),
          const SizedBox(width: 3),
          // Pre/post is a per-send choice, so it gets its own control rather
          // than being a global channel setting.
          GestureDetector(
            onTap: () => onChanged(
              send.copyWith(
                tap: send.tap == SendTap.preFader
                    ? SendTap.postFader
                    : SendTap.preFader,
              ),
            ),
            child: Text(
              send.tap == SendTap.preFader ? 'PRE' : 'POST',
              style: TextStyle(
                color: send.tap == SendTap.preFader
                    ? AppColors.neonYellow
                    : cs.onSurfaceVariant,
                fontSize: 7,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          const SizedBox(width: 3),
          Expanded(
            child: Text(
              formatGainDb(send.levelDb),
              textAlign: TextAlign.right,
              style: TextStyle(color: cs.onSurfaceVariant, fontSize: 7),
            ),
          ),
        ],
      ),
    );
  }
}

/// The whole strip area: every channel plus master.
class MixerStripArea extends StatelessWidget {
  /// Creates the strip area.
  const MixerStripArea({
    super.key,
    required this.state,
    required this.onChanged,
    this.levels = const {},
    this.accentColors = const {},
    this.height = 160,
    this.isPlaybackActive = false,
  });

  /// The mixer state to render.
  final MixerState state;

  /// Called with an updated channel, identified by id.
  final void Function(String channelId, MixerChannel updated) onChanged;

  /// Live meter readings keyed by channel id.
  final Map<String, MeterReading> levels;

  /// Accent colours keyed by channel id.
  final Map<String, Color> accentColors;

  /// Height of the strip area.
  final double height;

  /// Whether transport is running.
  final bool isPlaybackActive;

  @override
  Widget build(BuildContext context) {
    // Master renders last, mirroring a console where the master sits to the
    // right of the channels it sums.
    final channels = state.channels.where((c) => !c.isMaster).toList();
    final master = state.master;

    return Container(
      height: height,
      decoration: const BoxDecoration(color: MixerColors.background),
      child: Row(
        children: [
          Expanded(
            child: ListView.builder(
              scrollDirection: Axis.horizontal,
              itemCount: channels.length + 1,
              itemExtent: kStripWidth,
              itemBuilder: (context, index) {
                if (index == channels.length) {
                  return MixerChannelStrip(
                    channel: master,
                    levels: levels[master.id],
                    activeEffectCount:
                        master.effects.where((e) => e.isProcessing).length,
                    options: StripOptions(isPlaybackActive: isPlaybackActive),
                    onChanged: (updated) => onChanged(master.id, updated),
                  );
                }
                final channel = channels[index];
                return MixerChannelStrip(
                  channel: channel,
                  levels: levels[channel.id],
                  accentColor: accentColors[channel.id],
                  activeEffectCount:
                      channel.effects.where((e) => e.isProcessing).length,
                  options: StripOptions(
                    isPlaybackActive: isPlaybackActive,
                    soloActiveElsewhere: state.hasSolo && !channel.isSolo,
                  ),
                  onChanged: (updated) => onChanged(channel.id, updated),
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

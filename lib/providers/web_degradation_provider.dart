import 'package:flutter_riverpod/flutter_riverpod.dart';

/// Web (and cross-platform) audio degradation policy (PLAN §3.S1.5).
///
/// ## What problem this solves
///
/// On a weak machine — most often a browser tab — the DSP graph can stop
/// meeting its deadline. The audio thread then underruns ("xruns"), which the
/// user hears as clicks and dropouts. The plan's answer is a **three-tier
/// automatic degradation**: shed the most expensive work first, tell the user
/// plainly what was shed, and never silently pretend nothing happened.
///
/// ## The two rules that shape the design
///
/// 1. **Detection never runs in the audio callback.** The Rust audio thread only
///    increments counters; Dart polls a lock-free snapshot and decides here.
///    This file is that decision, and it is pure so it can be tested without an
///    engine or a timer.
/// 2. **A warning never auto-dismisses.** If the app quietly re-enabled the
///    expensive path, the user would conclude "it fixed itself" and then be
///    surprised by the next dropout. A warning is cleared only by an explicit
///    user action.
///
/// ## Reversibility
///
/// Degradation is reversible: after a sustained window at a healthy load the
/// policy reports that recovery is *available*. It is not applied automatically
/// because a level flapping up and down is worse than staying one level low, so
/// recovery also waits for the user.

/// The three degradation tiers, in the order the plan defines them.
enum DegradationLevel {
  /// L0 — everything is available.
  full,

  /// L1 — the expensive, optional work is disabled.
  reduced,

  /// L2 — only core playback remains.
  minimal,
}

extension DegradationLevelX on DegradationLevel {
  /// The ABI/tier code (`0`, `1`, `2`), matching `ZenithEngineStatus.degradeLevel`.
  int get code => index;

  /// Decodes the code coming back from the engine snapshot.
  static DegradationLevel fromCode(int code) => switch (code) {
        1 => DegradationLevel.reduced,
        2 => DegradationLevel.minimal,
        _ => DegradationLevel.full,
      };
}

/// A capability that degradation can turn off.
///
/// Named individually rather than as a level so the UI can grey out *specific*
/// controls with a reason, which is what the plan asks for ("被停用的功能在界面上
/// 置灰并附原因提示").
enum DegradedFeature {
  /// Convolution reverb (the most expensive reverb).
  convolutionReverb,

  /// Oversampled distortion (4x/8x).
  oversampledDistortion,

  /// High-ratio real-time time stretch.
  highRatioTimeStretch,

  /// All send/return buses.
  sendBuses,

  /// The real-time effect chain (baked offline instead).
  realtimeEffects,

  /// Polyphony above the core limit.
  extendedPolyphony,
}

/// Which features each level disables.
const Map<DegradationLevel, Set<DegradedFeature>> kDisabledFeaturesByLevel = {
  DegradationLevel.full: <DegradedFeature>{},
  DegradationLevel.reduced: <DegradedFeature>{
    DegradedFeature.convolutionReverb,
    DegradedFeature.oversampledDistortion,
    DegradedFeature.highRatioTimeStretch,
  },
  DegradationLevel.minimal: <DegradedFeature>{
    DegradedFeature.convolutionReverb,
    DegradedFeature.oversampledDistortion,
    DegradedFeature.highRatioTimeStretch,
    DegradedFeature.sendBuses,
    DegradedFeature.realtimeEffects,
    DegradedFeature.extendedPolyphony,
  },
};

/// The CPU load thresholds, as fractions of real time.
abstract final class DegradationThresholds {
  /// Below this, with no xruns, the engine is considered healthy.
  static const double healthyLoad = 0.60;

  /// Above this, or with sustained xruns, drop to L2.
  static const double overloadedLoad = 0.85;

  /// How many consecutive degraded polls before the warning is raised.
  ///
  /// A single spike must not degrade a session; a sustained one must.
  static const int confirmPolls = 3;

  /// How many consecutive healthy polls before recovery is *offered*.
  ///
  /// At the plan's ~1 Hz web poll this is ten seconds of stability.
  static const int recoveryPolls = 10;

  /// The polyphony ceiling at L2 (PLAN §3.S1.5).
  static const int minimalPolyphony = 32;
}

/// The result of one evaluation, kept pure so tests can drive it directly.
class DegradationDecision {
  /// The level the policy would settle on.
  final DegradationLevel level;

  /// Whether the warning should be shown (sticky once true).
  final bool warningActive;

  /// Whether healthy operation has been sustained long enough to offer recovery.
  final bool recoveryAvailable;

  const DegradationDecision({
    required this.level,
    required this.warningActive,
    required this.recoveryAvailable,
  });
}

/// Pure degradation state machine.
///
/// Holds only counters and the current level; no timers, no engine, so it is
/// safe to unit-test by feeding a sequence of loads and xrun counts.
class DegradationPolicy {
  DegradationLevel _level;
  bool _warningActive;
  int _degradedPolls;
  int _healthyPolls;
  int _lastXrunCount;
  bool _userForced;

  DegradationPolicy({DegradationLevel level = DegradationLevel.full})
      : _level = level,
        _warningActive = level != DegradationLevel.full,
        _degradedPolls = 0,
        _healthyPolls = 0,
        _lastXrunCount = 0,
        _userForced = false;

  /// The current level.
  DegradationLevel get level => _level;

  /// Whether the warning is currently raised.
  bool get warningActive => _warningActive;

  /// Whether the user has overridden the policy for the session.
  bool get userForced => _userForced;

  /// Whether recovery is on offer.
  bool get recoveryAvailable =>
      _level != DegradationLevel.full &&
      _healthyPolls >= DegradationThresholds.recoveryPolls;

  /// Feeds one poll of the engine's status.
  ///
  /// [cpuLoad] is the real-time fraction (`0..1`); [xrunCount] is the
  /// monotonic counter from the snapshot, so an *increase* since the last poll
  /// means a new underrun happened in this interval.
  DegradationDecision evaluate({
    required double cpuLoad,
    required int xrunCount,
    bool isWeb = false,
  }) {
    final newXrun = xrunCount > _lastXrunCount;
    _lastXrunCount = xrunCount;

    final healthy = cpuLoad < DegradationThresholds.healthyLoad && !newXrun;
    final overloaded = cpuLoad > DegradationThresholds.overloadedLoad;

    if (healthy) {
      _healthyPolls++;
      _degradedPolls = 0;
    } else {
      _degradedPolls++;
      _healthyPolls = 0;
    }

    // Only degrade on a *sustained* condition. Web tabs are jittery; reacting to
    // one bad poll would have the app shedding features constantly.
    //
    // A session the user explicitly overrode is not degraded again: the whole
    // point of "still enable" is that the app stops fighting the user. The
    // warning stays up, so the state is honest, but the policy holds still.
    if (_degradedPolls >= DegradationThresholds.confirmPolls && !_userForced) {
      final target = overloaded || (newXrun && _level == DegradationLevel.reduced)
          ? DegradationLevel.minimal
          : DegradationLevel.reduced;
      if (target != DegradationLevel.full && target.index > _level.index) {
        _level = target;
        // A warning is raised the first time we shed anything and never
        // auto-cleared.
        _warningActive = true;
      }
    }

    return DegradationDecision(
      level: _level,
      warningActive: _warningActive,
      recoveryAvailable: recoveryAvailable,
    );
  }

  /// Applies a user-confirmed recovery by one tier.
  ///
  /// Returns the new level. Moving to [DegradationLevel.full] clears the
  /// warning, because at that point the user has explicitly restored everything.
  DegradationLevel recover() {
    if (_level.index > 0) {
      _level = DegradationLevel.values[_level.index - 1];
    }
    _healthyPolls = 0;
    if (_level == DegradationLevel.full) {
      _warningActive = false;
      _userForced = false;
    }
    return _level;
  }

  /// The user chose "still enable" — keep the warning but stop shedding.
  void forceFull() {
    _level = DegradationLevel.full;
    _userForced = true;
    // The warning deliberately stays: the plan requires it to remain visible so
    // the user knows they are running past the safe point.
    _warningActive = true;
    _healthyPolls = 0;
  }
}

/// The degradation state exposed to the UI.
class WebDegradationState {
  /// The active tier.
  final DegradationLevel level;

  /// Whether the warning bar is showing.
  final bool warningActive;

  /// Whether the app may offer to restore a tier.
  final bool recoveryAvailable;

  /// Whether the user has overridden the policy for the current session.
  final bool userForced;

  const WebDegradationState({
    this.level = DegradationLevel.full,
    this.warningActive = false,
    this.recoveryAvailable = false,
    this.userForced = false,
  });

  /// The features currently switched off.
  Set<DegradedFeature> get disabledFeatures =>
      kDisabledFeaturesByLevel[level] ?? const {};

  /// Whether [feature] is currently unavailable.
  bool isDisabled(DegradedFeature feature) => disabledFeatures.contains(feature);

  WebDegradationState copyWith({
    DegradationLevel? level,
    bool? warningActive,
    bool? recoveryAvailable,
    bool? userForced,
  }) =>
      WebDegradationState(
        level: level ?? this.level,
        warningActive: warningActive ?? this.warningActive,
        recoveryAvailable: recoveryAvailable ?? this.recoveryAvailable,
        userForced: userForced ?? this.userForced,
      );
}

/// Exposes the degradation policy as Riverpod state.
///
/// The notifier owns the policy and the polling is driven by whoever has the
/// engine status — the web bridge, or a local poll on desktop. This file does
/// not start a timer itself, so it stays unit-testable and does not assume a
/// transport exists.
class WebDegradationNotifier extends Notifier<WebDegradationState> {
  late DegradationPolicy _policy;

  @override
  WebDegradationState build() {
    _policy = DegradationPolicy();
    return const WebDegradationState();
  }

  /// Feeds one status poll into the policy; returns the resulting state.
  WebDegradationState updateFromStatus({
    required double cpuLoad,
    required int xrunCount,
    bool isWeb = false,
  }) {
    final decision = _policy.evaluate(
      cpuLoad: cpuLoad,
      xrunCount: xrunCount,
      isWeb: isWeb,
    );
    state = state.copyWith(
      level: decision.level,
      warningActive: decision.warningActive,
      recoveryAvailable: decision.recoveryAvailable,
    );
    return state;
  }

  /// Restores one tier, on user request.
  void recover() {
    final level = _policy.recover();
    state = state.copyWith(
      level: level,
      warningActive: _policy.warningActive,
      recoveryAvailable: _policy.recoveryAvailable,
      userForced: false,
    );
  }

  /// The user chose "still enable": restore everything but keep the warning.
  void enableAll() {
    _policy.forceFull();
    state = state.copyWith(
      level: DegradationLevel.full,
      warningActive: true,
      recoveryAvailable: false,
      userForced: true,
    );
  }
}

/// The application's degradation state.
final webDegradationProvider =
    NotifierProvider<WebDegradationNotifier, WebDegradationState>(
  WebDegradationNotifier.new,
);

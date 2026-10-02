import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../models/project.dart';
import 'project_provider.dart';

/// Which side of an A/B comparison is currently live.
enum AbSide { a, b }

/// The A/B comparison state (PLAN §3.S9 item 1).
///
/// ## What A/B is for
///
/// A mix decision ("is this compressor better before or after the EQ?") is only
/// answerable by listening to both options back to back. A/B holds two full
/// project snapshots and swaps between them, so the user edits each side and
/// compares without saving and reopening.
///
/// ## Why immutable snapshots
///
/// `Project` is immutable (see `models/project.dart`), so a snapshot is just a
/// reference; there is no deep-copy step to get wrong. A switch is therefore one
/// state assignment, which is what makes it fast enough to use as a listening
/// tool.
class AbState {
  /// Whether comparison mode is armed.
  final bool active;

  /// Which side is live.
  final AbSide side;

  /// Snapshot of side A.
  final Project? a;

  /// Snapshot of side B.
  final Project? b;

  const AbState({
    this.active = false,
    this.side = AbSide.a,
    this.a,
    this.b,
  });

  /// Whether both sides hold a snapshot.
  bool get hasBothSides => a != null && b != null;

  AbState copyWith({
    bool? active,
    AbSide? side,
    Project? a,
    Project? b,
  }) =>
      AbState(
        active: active ?? this.active,
        side: side ?? this.side,
        a: a ?? this.a,
        b: b ?? this.b,
      );
}

/// The pure A/B switching logic.
///
/// Kept free of Riverpod so it can be unit-tested without a provider container:
/// the project is read and written through the injected [read] / [write]
/// callbacks. The Riverpod layer below is a thin adapter over this.
///
/// This is the same split the degradation policy uses, and for the same reason:
/// the interesting logic (which side is live, what a switch preserves) is a pure
/// function worth testing directly, and the framework plumbing should not be
/// able to break it.
class AbController {
  /// Reads the currently live project.
  final Project Function() read;

  /// Writes the live project.
  final void Function(Project) write;

  AbState _state = const AbState();

  AbController({required this.read, required this.write});

  /// The current comparison state.
  AbState get state => _state;

  /// Arms comparison mode, snapshotting the current project into A and copying
  /// it into B so both sides start identical.
  ///
  /// Starting B as a copy of A is deliberate: the user's first edit defines the
  /// difference, rather than having to build B from scratch.
  void arm() {
    final current = read();
    _state = AbState(active: true, side: AbSide.a, a: current, b: current);
  }

  /// Snapshots the live project into the side it belongs to.
  ///
  /// Call before switching, or after edits, so a switch does not lose work.
  void capture() {
    if (!_state.active) return;
    final current = read();
    _state = _state.side == AbSide.a
        ? _state.copyWith(a: current)
        : _state.copyWith(b: current);
  }

  /// Switches to [side], capturing the current side first and restoring the
  /// target.
  void switchTo(AbSide side) {
    if (!_state.active || side == _state.side) return;
    capture();
    final target = side == AbSide.a ? _state.a : _state.b;
    if (target == null) return;
    write(target);
    _state = _state.copyWith(side: side);
  }

  /// Flips to the other side. This is the gesture a keyboard shortcut binds to.
  void toggle() => switchTo(_state.side == AbSide.a ? AbSide.b : AbSide.a);

  /// Copies the live side onto the other side, discarding that side's edits.
  ///
  /// "Make B like A", which is how a user commits a decision after comparing.
  void copyLiveToOther() {
    if (!_state.active) return;
    capture();
    final live = _state.side == AbSide.a ? _state.a : _state.b;
    if (live == null) return;
    _state = _state.side == AbSide.a
        ? _state.copyWith(b: live)
        : _state.copyWith(a: live);
  }

  /// Leaves comparison mode, keeping the live side as the project.
  void disarm() {
    capture();
    _state = _state.copyWith(active: false);
  }
}

/// The Riverpod-facing A/B controller.
///
/// Wraps [AbController] and binds it to `projectProvider`.
class AbNotifier extends Notifier<AbState> {
  late AbController _controller;

  @override
  AbState build() {
    _controller = AbController(
      read: () => ref.read(projectProvider),
      write: (project) => ref.read(projectProvider.notifier).state = project,
    );
    return _controller.state;
  }

  /// Arms comparison mode, snapshotting the current project into both sides.
  void arm() {
    _controller.arm();
    state = _controller.state;
  }

  /// Snapshots the live project into its side.
  void capture() {
    _controller.capture();
    state = _controller.state;
  }

  /// Switches to [side].
  void switchTo(AbSide side) {
    _controller.switchTo(side);
    state = _controller.state;
  }

  /// Flips to the other side.
  void toggle() {
    _controller.toggle();
    state = _controller.state;
  }

  /// Copies the live side onto the other side.
  void copyLiveToOther() {
    _controller.copyLiveToOther();
    state = _controller.state;
  }

  /// Leaves comparison mode.
  void disarm() {
    _controller.disarm();
    state = _controller.state;
  }
}

/// The A/B comparison state.
final abCompareProvider =
    NotifierProvider<AbNotifier, AbState>(AbNotifier.new);

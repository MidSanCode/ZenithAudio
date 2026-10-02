import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/project.dart';
import 'package:zenith_audio/providers/ab_compare_provider.dart';

/// S9: the pure A/B switching logic, exercised without Riverpod.
void main() {
  late Project live;
  late AbController ab;

  setUp(() {
    live = const Project(id: 'x', name: 'Base');
    ab = AbController(read: () => live, write: (p) => live = p);
  });

  test('arm snapshots the current project into both sides', () {
    ab.arm();
    expect(ab.state.active, isTrue);
    expect(ab.state.side, AbSide.a);
    expect(ab.state.a!.name, 'Base');
    expect(ab.state.b!.name, 'Base');
    expect(ab.state.hasBothSides, isTrue);
  });

  test('switching captures the live side and restores the target', () {
    ab.arm();
    live = const Project(id: 'x', name: 'Edited A');
    ab.switchTo(AbSide.b);

    expect(live.name, 'Base', reason: 'B is still the original snapshot');
    expect(ab.state.a!.name, 'Edited A', reason: 'A captured the edit');

    live = const Project(id: 'x', name: 'Edited B');
    ab.switchTo(AbSide.a);
    expect(live.name, 'Edited A', reason: 'A restored');
    expect(ab.state.b!.name, 'Edited B', reason: 'B captured');
  });

  test('toggle flips between the two sides', () {
    ab.arm();
    live = const Project(id: 'x', name: 'A2');
    ab.toggle();
    expect(ab.state.side, AbSide.b);
    ab.toggle();
    expect(ab.state.side, AbSide.a);
    expect(live.name, 'A2');
  });

  test('copyLiveToOther overwrites the other side', () {
    ab.arm();
    live = const Project(id: 'x', name: 'Final');
    ab.copyLiveToOther();
    expect(ab.state.b!.name, 'Final', reason: "B takes A's live content");
  });

  test('disarm keeps the live side and stops comparing', () {
    ab.arm();
    live = const Project(id: 'x', name: 'Live');
    ab.disarm();
    expect(ab.state.active, isFalse);
    expect(live.name, 'Live');
  });

  test('switching while inactive is a no-op', () {
    ab.switchTo(AbSide.b);
    expect(ab.state.active, isFalse);
    expect(live.name, 'Base');
  });

  test('capture while inactive does nothing', () {
    live = const Project(id: 'x', name: 'Changed');
    ab.capture();
    expect(ab.state.a, isNull);
    expect(ab.state.b, isNull);
  });
}

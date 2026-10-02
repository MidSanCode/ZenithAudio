import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/playlist.dart';
import 'package:zenith_audio/widgets/editor/arrangement_geometry.dart';

/// S6a: the pure arrangement-view geometry behind the playlist canvas.
void main() {
  const geometry = ArrangementGeometry(
    pixelsPerTick: 0.1,
    laneHeight: 40,
    laneHeaderWidth: 100,
  );

  group('tick <-> x', () {
    test('round-trips at the origin', () {
      expect(geometry.tickToX(0), 100);
      expect(geometry.xToTick(100), 0);
    });

    test('scales by pixels per tick', () {
      expect(geometry.tickToX(100), 110, reason: '100 ticks * 0.1 px');
      expect(geometry.xToTick(110), 100);
    });

    test('clamps a negative tick to zero', () {
      expect(geometry.xToTick(0), 0);
      expect(geometry.xToTick(-50), 0);
    });

    test('honours the tick scroll', () {
      final scrolled = geometry.copyWith(scrollTicks: 500);
      expect(scrolled.tickToX(500), 100, reason: 'scrolled to the origin');
      expect(scrolled.xToTick(100), 500);
    });
  });

  group('lane <-> y', () {
    test('maps lane index to y', () {
      expect(geometry.laneToY(0), 0);
      expect(geometry.laneToY(2), 80);
      expect(geometry.yToLane(85), 2);
    });

    test('clamps a negative y to lane zero', () {
      expect(geometry.yToLane(-10), 0);
    });
  });

  group('item rectangles', () {
    test('a block spans its start to end', () {
      const item = PlaylistItem(
        id: 'i',
        patternId: 'p',
        startTicks: 0,
        lengthTicks: 1000,
      );
      final rect = geometry.itemRect(item);
      expect(rect.left, 100);
      expect(rect.width, 100, reason: '1000 ticks * 0.1 px');
      expect(rect.height, 40);
    });

    test('a zero-length block is still at least one pixel wide', () {
      const item = PlaylistItem(
        id: 'i',
        patternId: 'p',
        startTicks: 100,
        lengthTicks: 0,
      );
      expect(geometry.itemRect(item).width, greaterThanOrEqualTo(1.0));
    });
  });

  group('hit testing', () {
    const a = PlaylistItem(
      id: 'a',
      patternId: 'p',
      startTicks: 0,
      lengthTicks: 500,
      trackIndex: 0,
    );
    const b = PlaylistItem(
      id: 'b',
      patternId: 'p',
      startTicks: 200,
      lengthTicks: 500,
      trackIndex: 0,
    );
    const playlist = Playlist(items: [a, b]);

    test('hits the item under the point', () {
      expect(geometry.hitTest(playlist, 100, 10)?.id, 'a');
    });

    test('a later overlapping item wins (drawn on top)', () {
      // x=130 is inside both a (100..150) and b (120..170); b is later.
      expect(geometry.hitTest(playlist, 130, 10)?.id, 'b');
    });

    test('misses outside every block', () {
      expect(geometry.hitTest(playlist, 100, 200), isNull);
      expect(geometry.hitTest(playlist, 9999, 10), isNull);
    });

    test('respects the lane', () {
      const lane1 = PlaylistItem(
        id: 'c',
        patternId: 'p',
        startTicks: 0,
        lengthTicks: 100,
        trackIndex: 1,
      );
      const only = Playlist(items: [lane1]);
      expect(geometry.hitTest(only, 105, 40)?.id, 'c');
      expect(geometry.hitTest(only, 105, 10), isNull);
    });
  });

  group('snapping', () {
    test('snaps to the nearest grid line', () {
      // 490 is nearest 480; 750 is nearest 960 (1.5625 rounds up).
      expect(snapArrangementTick(490, 480), 480);
      expect(snapArrangementTick(750, 480), 960);
    });

    test('a non-positive grid leaves the tick unchanged', () {
      expect(snapArrangementTick(123, 0), 123);
    });

    test('never returns a negative tick', () {
      expect(snapArrangementTick(-10, 480), 0);
    });

    test('a grid of one bar snaps blocks to the bar', () {
      final bar = Ticks.barTicks(4);
      // Half a bar rounds up to the bar; a quarter rounds down to zero.
      expect(snapArrangementTick(bar ~/ 2, bar), bar);
      expect(snapArrangementTick(bar ~/ 4, bar), 0);
    });
  });
}

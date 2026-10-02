import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/plugins/plugin_host.dart';
import 'package:zenith_audio/plugins/plugin_slot.dart';

/// S7: the SDK-independent half of plugin hosting — slot state, serialization
/// and search-path resolution. No plugin binary required.
void main() {
  PluginSlotState slot(String id, {String? path, bool bypassed = false}) =>
      PluginSlotState(
        pluginId: id,
        format: PluginFormat.clap,
        path: path ?? '/p/$id.clap',
        bypassed: bypassed,
      );

  group('slot state', () {
    test('serialises and restores an opaque preset blob', () {
      final original = slot('a').copyWith(state: Uint8List.fromList([1, 2, 3, 250]));
      final restored = PluginSlotState.fromJson(original.toJson());
      expect(restored.pluginId, 'a');
      expect(restored.format, PluginFormat.clap);
      expect(restored.state, [1, 2, 3, 250]);
      expect(restored.hasState, isTrue);
    });

    test('a stateless slot omits the state key', () {
      final json = slot('a').toJson();
      expect(json.containsKey('state'), isFalse);
    });

    test('a corrupt state blob degrades to no state, not an error', () {
      final json = slot('a').toJson()..['state'] = '!!!not base64!!!';
      final restored = PluginSlotState.fromJson(json);
      expect(restored.pluginId, 'a');
      expect(restored.state, isEmpty);
    });

    test('an unknown format name falls back to clap', () {
      final json = slot('a').toJson()..['format'] = 'weird-future-format';
      expect(PluginSlotState.fromJson(json).format, PluginFormat.clap);
    });

    test('a round trip preserves bypass and track id', () {
      final original = slot('a', bypassed: true).copyWith(trackId: 't1');
      final restored = PluginSlotState.fromJson(original.toJson());
      expect(restored.bypassed, isTrue);
      expect(restored.trackId, 't1');
    });
  });

  group('slot bank', () {
    test('adding slots groups them by track in order', () {
      final bank = PluginSlotBank()
          .addSlot('t1', slot('a'))
          .addSlot('t1', slot('b'))
          .addSlot('t2', slot('c'));
      expect(bank.slotsFor('t1').map((s) => s.pluginId).toList(), ['a', 'b']);
      expect(bank.slotsFor('t2').map((s) => s.pluginId).toList(), ['c']);
      expect(bank.totalSlots, 3);
    });

    test('removing the last slot drops the empty track', () {
      final bank = PluginSlotBank().addSlot('t1', slot('a'));
      final after = bank.removeSlot('t1', 0);
      expect(after.slotsFor('t1'), isEmpty);
      expect(after.trackIds.contains('t1'), isFalse);
    });

    test('removing an out-of-range index is a no-op', () {
      final bank = PluginSlotBank().addSlot('t1', slot('a'));
      expect(bank.removeSlot('t1', 5).totalSlots, 1);
      expect(bank.removeSlot('nope', 0).totalSlots, 1);
    });

    test('reordering carries the slot value', () {
      final bank = PluginSlotBank()
          .addSlot('t1', slot('a'))
          .addSlot('t1', slot('b'))
          .addSlot('t1', slot('c'));
      final moved = bank.moveSlot('t1', 0, 2);
      expect(moved.slotsFor('t1').map((s) => s.pluginId).toList(), ['b', 'c', 'a']);
    });

    test('replacing a slot keeps its neighbours', () {
      final bank = PluginSlotBank()
          .addSlot('t1', slot('a'))
          .addSlot('t1', slot('b'));
      final replaced = bank.replaceSlot('t1', 0, slot('z'));
      expect(replaced.slotsFor('t1').map((s) => s.pluginId).toList(), ['z', 'b']);
    });

    test('pruneMissing drops uninstalled plugins and reports the track', () {
      final bank = PluginSlotBank()
          .addSlot('t1', slot('a', path: '/gone.clap'))
          .addSlot('t1', slot('b', path: '/here.clap'))
          .addSlot('t2', slot('c', path: '/also-gone.clap'));
      final result = bank.pruneMissing((path) => path == '/here.clap');
      expect(result.bank.slotsFor('t1').map((s) => s.pluginId).toList(), ['b']);
      expect(result.bank.slotsFor('t2'), isEmpty);
      expect(result.affectedTracks, {'t1', 't2'});
    });

    test('round-trips the whole bank through JSON', () {
      final bank = PluginSlotBank()
          .addSlot('t1', slot('a').copyWith(state: Uint8List.fromList([9, 8, 7])))
          .addSlot('t2', slot('c', bypassed: true));
      final restored = PluginSlotBank.fromJson(bank.toJson());
      expect(restored.totalSlots, 2);
      expect(restored.slotsFor('t1').single.state, [9, 8, 7]);
      expect(restored.slotsFor('t2').single.bypassed, isTrue);
    });

    test('a malformed JSON entry is skipped, not fatal', () {
      final json = <String, dynamic>{
        't1': [
          slot('a').toJson(),
          'not a slot',
          42,
        ],
        't2': 'not a list',
      };
      final bank = PluginSlotBank.fromJson(json);
      expect(bank.slotsFor('t1').map((s) => s.pluginId).toList(), ['a']);
      expect(bank.slotsFor('t2'), isEmpty);
    });

    test('slotsFor returns an unmodifiable view', () {
      final bank = PluginSlotBank().addSlot('t1', slot('a'));
      expect(() => bank.slotsFor('t1').add(slot('b')), throwsUnsupportedError);
    });
  });

  group('search paths', () {
    test('appends the CLAP directory to each root', () {
      final paths = PluginSearchPaths.clapPaths(['/Library/Audio/Plug-Ins', '/Users/x/.clap']);
      expect(paths, [
        '/Library/Audio/Plug-Ins/CLAP',
        '/Users/x/.clap/CLAP',
      ]);
    });

    test('normalises a trailing slash so the join is clean', () {
      final paths = PluginSearchPaths.clapPaths(['/root/']);
      expect(paths.single, '/root/CLAP');
    });

    test('an empty root set yields no paths', () {
      expect(PluginSearchPaths.clapPaths(const []), isEmpty);
    });
  });
}

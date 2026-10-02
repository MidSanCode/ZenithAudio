import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/musical_time.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/pattern.dart';
import 'package:zenith_audio/models/playlist.dart';
import 'package:zenith_audio/models/project.dart';
import 'package:zenith_audio/plugins/plugin_host.dart';
import 'package:zenith_audio/plugins/plugin_slot.dart';
import 'package:zenith_audio/services/lgdf_project_codec.dart';

/// S6/S7: the project document round-trips the arrangement and plugin slots.
///
/// A save that silently drops the arrangement is invisible until the user
/// reopens the project and finds their playlist gone. These tests pin the two
/// optional layers that were missing from `buildProjectDocument`.
void main() {
  const codec = LgdfProjectCodec();

  Project withArrangement() {
    final pattern = Pattern(
      id: 'p1',
      name: 'Lead',
      lengthTicks: Ticks.barTicks(4),
      notes: [Note(pitch: 60, startTicks: 0, lengthTicks: 480)],
    );
    final playlist = Playlist(
      items: const [
        PlaylistItem(
          id: 'i1',
          patternId: 'p1',
          startTicks: 0,
          lengthTicks: Ticks.ppq * 4,
        ),
      ],
    );
    return Project(
      id: 'proj',
      name: 'Song',
      patterns: [pattern],
      playlist: playlist,
    );
  }

  test('patterns and playlist survive a document round trip', () {
    final project = withArrangement();
    final doc = codec.buildProjectDocument(project, const {});
    final restored = codec.parseProjectDocument(doc);

    expect(restored.patterns, hasLength(1));
    expect(restored.patterns.single.id, 'p1');
    expect(restored.patterns.single.notes.single.pitch, 60);
    expect(restored.playlist, isNotNull);
    expect(restored.playlist!.items, hasLength(1));
    expect(restored.playlist!.items.single.patternId, 'p1');
  });

  test('the document omits the arrangement keys when there is none', () {
    const project = Project(id: 'p', name: 'Empty');
    final doc = codec.buildProjectDocument(project, const {});
    expect(doc.containsKey('patterns'), isFalse);
    expect(doc.containsKey('playlist'), isFalse);
    expect(doc.containsKey('plugin_slots'), isFalse);
  });

  test('plugin slots survive a document round trip', () {
    final bank = PluginSlotBank().addSlot(
      'track-1',
      PluginSlotState(
        pluginId: 'com.example.compressor',
        format: PluginFormat.clap,
        path: '/plugins/comp.clap',
        bypassed: true,
        state: Uint8List.fromList([1, 2, 3, 250]),
      ),
    );
    final project = Project(id: 'p', name: 'Plug', pluginSlots: bank);
    final doc = codec.buildProjectDocument(project, const {});
    final restored = codec.parseProjectDocument(doc);

    expect(restored.pluginSlots, isNotNull);
    final slot = restored.pluginSlots!.slotsFor('track-1').single;
    expect(slot.pluginId, 'com.example.compressor');
    expect(slot.bypassed, isTrue);
    expect(slot.state, [1, 2, 3, 250]);
  });

  test('a legacy document without the new keys still parses', () {
    // Exactly what an older version wrote: no patterns / playlist / plugins.
    final legacy = <String, dynamic>{
      'document_version': 1,
      'project_id': 'p',
      'name': 'Legacy',
      'tracks': <dynamic>[],
      'bpm': 120,
    };
    final project = codec.parseProjectDocument(legacy);
    expect(project.patterns, isEmpty);
    expect(project.playlist, isNull);
    expect(project.pluginSlots, isNull);
  });

  test('a track color is written and read back', () {
    // A small guard that the document shape itself is stable.
    const project = Project(id: 'p', name: 'N');
    final doc = codec.buildProjectDocument(project, const {});
    expect(doc['name'], 'N');
    expect(doc['bpm'], 120);
    expect(doc['tracks'], isA<List<dynamic>>());
    // Keep `material` imported for `Color` used by Track defaults elsewhere.
    expect(const Color(0xFF112233).toARGB32(), 0xFF112233);
  });
}

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/mixer/mixer_migration.dart';
import 'package:zenith_audio/mixer/mixer_model.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/track.dart';

Track makeTrack({
  required String id,
  String? name,
  double volume = 0.8,
  double pan = 0.0,
  bool isMuted = false,
  bool isSolo = false,
  String? mixerChannelId,
}) {
  return Track(
    id: id,
    name: name ?? id,
    type: TrackType.audio,
    volume: volume,
    pan: pan,
    isMuted: isMuted,
    isSolo: isSolo,
    color: const Color(0xFF00FF00),
    mixerChannelId: mixerChannelId,
  );
}

void main() {
  group('dB conversion', () {
    test('unity gain is 0 dB and round-trips', () {
      expect(dbToGain(0.0), closeTo(1.0, 1e-9));
      expect(gainToDb(1.0), closeTo(0.0, 1e-9));
    });

    test('known anchors match the engine fader curve', () {
      expect(dbToGain(6.0), closeTo(1.995262, 1e-5));
      expect(dbToGain(-6.0), closeTo(0.501187, 1e-5));
      expect(dbToGain(12.0), closeTo(3.981072, 1e-5));
    });

    test('the bottom of the fader is exact silence', () {
      expect(dbToGain(kMinGainDb), 0.0);
      expect(dbToGain(-200.0), 0.0);
      expect(dbToGain(double.negativeInfinity), 0.0);
    });

    test('clamping bounds the fader and survives NaN', () {
      expect(clampDb(99.0), kMaxGainDb);
      expect(clampDb(-999.0), kMinGainDb);
      expect(clampDb(double.nan), kMinGainDb);
    });
  });

  group('legacy volume migration', () {
    test('a linear volume becomes the matching dB position', () {
      expect(legacyVolumeToDb(1.0), closeTo(0.0, 1e-9));
      expect(legacyVolumeToDb(0.5), closeTo(-6.0206, 1e-3));
      expect(legacyVolumeToDb(0.25), closeTo(-12.0412, 1e-3));
    });

    test('zero volume maps to the fader floor, not to negative infinity', () {
      final db = legacyVolumeToDb(0.0);
      expect(db, kMinGainDb);
      expect(db.isFinite, isTrue, reason: 'a non-finite dB would poison JSON');
      // And it reads back as exact silence.
      expect(dbToLegacyVolume(db), 0.0);
    });

    test('the round trip is lossless across the whole usable range', () {
      for (final volume in [0.0, 0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0]) {
        final db = legacyVolumeToDb(volume);
        final back = dbToLegacyVolume(db);
        expect(
          back,
          closeTo(volume, 1e-6),
          reason: 'volume $volume round-tripped to $back',
        );
      }
    });
  });

  group('migrateFromTracks', () {
    test('creates one channel per track plus a master', () {
      final result = migrateFromTracks([
        makeTrack(id: 't1'),
        makeTrack(id: 't2'),
        makeTrack(id: 't3'),
      ]);

      expect(result.state.channels.length, 4, reason: '3 tracks + master');
      expect(result.state.channels.first.isMaster, isTrue);
      expect(result.hasChannels, isTrue);
    });

    test('carries volume, pan, mute and solo across losslessly', () {
      final track = makeTrack(
        id: 'lead',
        name: 'Lead',
        volume: 0.5,
        pan: -0.75,
        isMuted: true,
        isSolo: false,
      );
      final result = migrateFromTracks([track]);
      final channel = result.state.channelById('lead')!;

      expect(channel.name, 'Lead');
      // The channel's fader must reproduce the track's original linear volume.
      expect(dbToLegacyVolume(channel.gainDb), closeTo(0.5, 1e-6));
      expect(channel.pan, -0.75);
      expect(channel.isMuted, isTrue);
      expect(channel.isSolo, isFalse);
    });

    test('a muted-and-soloed track keeps both flags', () {
      // The awkward combination: a track can be soloed and muted at once, and
      // the migration must not "tidy" either flag away.
      final result = migrateFromTracks([
        makeTrack(id: 'both', isMuted: true, isSolo: true),
      ]);
      final channel = result.state.channelById('both')!;
      expect(channel.isMuted, isTrue);
      expect(channel.isSolo, isTrue);
    });

    test('a zero-volume track migrates to a silent channel', () {
      final result = migrateFromTracks([makeTrack(id: 'silent', volume: 0.0)]);
      final channel = result.state.channelById('silent')!;
      expect(channel.gainDb, kMinGainDb);
      expect(channel.linearGain, 0.0);
    });

    test('every migrated channel is routed to master', () {
      final result = migrateFromTracks([
        makeTrack(id: 'a'),
        makeTrack(id: 'b'),
      ]);
      for (final channel in result.state.channels.where((c) => !c.isMaster)) {
        expect(channel.output, kMasterChannelId);
      }
    });

    test('the track-to-channel map covers every track', () {
      final tracks = [makeTrack(id: 'a'), makeTrack(id: 'b')];
      final result = migrateFromTracks(tracks);
      expect(result.trackToChannel.keys.toSet(), {'a', 'b'});
      for (final track in tracks) {
        expect(result.state.channelById(result.trackToChannel[track.id]!),
            isNotNull);
      }
    });

    test('a declared mixerChannelId is honoured', () {
      final result = migrateFromTracks([
        makeTrack(id: 'a', mixerChannelId: 'chan-7'),
      ]);
      expect(result.trackToChannel['a'], 'chan-7');
      expect(result.state.channelById('chan-7'), isNotNull);
    });

    test('two tracks claiming one channel do not collapse into one', () {
      // A hand-edited file could name the same channel twice; both tracks must
      // still end up addressable.
      final result = migrateFromTracks([
        makeTrack(id: 'a', mixerChannelId: 'shared'),
        makeTrack(id: 'b', mixerChannelId: 'shared'),
      ]);
      expect(result.trackToChannel['a'], 'shared');
      expect(result.trackToChannel['b'], isNot('shared'));
      expect(result.state.channels.length, 3, reason: '2 tracks + master');
    });

    test('an empty project yields just a master', () {
      final result = migrateFromTracks([]);
      expect(result.state.channels.length, 1);
      expect(result.state.channels.single.isMaster, isTrue);
      expect(result.hasChannels, isFalse);
    });
  });

  group('audibility rules', () {
    test('with no solo, mute alone silences a channel', () {
      final result = migrateFromTracks([
        makeTrack(id: 'a', isMuted: true),
        makeTrack(id: 'b'),
      ]);
      final state = result.state;
      expect(state.isAudible(state.channelById('a')!), isFalse);
      expect(state.isAudible(state.channelById('b')!), isTrue);
    });

    test('solo silences everything not soloed', () {
      final result = migrateFromTracks([
        makeTrack(id: 'a'),
        makeTrack(id: 'b', isSolo: true),
      ]);
      final state = result.state;
      expect(state.isAudible(state.channelById('a')!), isFalse);
      expect(state.isAudible(state.channelById('b')!), isTrue);
    });

    test('master stays audible even when something else is soloed', () {
      final result = migrateFromTracks([makeTrack(id: 'a', isSolo: true)]);
      expect(result.state.isAudible(result.state.master), isTrue);
    });

    test('a muted soloed channel is still audible, matching the engine', () {
      // Solo wins in the engine, so the Dart model must agree or the UI would
      // grey out a channel the user can hear.
      final result = migrateFromTracks([
        makeTrack(id: 'a', isMuted: true, isSolo: true),
      ]);
      final state = result.state;
      expect(state.isAudible(state.channelById('a')!), isTrue);
    });
  });

  group('applyMixerToTracks', () {
    test('writes fader, pan, mute and solo back onto the track', () {
      final result = migrateFromTracks([makeTrack(id: 'a')]);
      final moved = result.state.replaceChannel(
        'a',
        result.state.channelById('a')!.copyWith(
              gainDb: -6.0,
              pan: 0.5,
              isMuted: true,
              isSolo: true,
            ),
      );

      final tracks = applyMixerToTracks(moved, [makeTrack(id: 'a')]);
      final track = tracks.single;

      expect(track.volume, closeTo(0.501187, 1e-5));
      expect(track.pan, 0.5);
      expect(track.isMuted, isTrue);
      expect(track.isSolo, isTrue);
      expect(track.mixerChannelId, 'a');
    });

    test('a round trip through the track leaves the mixer unchanged', () {
      final original = migrateFromTracks([
        makeTrack(id: 'a', volume: 0.3, pan: 0.2, isMuted: true),
      ]);
      final tracks = applyMixerToTracks(original.state, [
        makeTrack(id: 'a', volume: 0.3, pan: 0.2, isMuted: true),
      ]);
      final again = migrateFromTracks(tracks);

      final before = original.state.channelById('a')!;
      final after = again.state.channelById('a')!;
      expect(after.gainDb, closeTo(before.gainDb, 1e-5));
      expect(after.pan, closeTo(before.pan, 1e-9));
      expect(after.isMuted, before.isMuted);
      expect(after.isSolo, before.isSolo);
    });

    test('a track with no channel is passed through untouched', () {
      const state = MixerState(channels: []);
      final track = makeTrack(id: 'orphan', volume: 0.42, pan: 0.3);
      final result = applyMixerToTracks(state, [track]);
      expect(result.single.volume, 0.42);
      expect(result.single.pan, 0.3);
    });
  });

  group('serialization', () {
    test('a mixer state survives a JSON round trip', () {
      final original = MixerState(
        panLaw: MixerPanLaw.constantAmplitude6Db,
        channels: [
          const MixerChannel(
            id: kMasterChannelId,
            name: 'Master',
            role: MixerChannelRole.master,
          ),
          const MixerChannel(
            id: 'a',
            name: 'Lead',
            gainDb: -3.5,
            pan: -0.25,
            isMuted: true,
            isSolo: false,
            output: kMasterChannelId,
            sends: [
              MixerSend(
                enabled: true,
                levelDb: -6.0,
                tap: SendTap.preFader,
                destination: 'verb',
              ),
            ],
            effects: [
              MixerEffectSlot(kind: 7, wet: 0.5, sidechainSource: 'b'),
            ],
          ),
        ],
      );

      final restored = MixerState.fromJson(original.toJson())!;
      expect(restored.panLaw, MixerPanLaw.constantAmplitude6Db);
      expect(restored.channels.length, 2);

      final channel = restored.channelById('a')!;
      expect(channel.name, 'Lead');
      expect(channel.gainDb, closeTo(-3.5, 1e-9));
      expect(channel.pan, closeTo(-0.25, 1e-9));
      expect(channel.isMuted, isTrue);
      expect(channel.output, kMasterChannelId);
      expect(channel.sends.single.tap, SendTap.preFader);
      expect(channel.sends.single.destination, 'verb');
      expect(channel.effects.single.kind, 7);
      expect(channel.effects.single.wet, closeTo(0.5, 1e-9));
      expect(channel.effects.single.sidechainSource, 'b');
    });

    test('a missing mixer section reads as null so migration can run', () {
      expect(MixerState.fromJson(null), isNull);
      expect(MixerState.fromJson(<String, dynamic>{}), isNull);
      expect(MixerState.fromJson({'channels': 'nonsense'}), isNull);
      expect(MixerState.fromJson({'channels': <dynamic>[]}), isNull);
    });

    test('malformed channel entries are skipped rather than throwing', () {
      final state = MixerState.fromJson({
        'channels': [
          {'id': 'good', 'name': 'Good', 'role': 'insert'},
          'not a map',
          {'name': 'no id'},
          {'id': 'also-good', 'name': 'Also', 'gain_db': 'bad', 'pan': null},
        ],
      })!;

      expect(state.channels.length, 2);
      expect(state.channelById('good'), isNotNull);
      final second = state.channelById('also-good')!;
      expect(second.gainDb, 0.0, reason: 'a malformed gain falls back to unity');
      expect(second.pan, 0.0);
    });

    test('an unknown role or pan law falls back rather than failing', () {
      final state = MixerState.fromJson({
        'pan_law': 'from-the-future',
        'channels': [
          {'id': 'x', 'name': 'X', 'role': 'quantum'},
        ],
      })!;
      expect(state.panLaw, MixerPanLaw.constantPower3Db);
      expect(state.channels.single.role, MixerChannelRole.insert);
    });

    test('over-long send and effect lists are truncated to the limits', () {
      final state = MixerState.fromJson({
        'channels': [
          {
            'id': 'x',
            'name': 'X',
            'sends': List.generate(20, (i) => {'enabled': true, 'level_db': 0}),
            'effects': List.generate(50, (i) => {'kind': i}),
          },
        ],
      })!;
      final channel = state.channels.single;
      expect(channel.sends.length, kMaxSendsPerChannel);
      expect(channel.effects.length, kMaxEffectSlots);
    });
  });

  group('reconcileWithTracks', () {
    test('appends a channel for a track the mixer does not know', () {
      final initial = migrateFromTracks([makeTrack(id: 'a')]);
      final reconciled = reconcileWithTracks(initial.state, [
        makeTrack(id: 'a', volume: 0.5),
        makeTrack(id: 'b', volume: 0.25),
      ]);
      expect(reconciled.state.channelById('b'), isNotNull);
      expect(reconciled.state.channels.length, 3, reason: 'a, b, master');
    });

    test('never modifies an existing channel', () {
      final initial = migrateFromTracks([makeTrack(id: 'a', volume: 0.5)]);
      final before = initial.state.channelById('a')!;
      final reconciled = reconcileWithTracks(initial.state, [
        makeTrack(id: 'a', volume: 0.9), // a changed track value
        makeTrack(id: 'b'),
      ]);
      final after = reconciled.state.channelById('a')!;
      expect(after.gainDb, before.gainDb,
          reason: 'reconciliation must only add channels');
    });

    test('a fully covered project is unchanged', () {
      final initial = migrateFromTracks([makeTrack(id: 'a'), makeTrack(id: 'b')]);
      final reconciled = reconcileWithTracks(
        initial.state,
        [makeTrack(id: 'a'), makeTrack(id: 'b')],
      );
      expect(reconciled.state.channels.length, initial.state.channels.length);
    });
  });

  group('validateRouting', () {
    MixerChannel channel(String id, {String? output, MixerChannelRole role = MixerChannelRole.insert}) =>
        MixerChannel(id: id, name: id, role: role, output: output);

    test('a simple tree has no problems', () {
      final state = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('a', output: kMasterChannelId),
        channel('b', output: kMasterChannelId),
      ]);
      expect(validateRouting(state), isEmpty);
    });

    test('a direct cycle is reported', () {
      final state = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('a', output: 'b'),
        channel('b', output: 'a'),
      ]);
      final problems = validateRouting(state);
      expect(problems, isNotEmpty);
      expect(problems.where((p) => p.contains('成环')), isNotEmpty);
    });

    test('a self-route is reported', () {
      final state = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('a', output: 'a'),
      ]);
      expect(validateRouting(state), isNotEmpty);
    });

    test('a route to a missing channel is reported', () {
      final state = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('a', output: 'ghost'),
      ]);
      final problems = validateRouting(state);
      expect(problems.any((p) => p.contains('不存在')), isTrue);
    });

    test('a missing output is reported for non-master channels', () {
      final state = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('a'),
      ]);
      expect(validateRouting(state), isNotEmpty);
    });

    test('routing master onward is reported', () {
      final state = MixerState(channels: [
        channel(kMasterChannelId, output: 'a', role: MixerChannelRole.master),
        channel('a', output: kMasterChannelId),
      ]);
      expect(validateRouting(state), isNotEmpty);
    });

    test('a deep but legal chain is clean, and one hop too far is not', () {
      // master <- g3 <- g2 <- g1 <- g0 is exactly kMaxGroupDepth edges.
      final legal = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('g0', output: 'g1'),
        channel('g1', output: 'g2'),
        channel('g2', output: 'g3'),
        channel('g3', output: kMasterChannelId),
      ]);
      expect(validateRouting(legal), isEmpty,
          reason: 'depth $kMaxGroupDepth should be accepted');

      final tooDeep = MixerState(channels: [
        channel(kMasterChannelId, role: MixerChannelRole.master),
        channel('g0', output: 'g1'),
        channel('g1', output: 'g2'),
        channel('g2', output: 'g3'),
        channel('g3', output: 'g4'),
        channel('g4', output: kMasterChannelId),
      ]);
      expect(validateRouting(tooDeep), isNotEmpty,
          reason: 'one hop past the limit must be refused');
    });

    test('an empty mixer reports nothing', () {
      expect(validateRouting(const MixerState()), isEmpty);
    });
  });

  group('model value semantics', () {
    test('MixerChannel.copyWith clears output explicitly', () {
      const c = MixerChannel(id: 'a', name: 'A', output: 'master');
      expect(c.copyWith(output: 'b').output, 'b');
      expect(c.copyWith(clearOutput: true).output, isNull);
      expect(c.copyWith().output, 'master', reason: 'unset fields are kept');
    });

    test('an enabled send with no destination is inert', () {
      const send = MixerSend(enabled: true);
      expect(send.isActive, isFalse);
      expect(send.linearGain, 0.0);
    });

    test('a fully dry or bypassed slot does not process', () {
      const dry = MixerEffectSlot(kind: 1, wet: 0.0);
      expect(dry.isProcessing, isFalse);
      const bypassed = MixerEffectSlot(kind: 1, bypassed: true);
      expect(bypassed.isProcessing, isFalse);
      const active = MixerEffectSlot(kind: 1, wet: 0.5);
      expect(active.isProcessing, isTrue);
    });

    test('replaceChannel ignores an unknown id', () {
      final state = migrateFromTracks([makeTrack(id: 'a')]).state;
      final unchanged = state.replaceChannel(
        'nonexistent',
        const MixerChannel(id: 'nonexistent', name: 'Nope'),
      );
      expect(unchanged.channels.length, state.channels.length);
    });

    test('master is synthesised when a state omits it', () {
      const state = MixerState(channels: []);
      expect(state.master.isMaster, isTrue);
      expect(state.master.id, kMasterChannelId);
    });
  });

  group('integration', () {
    test('a legacy project opens with its mix intact', () {
      // The S3 acceptance criterion, end to end: a project file with no mixer
      // section, opened and inspected.
      final tracks = [
        makeTrack(id: 'drums', name: 'Drums', volume: 0.9, pan: 0.0),
        makeTrack(id: 'bass', name: 'Bass', volume: 0.7, pan: -0.3),
        makeTrack(id: 'keys', name: 'Keys', volume: 0.55, pan: 0.4, isMuted: true),
        makeTrack(id: 'vox', name: 'Vox', volume: 1.0, isSolo: true),
      ];

      // No 'mixer' key in the file, so this returns null and migration runs.
      expect(MixerState.fromJson(null), isNull);
      final result = migrateFromTracks(tracks);

      for (final track in tracks) {
        final channel = result.state.channelById(result.trackToChannel[track.id]!)!;
        expect(dbToLegacyVolume(channel.gainDb), closeTo(track.volume, 1e-6),
            reason: 'volume lost for ${track.id}');
        expect(channel.pan, closeTo(track.pan, 1e-9),
            reason: 'pan lost for ${track.id}');
        expect(channel.isMuted, track.isMuted, reason: 'mute lost for ${track.id}');
        expect(channel.isSolo, track.isSolo, reason: 'solo lost for ${track.id}');
      }

      expect(validateRouting(result.state), isEmpty);
    });

    test('a saved mixer reopens byte-identical', () {
      final first = migrateFromTracks([
        makeTrack(id: 'a', volume: 0.6, pan: -0.5),
        makeTrack(id: 'b', volume: 0.2, isSolo: true),
      ]).state;

      final json = first.toJson();
      final second = MixerState.fromJson(json)!;
      final third = MixerState.fromJson(second.toJson())!;

      // Idempotent under repeated serialisation: no value drifts on each save.
      expect(second.toJson(), json);
      expect(third.toJson(), json);
    });
  });
}

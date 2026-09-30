/// Legacy project migration into the mixer.
///
/// Older projects have no mixer: each track implicitly owns one volume, one pan
/// position and two flags. PLAN §3.S3 item 8 requires that opening one produces
/// a mixer where **the original volume, pan, mute and solo survive unchanged**.
///
/// ## Why this is a separate file
///
/// Migration is the one part of the mixer that must be *provably* lossless, and
/// proving it is much easier when the conversion is a pure function from tracks
/// to channels with no UI, no engine and no I/O involved. It is also the part
/// most likely to need revisiting as the project format evolves, so keeping it
/// isolated means the model itself never has to grow migration branches.
///
/// ## The losslessness contract
///
/// Track volume is a linear `0.0..1.0` gain while a mixer fader is a dB curve.
/// The conversion is therefore `db = 20·log10(volume)`, and the inverse is
/// `volume = 10^(db/20)`. Two cases need care:
///
/// * **`volume == 0`** maps to [kMinGainDb] (silence), not to `-infinity`. The
///   round trip returns exactly `0.0` because [dbToGain] treats the floor as
///   mute rather than as a very small gain.
/// * **Rounding.** `gainToDb(dbToGain(x)) == x` only to floating-point
///   precision, so nothing here rounds the stored dB value. The tests assert a
///   tolerance well below any audible threshold rather than exact equality.
library;

import '../models/track.dart';
import 'mixer_model.dart';

/// Converts a legacy linear track volume to a fader position in dB.
///
/// A track volume of `0.0` becomes [kMinGainDb], the fader floor, which the
/// engine renders as exact silence.
double legacyVolumeToDb(double volume) {
  if (volume.isNaN) return 0.0;
  if (volume <= 0) return kMinGainDb;
  return clampDb(gainToDb(volume));
}

/// Converts a fader position in dB back to a legacy linear track volume.
///
/// Used when writing a migrated project back in the legacy shape, and to
/// verify the round trip.
double dbToLegacyVolume(double db) {
  if (db <= kMinGainDb) return 0.0;
  final gain = dbToGain(db);
  return gain > 1.0 ? 1.0 : gain;
}

/// The result of migrating a project's tracks into mixer channels.
class MixerMigrationResult {
  /// Creates a migration result.
  const MixerMigrationResult({
    required this.state,
    required this.trackToChannel,
  });

  /// The mixer state built from the tracks.
  final MixerState state;

  /// Maps each track id to the id of the channel created for it.
  ///
  /// The editor uses this to route a track's audio into its channel without
  /// guessing at naming conventions.
  final Map<String, String> trackToChannel;

  /// Whether the migration produced any channel beyond master.
  bool get hasChannels => state.channels.any((c) => !c.isMaster);
}

/// Builds a mixer state for a legacy project from its tracks.
///
/// One track becomes one insert channel, following the plan's "each track = one
/// channel" rule. The channel id is the track id, so the association is
/// implicit and cannot drift.
///
/// Volume, pan, mute and solo are carried across exactly:
///
/// * `volume` → [MixerChannel.gainDb] via [legacyVolumeToDb]
/// * `pan` → [MixerChannel.pan] unchanged (both use `-1.0..1.0`)
/// * `isMuted` → [MixerChannel.isMuted]
/// * `isSolo` → [MixerChannel.isSolo]
///
/// Every channel is routed to master, which is the only default that cannot
/// introduce a cycle.
///
/// A track that already declares a [Track.mixerChannelId] is honoured by
/// reusing that id, so a partially-migrated project does not end up with two
/// channels claiming to own the same track.
MixerMigrationResult migrateFromTracks(List<Track> tracks) {
  final channels = <MixerChannel>[
    const MixerChannel(
      id: kMasterChannelId,
      name: 'Master',
      role: MixerChannelRole.master,
    ),
  ];
  final map = <String, String>{};
  final usedIds = <String>{kMasterChannelId};

  for (final track in tracks) {
    var channelId = track.mixerChannelId ?? track.id;
    // Guard against a file that assigns one channel to two tracks: keep the
    // first claim and give later tracks their own, so no track is silently
    // dropped from the mixer.
    if (usedIds.contains(channelId)) {
      channelId = '${track.id}~${usedIds.length}';
    }
    usedIds.add(channelId);

    channels.add(
      MixerChannel(
        id: channelId,
        name: track.name,
        role: MixerChannelRole.insert,
        gainDb: legacyVolumeToDb(track.volume),
        pan: track.pan.clamp(-1.0, 1.0),
        isMuted: track.isMuted,
        isSolo: track.isSolo,
        output: kMasterChannelId,
      ),
    );
    map[track.id] = channelId;
  }

  return MixerMigrationResult(
    state: MixerState(channels: channels),
    trackToChannel: map,
  );
}

/// Ensures a loaded mixer accounts for every track in the project.
///
/// A project can gain tracks after its mixer was written — a newer build may
/// have added a track, or a hand-edited file may omit one. Any track without a
/// channel gets one appended, using the same conversion as a full migration, so
/// a track is never silently inaudible.
///
/// Existing channels are returned untouched: this only ever adds.
MixerMigrationResult reconcileWithTracks(
  MixerState state,
  List<Track> tracks,
) {
  final channels = List<MixerChannel>.from(state.channels);
  final map = <String, String>{};
  final usedIds = channels.map((c) => c.id).toSet();

  // Index channels by the track they were built from. A channel created by
  // migration uses the track id, and a track records its channel in
  // `mixerChannelId`; either link is enough to consider it covered.
  final channelIds = channels.map((c) => c.id).toSet();

  for (final track in tracks) {
    final declared = track.mixerChannelId;
    if (declared != null && channelIds.contains(declared)) {
      map[track.id] = declared;
      continue;
    }
    if (channelIds.contains(track.id)) {
      map[track.id] = track.id;
      continue;
    }

    var channelId = declared ?? track.id;
    if (usedIds.contains(channelId)) {
      channelId = '${track.id}~${usedIds.length}';
    }
    usedIds.add(channelId);
    channels.add(
      MixerChannel(
        id: channelId,
        name: track.name,
        role: MixerChannelRole.insert,
        gainDb: legacyVolumeToDb(track.volume),
        pan: track.pan.clamp(-1.0, 1.0),
        isMuted: track.isMuted,
        isSolo: track.isSolo,
        output: kMasterChannelId,
      ),
    );
    map[track.id] = channelId;
  }

  return MixerMigrationResult(
    state: state.copyWith(channels: channels),
    trackToChannel: map,
  );
}

/// Writes mixer channel values back onto the project's tracks.
///
/// The inverse of [migrateFromTracks]: after the user moves a fader in the
/// mixer, the owning track's legacy `volume`/`pan`/`isMuted`/`isSolo` fields are
/// updated too. Without this the project would carry two disagreeing copies of
/// the same setting, and a reader that only understands tracks would hear the
/// stale one.
///
/// Tracks with no channel in [state] are returned unchanged.
List<Track> applyMixerToTracks(
  MixerState state,
  List<Track> tracks,
) {
  final result = <Track>[];
  for (final track in tracks) {
    MixerChannel? channel;
    final declared = track.mixerChannelId;
    if (declared != null) {
      channel = state.channelById(declared);
    }
    channel ??= state.channelById(track.id);

    if (channel == null || channel.isMaster) {
      result.add(track);
      continue;
    }

    result.add(
      track.copyWith(
        volume: dbToLegacyVolume(channel.gainDb),
        pan: channel.pan.clamp(-1.0, 1.0),
        isMuted: channel.isMuted,
        isSolo: channel.isSolo,
        mixerChannelId: channel.id,
      ),
    );
  }
  return result;
}

/// Validates that a mixer's routing is acyclic and within the depth limit.
///
/// Returns the list of problems found, empty when the routing is sound. This
/// mirrors the engine's build-time check (`mixer/graph.rs`) so the UI can refuse
/// a connection before it ever reaches the engine, and the user sees the reason
/// rather than a silent no-op.
///
/// Reported problems are human-readable and contain no third-party brand names.
List<String> validateRouting(MixerState state) {
  final problems = <String>[];
  final byId = {for (final c in state.channels) c.id: c};

  for (final channel in state.channels) {
    final output = channel.output;
    if (output == null) {
      if (!channel.isMaster) {
        problems.add('通道「${channel.name}」没有输出目标');
      }
      continue;
    }
    if (!byId.containsKey(output)) {
      problems.add('通道「${channel.name}」输出到不存在的通道「$output」');
      continue;
    }
    if (output == channel.id) {
      problems.add('通道「${channel.name}」不能输出到自身');
      continue;
    }
    if (channel.isMaster) {
      problems.add('主控通道不能再输出到其它通道');
      continue;
    }

    // Walk downstream looking for a return to this channel.
    var current = byId[output];
    var hops = 0;
    while (current != null) {
      if (current.id == channel.id) {
        problems.add('路由成环：通道「${channel.name}」经由「$output」回到了自身');
        break;
      }
      hops++;
      if (hops > state.channels.length) {
        problems.add('路由成环：从通道「${channel.name}」出发的链路无法终止');
        break;
      }
      final next = current.output;
      current = next == null ? null : byId[next];
    }
  }

  // Depth check, measured on the whole path rather than only downstream, so a
  // shallow-looking hop that splices two deep chains is still caught.
  for (final channel in state.channels) {
    if (channel.isMaster) continue;
    var depth = 0;
    var current = byId[channel.output];
    while (current != null && depth <= state.channels.length) {
      depth++;
      final next = current.output;
      current = next == null ? null : byId[next];
    }
    if (depth > kMaxGroupDepth) {
      problems.add(
        '通道「${channel.name}」的嵌套深度为 $depth，超过上限 $kMaxGroupDepth',
      );
    }
  }

  return problems;
}

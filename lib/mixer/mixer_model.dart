/// Mixer data model: channels, buses, sends and effect slots.
///
/// This is the Dart-side *mirror* of the Rust mixer in
/// `native/zenith_core/src/mixer/`. It exists for three reasons:
///
/// 1. **Persistence.** A project's mixer state has to be written to and read
///    from `spec/project.json`, and JSON is Dart's job.
/// 2. **Editing.** The UI mutates a plain value model; the FFI calls that push
///    those values into the engine happen at the boundary, not in the widget
///    tree.
/// 3. **Migration.** Legacy projects have one implicit channel per track, and
///    their volume/pan/mute/solo must survive the conversion losslessly
///    (PLAN §3.S3 item 8).
///
/// The Rust side remains authoritative for *audio*; this model never processes
/// samples and holds no buffers.
library;

import 'dart:math' as math;

/// Default insert channels a new project receives (PLAN §3.S3 item 1).
const int kDefaultInsertChannels = 64;

/// Default return buses a new project receives (PLAN §3.S3 item 1).
const int kDefaultReturnBuses = 8;

/// Sends available on every channel (PLAN §3.S3 item 3).
const int kMaxSendsPerChannel = 4;

/// Insert slots on every channel (PLAN §1.2, §3.S3 item 4).
const int kMaxEffectSlots = 10;

/// Maximum group nesting depth the router accepts (PLAN §3.S3 item 2).
const int kMaxGroupDepth = 4;

/// Lowest fader position, in decibels. Treated as silence.
const double kMinGainDb = -96.0;

/// Highest fader position, in decibels (PLAN §3.S3 item 6).
const double kMaxGainDb = 12.0;

/// What a channel is for.
///
/// The wire names are part of the project format: they are written to
/// `spec/project.json` and must stay stable.
enum MixerChannelRole {
  /// A normal insert channel fed by a track.
  insert('insert'),

  /// A return channel fed by sends.
  return_('return'),

  /// A group channel that sums other channels.
  group('group'),

  /// The single master channel.
  master('master');

  const MixerChannelRole(this.wireName);

  /// Stable identifier used in the project file.
  final String wireName;

  /// Parses a wire name, falling back to [insert] for unknown values.
  ///
  /// Tolerating an unknown role keeps a project written by a newer build
  /// readable rather than throwing away the whole mixer.
  static MixerChannelRole fromWire(String? value) {
    for (final role in MixerChannelRole.values) {
      if (role.wireName == value) return role;
    }
    return MixerChannelRole.insert;
  }
}

/// Where a send taps its channel.
enum SendTap {
  /// After the fader: the send follows the channel level.
  postFader('post'),

  /// Before the fader: the send ignores the channel level.
  preFader('pre');

  const SendTap(this.wireName);

  /// Stable identifier used in the project file.
  final String wireName;

  /// Parses a wire name, defaulting to [postFader].
  static SendTap fromWire(String? value) {
    for (final tap in SendTap.values) {
      if (tap.wireName == value) return tap;
    }
    return SendTap.postFader;
  }
}

/// Selectable pan laws, mirroring the Rust `PanLaw` enum.
enum MixerPanLaw {
  /// `-3 dB` at centre: constant power.
  constantPower3Db('cp3'),

  /// `-4.5 dB` at centre.
  constantPower4Point5Db('cp4'),

  /// `-6 dB` at centre: constant amplitude, folds to mono at unity.
  constantAmplitude6Db('ca6'),

  /// No attenuation anywhere.
  linear('linear');

  const MixerPanLaw(this.wireName);

  /// Stable identifier used in the project file.
  final String wireName;

  /// Parses a wire name, defaulting to [constantPower3Db].
  static MixerPanLaw fromWire(String? value) {
    for (final law in MixerPanLaw.values) {
      if (law.wireName == value) return law;
    }
    return MixerPanLaw.constantPower3Db;
  }
}

/// Converts decibels to a linear amplitude multiplier.
///
/// Mirrors the Rust `db_to_gain` so the UI can display a meter or a fader
/// position consistently with what the engine applies. At or below
/// [kMinGainDb] this returns exactly `0.0`, matching the engine's "the bottom
/// of the fader is a true mute" rule.
double dbToGain(double db) {
  if (db.isNaN || db <= kMinGainDb) return 0.0;
  return math.pow(10, db / 20).toDouble();
}

/// Converts a linear amplitude to decibels, mapping silence to [kMinGainDb].
double gainToDb(double gain) {
  if (gain.isNaN || gain <= 0) return kMinGainDb;
  final db = 20 * (math.log(gain) / math.ln10);
  return db < kMinGainDb ? kMinGainDb : db;
}

/// Clamps a decibel value into the fader's legal range.
double clampDb(double db) {
  if (db.isNaN) return kMinGainDb;
  return db.clamp(kMinGainDb, kMaxGainDb);
}

/// One send slot.
class MixerSend {
  /// Creates a send.
  const MixerSend({
    this.enabled = false,
    this.levelDb = 0.0,
    this.tap = SendTap.postFader,
    this.destination,
  });

  /// Whether this send contributes.
  final bool enabled;

  /// Send level, in decibels.
  final double levelDb;

  /// Where the send taps the channel.
  final SendTap tap;

  /// Destination channel id, or `null` when unrouted.
  final String? destination;

  /// Whether this send actually contributes.
  ///
  /// An enabled send with no destination is inert: enabling a send before
  /// choosing where it goes must not route it to the first available bus.
  bool get isActive => enabled && destination != null;

  /// Linear gain this send applies, or `0.0` when inactive.
  double get linearGain => isActive ? dbToGain(levelDb) : 0.0;

  /// Returns a copy with the given fields replaced.
  MixerSend copyWith({
    bool? enabled,
    double? levelDb,
    SendTap? tap,
    String? destination,
    bool clearDestination = false,
  }) {
    return MixerSend(
      enabled: enabled ?? this.enabled,
      levelDb: levelDb ?? this.levelDb,
      tap: tap ?? this.tap,
      destination: clearDestination ? null : (destination ?? this.destination),
    );
  }

  /// Serialises to the project format.
  Map<String, dynamic> toJson() => {
        'enabled': enabled,
        'level_db': levelDb,
        'tap': tap.wireName,
        if (destination != null) 'destination': destination,
      };

  /// Reads a send, tolerating missing or malformed fields.
  factory MixerSend.fromJson(Map<String, dynamic> json) {
    final rawLevel = json['level_db'];
    return MixerSend(
      enabled: json['enabled'] == true,
      levelDb: rawLevel is num ? clampDb(rawLevel.toDouble()) : 0.0,
      tap: SendTap.fromWire(json['tap'] as String?),
      destination: json['destination'] as String?,
    );
  }

  @override
  bool operator ==(Object other) =>
      other is MixerSend &&
      other.enabled == enabled &&
      other.levelDb == levelDb &&
      other.tap == tap &&
      other.destination == destination;

  @override
  int get hashCode => Object.hash(enabled, levelDb, tap, destination);

  @override
  String toString() =>
      'MixerSend(enabled: $enabled, levelDb: $levelDb, tap: ${tap.wireName}, '
      'destination: $destination)';
}

/// One insert slot.
class MixerEffectSlot {
  /// Creates an effect slot.
  const MixerEffectSlot({
    this.kind,
    this.bypassed = false,
    this.wet = 1.0,
    this.sidechainSource,
  });

  /// Effect kind id, or `null` when the slot is empty.
  ///
  /// Built-in kinds are `0x0000_0000..=0x0000_FFFF` and plugin kinds
  /// `0x0001_0000+` (ABI §11 Q2).
  final int? kind;

  /// Whether the slot is bypassed.
  final bool bypassed;

  /// Wet/dry balance, `0.0` fully dry to `1.0` fully wet.
  final double wet;

  /// Channel id this effect listens to, for sidechain-capable effects.
  final String? sidechainSource;

  /// Whether this slot is empty.
  bool get isEmpty => kind == null;

  /// Whether this slot should process audio.
  ///
  /// A fully dry slot is skipped so it cannot add latency to the channel
  /// without contributing any audible effect.
  bool get isProcessing => kind != null && !bypassed && wet > 0.0;

  /// Returns a copy with the given fields replaced.
  MixerEffectSlot copyWith({
    int? kind,
    bool? bypassed,
    double? wet,
    String? sidechainSource,
    bool clearKind = false,
    bool clearSidechain = false,
  }) {
    return MixerEffectSlot(
      kind: clearKind ? null : (kind ?? this.kind),
      bypassed: bypassed ?? this.bypassed,
      wet: wet ?? this.wet,
      sidechainSource:
          clearSidechain ? null : (sidechainSource ?? this.sidechainSource),
    );
  }

  /// Serialises to the project format.
  Map<String, dynamic> toJson() => {
        if (kind != null) 'kind': kind,
        if (bypassed) 'bypassed': true,
        if (wet != 1.0) 'wet': wet,
        if (sidechainSource != null) 'sidechain': sidechainSource,
      };

  /// Reads a slot, tolerating missing or malformed fields.
  factory MixerEffectSlot.fromJson(Map<String, dynamic> json) {
    final rawKind = json['kind'];
    final rawWet = json['wet'];
    return MixerEffectSlot(
      kind: rawKind is num ? rawKind.toInt() : null,
      bypassed: json['bypassed'] == true,
      wet: rawWet is num ? rawWet.toDouble().clamp(0.0, 1.0) : 1.0,
      sidechainSource: json['sidechain'] as String?,
    );
  }

  @override
  bool operator ==(Object other) =>
      other is MixerEffectSlot &&
      other.kind == kind &&
      other.bypassed == bypassed &&
      other.wet == wet &&
      other.sidechainSource == sidechainSource;

  @override
  int get hashCode => Object.hash(kind, bypassed, wet, sidechainSource);

  @override
  String toString() =>
      'MixerEffectSlot(kind: $kind, bypassed: $bypassed, wet: $wet, '
      'sidechain: $sidechainSource)';
}

/// A single mixer channel.
class MixerChannel {
  /// Creates a channel.
  const MixerChannel({
    required this.id,
    required this.name,
    this.role = MixerChannelRole.insert,
    this.gainDb = 0.0,
    this.pan = 0.0,
    this.isMuted = false,
    this.isSolo = false,
    this.phaseInvert = false,
    this.output,
    this.sends = const [],
    this.effects = const [],
  });

  /// Stable channel identifier.
  ///
  /// Track-linked channels use their track id so the association survives a
  /// save/load round trip without a separate lookup table.
  final String id;

  /// Display name.
  final String name;

  /// What this channel is for.
  final MixerChannelRole role;

  /// Fader position, in decibels.
  final double gainDb;

  /// Pan position, `-1.0` hard left to `1.0` hard right.
  final double pan;

  /// Whether the channel is muted.
  final bool isMuted;

  /// Whether the channel is soloed.
  final bool isSolo;

  /// Whether the channel's polarity is inverted.
  final bool phaseInvert;

  /// Channel this one feeds, or `null` for master.
  final String? output;

  /// The channel's sends, at most [kMaxSendsPerChannel].
  final List<MixerSend> sends;

  /// The channel's insert slots, at most [kMaxEffectSlots].
  final List<MixerEffectSlot> effects;

  /// Whether this is the master channel.
  bool get isMaster => role == MixerChannelRole.master;

  /// Linear fader gain, ignoring mute.
  double get linearGain => dbToGain(gainDb);

  /// Returns a copy with the given fields replaced.
  MixerChannel copyWith({
    String? id,
    String? name,
    MixerChannelRole? role,
    double? gainDb,
    double? pan,
    bool? isMuted,
    bool? isSolo,
    bool? phaseInvert,
    String? output,
    List<MixerSend>? sends,
    List<MixerEffectSlot>? effects,
    bool clearOutput = false,
  }) {
    return MixerChannel(
      id: id ?? this.id,
      name: name ?? this.name,
      role: role ?? this.role,
      gainDb: gainDb ?? this.gainDb,
      pan: pan ?? this.pan,
      isMuted: isMuted ?? this.isMuted,
      isSolo: isSolo ?? this.isSolo,
      phaseInvert: phaseInvert ?? this.phaseInvert,
      output: clearOutput ? null : (output ?? this.output),
      sends: sends ?? this.sends,
      effects: effects ?? this.effects,
    );
  }

  /// Serialises to the project format.
  ///
  /// Only non-default values are written for the optional fields, so a simple
  /// project's mixer stays small and an older reader sees a familiar shape.
  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'role': role.wireName,
        'gain_db': gainDb,
        'pan': pan,
        if (isMuted) 'muted': true,
        if (isSolo) 'solo': true,
        if (phaseInvert) 'phase_invert': true,
        if (output != null) 'output': output,
        if (sends.isNotEmpty) 'sends': sends.map((s) => s.toJson()).toList(),
        if (effects.isNotEmpty)
          'effects': effects.map((e) => e.toJson()).toList(),
      };

  /// Reads a channel, tolerating missing or malformed fields.
  factory MixerChannel.fromJson(Map<String, dynamic> json) {
    final rawGain = json['gain_db'];
    final rawPan = json['pan'];
    final rawSends = json['sends'];
    final rawEffects = json['effects'];

    final sends = <MixerSend>[];
    if (rawSends is List) {
      for (final entry in rawSends.take(kMaxSendsPerChannel)) {
        if (entry is Map) {
          sends.add(MixerSend.fromJson(Map<String, dynamic>.from(entry)));
        }
      }
    }

    final effects = <MixerEffectSlot>[];
    if (rawEffects is List) {
      for (final entry in rawEffects.take(kMaxEffectSlots)) {
        if (entry is Map) {
          effects.add(MixerEffectSlot.fromJson(Map<String, dynamic>.from(entry)));
        }
      }
    }

    return MixerChannel(
      id: (json['id'] as String?) ?? '',
      name: (json['name'] as String?) ?? '',
      role: MixerChannelRole.fromWire(json['role'] as String?),
      gainDb: rawGain is num ? clampDb(rawGain.toDouble()) : 0.0,
      pan: rawPan is num ? rawPan.toDouble().clamp(-1.0, 1.0) : 0.0,
      isMuted: json['muted'] == true,
      isSolo: json['solo'] == true,
      phaseInvert: json['phase_invert'] == true,
      output: json['output'] as String?,
      sends: sends,
      effects: effects,
    );
  }

  @override
  String toString() =>
      'MixerChannel(id: $id, role: ${role.wireName}, gainDb: $gainDb, '
      'pan: $pan, mute: $isMuted, solo: $isSolo)';
}

/// The master channel's id.
///
/// A fixed, reserved id so the master strip is addressable without scanning
/// the channel list.
const String kMasterChannelId = 'master';

/// The complete mixer state of a project.
class MixerState {
  /// Creates a mixer state.
  const MixerState({
    this.channels = const [],
    this.panLaw = MixerPanLaw.constantPower3Db,
  });

  /// Every channel, master included.
  final List<MixerChannel> channels;

  /// The pan law every channel renders through.
  final MixerPanLaw panLaw;

  /// Returns the master channel, creating a default one when absent.
  ///
  /// The mixer always has a master: a project whose file omits it must still
  /// produce one, otherwise there is no path to the output.
  MixerChannel get master {
    for (final c in channels) {
      if (c.isMaster) return c;
    }
    return const MixerChannel(
      id: kMasterChannelId,
      name: 'Master',
      role: MixerChannelRole.master,
    );
  }

  /// Whether any channel is soloed.
  bool get hasSolo => channels.any((c) => c.isSolo && !c.isMaster);

  /// Whether the mixer carries any channel beyond master.
  bool get isEmpty => channels.where((c) => !c.isMaster).isEmpty;

  /// Looks a channel up by id, or returns `null`.
  MixerChannel? channelById(String id) {
    for (final c in channels) {
      if (c.id == id) return c;
    }
    return null;
  }

  /// Whether [channel] should currently be heard.
  ///
  /// Solo is exclusive-by-presence, matching the engine's rule: if anything is
  /// soloed, only soloed channels sound. Master is always audible so a solo
  /// actually reaches the output.
  bool isAudible(MixerChannel channel) {
    if (channel.isMaster) return true;
    if (hasSolo) return channel.isSolo;
    return !channel.isMuted;
  }

  /// Returns a copy with the given fields replaced.
  MixerState copyWith({
    List<MixerChannel>? channels,
    MixerPanLaw? panLaw,
  }) {
    return MixerState(
      channels: channels ?? this.channels,
      panLaw: panLaw ?? this.panLaw,
    );
  }

  /// Replaces one channel, matched by id.
  ///
  /// Returns the previous state unchanged when [id] is not present, so a stale
  /// UI callback cannot invent a channel.
  MixerState replaceChannel(String id, MixerChannel updated) {
    final next = <MixerChannel>[];
    var found = false;
    for (final c in channels) {
      if (c.id == id) {
        next.add(updated);
        found = true;
      } else {
        next.add(c);
      }
    }
    if (!found) return this;
    return copyWith(channels: next);
  }

  /// Serialises to the project format.
  Map<String, dynamic> toJson() => {
        'pan_law': panLaw.wireName,
        'channels': channels.map((c) => c.toJson()).toList(),
      };

  /// Reads mixer state from a project file.
  ///
  /// Returns `null` when the mixer section is absent, which is how a legacy
  /// project is recognised: the caller then runs [migrateFromTracks].
  static MixerState? fromJson(Object? raw) {
    if (raw is! Map) return null;
    final map = Map<String, dynamic>.from(raw);
    final rawChannels = map['channels'];
    if (rawChannels is! List) return null;

    final channels = <MixerChannel>[];
    for (final entry in rawChannels) {
      if (entry is Map) {
        final channel =
            MixerChannel.fromJson(Map<String, dynamic>.from(entry));
        if (channel.id.isNotEmpty) channels.add(channel);
      }
    }
    if (channels.isEmpty) return null;

    return MixerState(
      channels: channels,
      panLaw: MixerPanLaw.fromWire(map['pan_law'] as String?),
    );
  }

  @override
  String toString() => 'MixerState(${channels.length} channels, '
      'panLaw: ${panLaw.wireName})';
}

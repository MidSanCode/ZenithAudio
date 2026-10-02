/// A serializable plugin slot: a descriptor reference plus per-instance state
/// (PLAN §3.S7: "插件状态（预设）随工程序列化").
///
/// ## Why this is separate from `PluginInstance`
///
/// `PluginInstance` is a live handle into the engine; it cannot be written to a
/// project file. What *is* written is a small record: which plugin, whether it
/// is bypassed, and an opaque state blob the plugin itself produced (its preset
/// in its own byte format). This type is that record — pure data, no engine, no
/// FFI — so project save/load is testable without a plugin binary.
///
/// ## The state blob is opaque on purpose
///
/// A CLAP plugin serialises its state in its own format; the host must not try
/// to understand it. Storing it as bytes (base64 in JSON) means any plugin's
/// preset round-trips without the host knowing what a "cutoff" is.
library;

import 'dart:convert';
import 'dart:typed_data';

import 'plugin_host.dart';

/// One plugin's saved state.
class PluginSlotState {
  /// Stable plugin id from the descriptor.
  final String pluginId;

  /// Format the plugin was loaded as.
  final PluginFormat format;

  /// Absolute path the plugin was loaded from, so a scan that misses it can
  /// still report where it used to be.
  final String path;

  /// Whether the slot is bypassed.
  final bool bypassed;

  /// Whether the slot is enabled at all (an inactive slot keeps its state).
  final bool enabled;

  /// Plugin-produced state blob (its preset). Empty when the plugin is
  /// stateless or never saved.
  final Uint8List state;

  /// The track / channel this slot is attached to, for migration.
  final String? trackId;

  PluginSlotState({
    required this.pluginId,
    required this.format,
    required this.path,
    this.bypassed = false,
    this.enabled = true,
    Uint8List? state,
    this.trackId,
  }) : state = state ?? Uint8List(0);

  /// Whether this slot carries a saved preset.
  bool get hasState => state.isNotEmpty;

  PluginSlotState copyWith({
    bool? bypassed,
    bool? enabled,
    Uint8List? state,
    String? trackId,
  }) =>
      PluginSlotState(
        pluginId: pluginId,
        format: format,
        path: path,
        bypassed: bypassed ?? this.bypassed,
        enabled: enabled ?? this.enabled,
        state: state ?? this.state,
        trackId: trackId ?? this.trackId,
      );

  /// Serialises the slot. The state blob is base64 so the JSON stays valid over
  /// any byte pattern the plugin produced.
  Map<String, dynamic> toJson() => {
        'plugin_id': pluginId,
        'format': format.name,
        'path': path,
        'bypassed': bypassed,
        'enabled': enabled,
        if (state.isNotEmpty) 'state': base64Encode(state),
        if (trackId != null) 'track_id': trackId,
      };

  /// Parses a slot, tolerating a missing or malformed state blob.
  ///
  /// A corrupt preset must not make the whole project unopenable: the slot is
  /// restored without its state, which the user can re-save. Losing one preset
  /// is far better than losing the project.
  factory PluginSlotState.fromJson(Map<String, dynamic> json) {
    Uint8List state = Uint8List(0);
    final encoded = json['state'];
    if (encoded is String && encoded.isNotEmpty) {
      try {
        state = base64Decode(encoded);
      } catch (_) {
        state = Uint8List(0);
      }
    }
    return PluginSlotState(
      pluginId: json['plugin_id'] as String? ?? '',
      format: _formatFromName(json['format'] as String?),
      path: json['path'] as String? ?? '',
      bypassed: json['bypassed'] as bool? ?? false,
      enabled: json['enabled'] as bool? ?? true,
      state: state,
      trackId: json['track_id'] as String?,
    );
  }

  static PluginFormat _formatFromName(String? name) {
    for (final f in PluginFormat.values) {
      if (f.name == name) return f;
    }
    return PluginFormat.clap;
  }
}

/// Resolves a plugin search path against the platform's plugin directories.
///
/// Pure strings, no filesystem: a caller passes the candidate directory roots
/// and gets back the paths to probe. This keeps the platform logic testable on
/// any machine.
///
/// The directory names are CLAP's own well-known relative paths, not another
/// product's — PLAN §0.2 forbids a third-party brand name in the code, and a
/// plugin directory name is exactly the sort of place one would leak in.
abstract final class PluginSearchPaths {
  /// The well-known CLAP directory name, relative to a platform root.
  static const String clapDirName = 'CLAP';

  /// Builds the full search list from platform roots.
  ///
  /// `roots` are already platform-specific (e.g. `/Library/Audio/Plug-Ins` on
  /// macOS); the caller supplies them so platform detection lives at the edge
  /// and this stays pure.
  static List<String> clapPaths(Iterable<String> roots) {
    final out = <String>[];
    for (final root in roots) {
      // Normalise a trailing slash so joining does not double it.
      final base = root.endsWith('/') ? root.substring(0, root.length - 1) : root;
      out.add('$base/$clapDirName');
    }
    return out;
  }
}

/// The project's plugin slots, grouped by track, with JSON round-tripping.
///
/// Pure data and pure operations: no engine, no FFI. A `PluginHost` instance is
/// what actually loads a plugin; this is the part that survives save/load and
/// that the UI binds to. Splitting them means the serialization — the part that
/// must not lose a user's preset — is tested without a plugin binary.
class PluginSlotBank {
  /// Slots per track id, in processing order.
  final Map<String, List<PluginSlotState>> _byTrack;

  PluginSlotBank([Map<String, List<PluginSlotState>>? byTrack])
      : _byTrack = {
          if (byTrack != null)
            for (final entry in byTrack.entries)
              entry.key: List<PluginSlotState>.from(entry.value),
        };

  /// The slots on a track, or an empty list.
  List<PluginSlotState> slotsFor(String trackId) =>
      List.unmodifiable(_byTrack[trackId] ?? const []);

  /// Whether any track has a slot.
  bool get isEmpty => _byTrack.values.every((s) => s.isEmpty);

  /// Total slots across every track.
  int get totalSlots =>
      _byTrack.values.fold(0, (sum, slots) => sum + slots.length);

  /// Every track id that has at least one slot.
  Iterable<String> get trackIds => _byTrack.keys;

  /// Appends a slot to a track, returning a new bank.
  PluginSlotBank addSlot(String trackId, PluginSlotState slot) {
    final next = Map<String, List<PluginSlotState>>.from(_byTrack);
    next[trackId] = [...(next[trackId] ?? const []), slot];
    return PluginSlotBank(next);
  }

  /// Removes the slot at [index] on a track.
  ///
  /// Out-of-range is a no-op rather than a throw: a UI click on a stale index
  /// must not crash the editor.
  PluginSlotBank removeSlot(String trackId, int index) {
    final slots = _byTrack[trackId];
    if (slots == null || index < 0 || index >= slots.length) return this;
    final next = Map<String, List<PluginSlotState>>.from(_byTrack);
    final copy = [...slots]..removeAt(index);
    if (copy.isEmpty) {
      next.remove(trackId);
    } else {
      next[trackId] = copy;
    }
    return PluginSlotBank(next);
  }

  /// Replaces the slot at [index] on a track.
  PluginSlotBank replaceSlot(String trackId, int index, PluginSlotState slot) {
    final slots = _byTrack[trackId];
    if (slots == null || index < 0 || index >= slots.length) return this;
    final next = Map<String, List<PluginSlotState>>.from(_byTrack);
    final copy = [...slots];
    copy[index] = slot;
    next[trackId] = copy;
    return PluginSlotBank(next);
  }

  /// Moves a slot within a track (reorder), carrying its state.
  PluginSlotBank moveSlot(String trackId, int from, int to) {
    final slots = _byTrack[trackId];
    if (slots == null ||
        from < 0 ||
        to < 0 ||
        from >= slots.length ||
        to >= slots.length ||
        from == to) {
      return this;
    }
    final next = Map<String, List<PluginSlotState>>.from(_byTrack);
    final copy = [...slots];
    final moved = copy.removeAt(from);
    copy.insert(to, moved);
    next[trackId] = copy;
    return PluginSlotBank(next);
  }

  /// Drops every slot whose plugin path no longer exists.
  ///
  /// Called after a scan: a plugin the user uninstalled must not leave a
  /// dangling slot that fails to load on every playback. Returns the new bank
  /// and the ids of the tracks it changed.
  ({PluginSlotBank bank, Set<String> affectedTracks}) pruneMissing(
    bool Function(String path) exists,
  ) {
    final next = <String, List<PluginSlotState>>{};
    final affected = <String>{};
    for (final entry in _byTrack.entries) {
      final kept = entry.value.where((s) => exists(s.path)).toList();
      if (kept.length != entry.value.length) affected.add(entry.key);
      if (kept.isNotEmpty) next[entry.key] = kept;
    }
    return (bank: PluginSlotBank(next), affectedTracks: affected);
  }

  /// Serialises the whole bank to a JSON-ready map keyed by track id.
  Map<String, dynamic> toJson() => {
        for (final entry in _byTrack.entries)
          entry.key: entry.value.map((s) => s.toJson()).toList(),
      };

  /// Parses a bank, tolerating any malformed entry.
  factory PluginSlotBank.fromJson(Map<String, dynamic> json) {
    final byTrack = <String, List<PluginSlotState>>{};
    for (final entry in json.entries) {
      final raw = entry.value;
      if (raw is! List) continue;
      final slots = <PluginSlotState>[];
      for (final item in raw) {
        if (item is Map<String, dynamic>) {
          slots.add(PluginSlotState.fromJson(item));
        } else if (item is Map) {
          slots.add(PluginSlotState.fromJson(Map<String, dynamic>.from(item)));
        }
      }
      if (slots.isNotEmpty) byTrack[entry.key] = slots;
    }
    return PluginSlotBank(byTrack);
  }
}

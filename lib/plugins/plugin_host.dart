/// Plugin hosting abstraction.
///
/// S0 ships interfaces only. S7 implements a CLAP-backed host; the plugin DSP
/// runs in-process with the Rust core.
///
/// ## Deliberate constraint
///
/// There is **no Dart-side effect interface** in this codebase, and this file
/// must not grow one. Effects — including effects loaded from plugins — are
/// implemented in Rust, and Dart only asks for parameter descriptors to build
/// UI (PLAN_DAW_PARITY.md §7 S0 item 4). Adding a Dart `Effect` abstraction
/// would create a second, competing implementation surface.
///
/// ## Licensing
///
/// Only CLAP and the project's own plugin ABI are supported. VST is explicitly
/// out of scope: its SDK licence is incompatible with this project's
/// AGPL-3.0-or-later licence.
library;

/// Plugin formats this host may load.
enum PluginFormat {
  /// CLAP — the primary supported format.
  clap,

  /// ZENITH's own ABI, used for bundled first-party effects.
  zenithNative,
}

/// A plugin scanned from disk, not yet instantiated.
class PluginDescriptor {
  /// Creates a descriptor.
  const PluginDescriptor({
    required this.id,
    required this.name,
    required this.vendor,
    required this.format,
    required this.path,
    this.version,
  });

  /// Stable identifier, unique within a machine.
  final String id;

  /// Display name.
  final String name;

  /// Vendor string, for grouping in the browser.
  final String vendor;

  /// Format this descriptor was discovered as.
  final PluginFormat format;

  /// Absolute path of the loadable binary on disk.
  final String path;

  /// Version string, when the format exposes one.
  final String? version;
}

/// A live plugin instance inside the engine.
///
/// The instance belongs to the engine's audio graph; the Dart side holds only
/// this handle and never touches sample buffers.
abstract interface class PluginInstance {
  /// The descriptor this instance was created from.
  PluginDescriptor get descriptor;

  /// Whether the instance is currently processing audio.
  bool get isActive;

  /// Whether the plugin reported an error that requires the user to act.
  bool get hasError;

  /// Last error message, when [hasError] is set.
  String? get lastError;

  /// Enables or bypasses processing.
  void setActive(bool active);

  /// Opens the plugin's own editor window, when it has one.
  ///
  /// Returns `false` when the plugin is headless; the caller then shows a
  /// generic parameter panel built from the parameter store.
  Future<bool> openEditor();

  /// Closes the plugin editor, if open.
  Future<void> closeEditor();

  /// Destroys the instance and releases its native resources.
  ///
  /// Idempotent.
  Future<void> dispose();
}

/// Discovers, instantiates and tracks plugins.
///
/// Implementations own the plugin search paths and the scan cache; nothing
/// else in the app walks the filesystem for plugins.
abstract interface class PluginHost {
  /// Whether the host finished its initial scan.
  bool get isScanned;

  /// Every plugin found by the last scan.
  List<PluginDescriptor> get available;

  /// Instances currently alive in the engine.
  List<PluginInstance> get loaded;

  /// Scans the platform's plugin paths.
  ///
  /// Returns the number of plugins discovered. Implementations must tolerate
  /// unreadable or corrupt plugin binaries rather than failing the whole scan —
  /// one bad plugin must not break a user's plugin library.
  Future<int> scan();

  /// Instantiates a plugin on a track.
  ///
  /// `trackId` follows the same identity used by [ParameterId.ownerId] in
  /// `automation/parameter.dart`, so a plugin's parameters are automatable
  /// through the ordinary parameter store.
  Future<PluginInstance> instantiate(PluginDescriptor descriptor, String trackId);

  /// Destroys every live instance.
  ///
  /// Called on project close and before a full engine shutdown.
  Future<void> unloadAll();
}

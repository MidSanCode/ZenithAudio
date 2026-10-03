// Native-assets build hook: drives `cargo build` for the Rust core.
//
// Flutter's hooks_runner executes this before linking a native target, so
// desktop, CI and WASM all go through one code path. `cargo` is an external
// build system that already produces a finished artifact, so this hook only
// builds and *registers* it — it never compiles C itself.
import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';

/// Rust crate that provides the C ABI described in `docs/ABI.md`.
const _crateName = 'zenith_core';

/// Path of the crate, relative to the package root.
const _cratePath = 'native/$_crateName';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;

    final codeConfig = input.config.code;
    final targetOS = codeConfig.targetOS;
    final targetArch = codeConfig.targetArchitecture;
    final rustTriple = _rustTriple(targetOS, targetArch);

    // This hooks API version has no `buildType`; release/debug is a user-define
    // so the same hook serves `flutter run` and `flutter build`. It defaults to
    // release because a debug-built DSP core is not something we ever ship.
    final isRelease = input.userDefines['release'] as bool? ?? true;

    // `cargo` writes into the *package* root's `target/` directory. Register
    // the manifest as a dependency so the hook re-runs when the Rust code
    // changes and the build cache does not go stale.
    final packageRoot = Directory.fromUri(input.packageRoot);
    final manifest = File('${packageRoot.path}/$_cratePath/Cargo.toml');
    if (!manifest.existsSync()) {
      throw BuildError(
        message: 'Rust manifest not found at ${manifest.path}.',
      );
    }
    output.dependencies.add(manifest.uri);
    final cargoLock = File('${packageRoot.path}/Cargo.lock');
    if (cargoLock.existsSync()) output.dependencies.add(cargoLock.uri);

    // Build into the hook's shared output directory rather than the package's
    // `target/`: keeps `flutter clean` semantics honest and stops two configs
    // from fighting over one target dir.
    final targetDir = Directory.fromUri(
      input.outputDirectoryShared.resolve('cargo/'),
    );
    if (!targetDir.existsSync()) targetDir.createSync(recursive: true);

    final result = await Process.run(
      'cargo',
      <String>[
        'build',
        '--manifest-path',
        manifest.path,
        '--target',
        rustTriple,
        '--target-dir',
        targetDir.path,
        if (isRelease) '--release',
      ],
      workingDirectory: packageRoot.path,
      runInShell: targetOS == OS.windows,
    );

    if (result.exitCode != 0) {
      // Surface cargo's stderr verbatim — it carries the actionable error.
      throw BuildError(
        message: 'cargo build failed for $rustTriple '
            '(exit ${result.exitCode}):\n${result.stderr}',
      );
    }

    // Cargo decides the artifact name/extension per target; derive it from the
    // target OS instead of hardcoding one platform's spelling.
    final profileDir = isRelease ? 'release' : 'debug';
    final library = File(
      '${targetDir.path}/$rustTriple/$profileDir/${_libraryFileName(targetOS)}',
    );
    if (!library.existsSync()) {
      throw BuildError(
        message: 'cargo reported success but the expected library was not '
            'found at ${library.path}',
      );
    }

    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: _crateName,
        linkMode: DynamicLoadingBundled(),
        file: library.uri,
      ),
    );
  });
}

/// The on-disk library name for a target OS.
String _libraryFileName(OS targetOS) {
  switch (targetOS) {
    case OS.windows:
      return '$_crateName.dll';
    case OS.macOS:
    case OS.iOS:
      return 'lib$_crateName.dylib';
    default:
      return 'lib$_crateName.so';
  }
}

/// Maps a Dart (OS, Architecture) pair to a Rust target triple.
///
/// Throwing on an unmapped pair is deliberate: silently falling back to the
/// host triple would link a library built for the wrong architecture.
String _rustTriple(OS os, Architecture arch) {
  return switch ((os, arch)) {
    (OS.windows, Architecture.x64) => 'x86_64-pc-windows-msvc',
    (OS.linux, Architecture.x64) => 'x86_64-unknown-linux-gnu',
    (OS.linux, Architecture.arm64) => 'aarch64-unknown-linux-gnu',
    (OS.macOS, Architecture.arm64) => 'aarch64-apple-darwin',
    (OS.macOS, Architecture.x64) => 'x86_64-apple-darwin',
    (OS.iOS, Architecture.arm64) => 'aarch64-apple-ios',
    (OS.android, Architecture.arm64) => 'aarch64-linux-android',
    (OS.android, Architecture.arm) => 'armv7-linux-androideabi',
    (OS.android, Architecture.x64) => 'x86_64-linux-android',
    (OS.android, Architecture.ia32) => 'i686-linux-android',
    _ => throw BuildError(
        message: 'Unsupported OS/architecture for the Rust core: $os / $arch',
      ),
  };
}

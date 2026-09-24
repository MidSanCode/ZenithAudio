import 'dart:convert';

import 'package:crypto/crypto.dart';

/// LGDF (Layered Generic Data Format) v2.0 primitives.
///
/// A DAW project is stored as a *directory-mode* LGDF project:
///
/// ```
/// <project>/                 工程根目录
/// ├── info.json              核心配置（工程身份证）
/// ├── registry.json          注册资源清单
/// ├── assets/audios/         资源层：音频
/// ├── metadata/audios/       元数据层：与 assets/ 一一镜像
/// ├── spec/                  描述层：project.json / config.json / overview.md
/// ├── work/                  SDK 临时文件（永不导出）
/// └── dist/                  发布目录：导出的 .zaproj 包
/// ```
///
/// Export packs the directory into a Deflate ZIP with the `.zaproj` extension
/// (ZENITH AUDIO PROJect) plus a `.sha256` companion file. The *contents* are
/// always LGDF — only the outer container extension is app-specific.
class Lgdf {
  Lgdf._();

  /// Format identifier written to `info.json`.
  static const String format = 'lgdf';

  /// Lowest SDK version able to read what we write.
  static const int minSdk = 1;

  /// Archive extension used for exports (the app's own project container).
  static const String extension = '.zaproj';

  /// Extensions accepted as an exported project archive, newest first.
  static const List<String> knownArchiveExtensions = ['.zaproj', '.lgdf', '.zap'];

  /// Removes a known archive extension from [fileName], if present.
  static String stripArchiveExtension(String fileName) {
    final lower = fileName.toLowerCase();
    for (final ext in knownArchiveExtensions) {
      if (lower.endsWith(ext)) {
        return fileName.substring(0, fileName.length - ext.length);
      }
    }
    return fileName;
  }

  /// A short, slug-safe fragment of a project id for fallback naming.
  ///
  /// Ids shorter than 8 characters are used whole — a fixed-length prefix
  /// would throw on them.
  static String shortId(String id) {
    if (id.isEmpty) return 'new';
    return id.length <= 8 ? id : id.substring(0, 8);
  }

  /// The project slug used for folder and package names.
  ///
  /// Takes the raw name/id so this module stays free of model imports.
  static String projectSlug(String name, String id) => slugify(
        name,
        fallback: 'project-${shortId(id)}',
      );

  // ── Canonical paths ──

  static const String infoFile = 'info.json';
  static const String registryFile = 'registry.json';
  static const String assetsDir = 'assets';
  static const String audioAssetsDir = 'assets/audios';
  static const String metadataDir = 'metadata';
  static const String audioMetadataDir = 'metadata/audios';
  static const String specDir = 'spec';
  static const String workDir = 'work';
  static const String distDir = 'dist';
  static const String ignoreFile = '.lgdfignore';

  // ── Description-layer files ──

  static const String projectSpecFile = 'spec/project.json';
  static const String configSpecFile = 'spec/config.json';
  static const String schemaSpecFile = 'spec/config.schema.json';
  static const String overviewSpecFile = 'spec/overview.md';

  /// Entries never included in an export package.
  static const Set<String> excludedFromPack = {workDir, distDir, '.tmp'};

  /// Default `.lgdfignore` written into new projects (gitignore syntax).
  static const String defaultIgnore = '''
# LGDF 附加排除规则（语法同 gitignore）
work/
dist/
.tmp/
''';

  /// Serializes [value] as UTF-8 JSON, Tab-indented, no BOM — the style
  /// required by the standard.
  static String encodeJson(Object? value) {
    const encoder = JsonEncoder.withIndent('\t');
    return encoder.convert(value);
  }

  static List<int> encodeJsonBytes(Object? value) =>
      utf8.encode(encodeJson(value));

  /// Lowercase hex SHA-256 of [bytes] (the standard mandates lowercase).
  static String sha256Hex(List<int> bytes) =>
      sha256.convert(bytes).toString().toLowerCase();

  /// Lowercase hex SHA-256 of an in-memory string (UTF-8 encoded).
  static String sha256HexOfString(String text) =>
      sha256Hex(utf8.encode(text));

  /// Normalizes a path to the LGDF form: `/` separated, relative, no leading
  /// slash, no `.` segments.
  static String normalizePath(String path) {
    final parts = path
        .replaceAll('\\', '/')
        .split('/')
        .where((p) => p.isNotEmpty && p != '.')
        .toList();
    return parts.join('/');
  }

  /// MIME type for a resource extension, per the standard's recommendation table.
  static String mimeForExtension(String ext) {
    switch (ext.toLowerCase()) {
      case '.wav':
        return 'audio/wav';
      case '.ogg':
        return 'audio/ogg';
      case '.flac':
        return 'audio/flac';
      case '.mp3':
        return 'audio/mpeg';
      case '.aac':
        return 'audio/aac';
      case '.m4a':
        return 'audio/mp4';
      case '.png':
        return 'image/png';
      case '.jpg':
      case '.jpeg':
        return 'image/jpeg';
      case '.svg':
        return 'image/svg+xml';
      case '.webp':
        return 'image/webp';
      case '.json':
        return 'application/json';
      case '.md':
        return 'text/markdown';
      default:
        return 'application/octet-stream';
    }
  }

  /// LGDF resource `type` for a resource extension.
  static String resourceTypeForExtension(String ext) {
    switch (ext.toLowerCase()) {
      case '.wav':
      case '.ogg':
      case '.flac':
      case '.mp3':
      case '.aac':
      case '.m4a':
        return 'audio';
      case '.png':
      case '.jpg':
      case '.jpeg':
      case '.svg':
      case '.webp':
        return 'image';
      case '.mp4':
      case '.webm':
        return 'video';
      case '.gltf':
      case '.glb':
      case '.obj':
      case '.stl':
        return 'model';
      case '.txt':
      case '.md':
      case '.json':
      case '.yaml':
      case '.toml':
        return 'text';
      default:
        return 'binary';
    }
  }

  /// Extension (with dot, lowercase) of a file name/path.
  static String extensionOf(String path) {
    final slash = path.lastIndexOf('/');
    final backslash = path.lastIndexOf('\\');
    final start = slash > backslash ? slash : backslash;
    final dot = path.lastIndexOf('.');
    if (dot <= start) return '';
    return path.substring(dot).toLowerCase();
  }

  /// File name without extension.
  static String stemOf(String path) {
    final slash = path.lastIndexOf('/');
    final backslash = path.lastIndexOf('\\');
    final start = slash > backslash ? slash : backslash;
    final dot = path.lastIndexOf('.');
    final name = start >= 0 ? path.substring(start + 1) : path;
    if (dot <= start) return name;
    return path.substring(start + 1, dot);
  }

  /// Sanitizes a display name into a valid LGDF `name` (`^[a-z0-9_-]+$`).
  ///
  /// Non-conforming characters become `-`; the result is lowercased and
  /// collapsed. Falls back to [fallback] when nothing usable remains.
  static String slugify(String input, {String fallback = 'project'}) {
    final lowered = input.trim().toLowerCase();
    final buffer = StringBuffer();
    for (final rune in lowered.runes) {
      final ch = String.fromCharCode(rune);
      if (RegExp(r'[a-z0-9_-]').hasMatch(ch)) {
        buffer.write(ch);
      } else if (ch == ' ' || ch == '.') {
        buffer.write('-');
      } else {
        // Non-ASCII (e.g. CJK) has no slug form in this scheme — drop it.
        buffer.write('-');
      }
    }
    var slug = buffer
        .toString()
        .replaceAll(RegExp(r'-{2,}'), '-')
        .replaceAll(RegExp(r'^-+|-+$'), '');
    if (slug.isEmpty) slug = fallback;
    return slug;
  }

  /// Builds the `metadata/<path>.json` content for one resource.
  static Map<String, dynamic> buildAssetMetadata({
    required String path,
    required int size,
    required String sha256Hex,
    required int createdTime,
    required int lastUpdateTime,
    Map<String, dynamic>? extra,
  }) {
    final ext = extensionOf(path);
    final type = resourceTypeForExtension(ext);
    return {
      'path': path,
      'type': type,
      'mime': mimeForExtension(ext),
      'format': ext.replaceFirst('.', ''),
      'size': size,
      'sha256': sha256Hex,
      'hash_algorithm': 'sha256',
      'created_time': createdTime,
      'last_update_time': lastUpdateTime,
      if (type == 'text') 'encoding': 'utf-8',
      if (extra != null && extra.isNotEmpty) 'extra': extra,
    };
  }

  /// Builds the `registry.json` content for a set of registered paths.
  static Map<String, dynamic> buildRegistry(List<String> registeredFiles) {
    final sorted = [...registeredFiles]..sort();
    return {
      'registered_files': sorted,
      'asset_count': sorted.length,
    };
  }

  /// Builds the `info.json` content.
  static Map<String, dynamic> buildInfo({
    required String name,
    required int createdTime,
    required int lastUpdateTime,
    String? displayName,
    String description = '',
    String? author,
    String? license,
    List<String> tags = const [],
    int version = 1,
    Map<String, dynamic> extra = const {},
    Map<String, dynamic>? authorSign,
  }) {
    return {
      'format': format,
      'min_sdk': minSdk,
      'name': name,
      if (displayName != null && displayName.isNotEmpty)
        'display_name': displayName,
      'description': description,
      if (author != null && author.isNotEmpty) 'author': author,
      if (license != null && license.isNotEmpty) 'license': license,
      'tags': tags,
      'created_time': createdTime,
      'last_update_time': lastUpdateTime,
      'version': version,
      if (extra.isNotEmpty) 'extra': extra,
      // Only present when the project actually carries a signature.
      'author_sign': ?authorSign,
    };
  }
}

import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import '../core/utils/logger.dart';
import 'webdav_client.dart';

/// Which cloud backend the workspace syncs to.
enum SyncProviderKind { msc, generic }

/// Cloud sync configuration.
///
/// - MSC 云同步: the official MSC Cloud Dashboard — the user provides the
///   server base URL, their login email, and an API token (created on the
///   account page). Files go through its WebDAV endpoint `{server}/dav/`.
/// - 第三方同步: any WebDAV-compatible server (Nextcloud, 坚果云, …).
class SyncConfig {
  final SyncProviderKind kind;

  /// MSC: server base URL, e.g. `https://cloud.example.com` (no `/dav/`).
  /// Generic: full WebDAV root, e.g. `https://dav.jianguoyun.com/dav/`.
  final String serverUrl;

  /// MSC: login email. Generic: WebDAV username.
  final String username;

  /// MSC: API token (shown once at creation). Generic: password / app token.
  final String secret;

  const SyncConfig({
    required this.kind,
    required this.serverUrl,
    required this.username,
    required this.secret,
  });

  bool get isComplete =>
      serverUrl.trim().isNotEmpty &&
      username.trim().isNotEmpty &&
      secret.trim().isNotEmpty;

  /// The WebDAV root used for sync traffic.
  String get webDavBaseUrl {
    final url = serverUrl.trim();
    switch (kind) {
      case SyncProviderKind.msc:
        // MSC exposes WebDAV at {server}/dav/ (cloud-sync.md §8).
        return url.endsWith('/') ? '${url}dav/' : '$url/dav/';
      case SyncProviderKind.generic:
        return url;
    }
  }

  /// Builds a client for the current configuration.
  WebDavClient createClient() => WebDavClient(
        baseUrl: webDavBaseUrl,
        username: username.trim(),
        password: secret,
      );

  Map<String, dynamic> toJson() => {
        'kind': kind.name,
        'server_url': serverUrl,
        'username': username,
        'secret': secret,
      };

  static SyncConfig? fromJson(Map<String, dynamic> json) {
    try {
      return SyncConfig(
        kind: SyncProviderKind.values.byName(json['kind'] as String),
        serverUrl: json['server_url'] as String? ?? '',
        username: json['username'] as String? ?? '',
        secret: json['secret'] as String? ?? '',
      );
    } catch (_) {
      return null;
    }
  }
}

/// Persists the sync configuration via shared_preferences.
///
/// The secret is stored locally in plain text, same as the app's other
/// settings; MSC API tokens can be revoked from the account page at any time.
class SyncConfigStore {
  static const _key = 'cloud_sync_config';

  static Future<SyncConfig?> load() async {
    try {
      final prefs = await SharedPreferences.getInstance();
      final raw = prefs.getString(_key);
      if (raw == null || raw.isEmpty) return null;
      return SyncConfig.fromJson(jsonDecode(raw) as Map<String, dynamic>);
    } catch (e) {
      AppLogger.w('Could not load sync config: $e');
      return null;
    }
  }

  static Future<void> save(SyncConfig config) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_key, jsonEncode(config.toJson()));
  }

  static Future<void> clear() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove(_key);
  }
}

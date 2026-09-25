import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

/// Minimal WebDAV client sufficient for project sync.
///
/// Implements only what sync needs: HEAD (exists/metadata), GET, PUT, DELETE,
/// MKCOL. Authentication is HTTP Basic. No XML parsing — remote state is
/// tracked through a sidecar `.meta.json` file instead of PROPFIND, which
/// keeps this client compatible with even barebones WebDAV servers.
///
/// Bodies are buffered in memory: project archives are a few MB at most, and
/// buffering sidesteps connection-lifecycle bugs.
class WebDavClient {
  final Uri baseUri;
  final String? username;
  final String? password;
  final Duration timeout;

  WebDavClient({
    required String baseUrl,
    this.username,
    this.password,
    this.timeout = const Duration(seconds: 30),
  }) : baseUri = _normalizeBase(baseUrl);

  static Uri _normalizeBase(String baseUrl) {
    var url = baseUrl.trim();
    if (!url.startsWith('http://') && !url.startsWith('https://')) {
      url = 'https://$url';
    }
    if (!url.endsWith('/')) url = '$url/';
    return Uri.parse(url);
  }

  /// Resolves a relative remote path against the base URL, encoding each
  /// segment so CJK project names survive the wire.
  Uri _resolve(String remotePath) {
    final rel = remotePath.startsWith('/') ? remotePath.substring(1) : remotePath;
    final encoded = rel.split('/').map(Uri.encodeComponent).join('/');
    return baseUri.resolve(encoded);
  }

  void _applyAuth(HttpClientRequest request) {
    final user = username;
    if (user == null || user.isEmpty) return;
    final credentials = base64Encode(utf8.encode('$user:${password ?? ''}'));
    request.headers.set(HttpHeaders.authorizationHeader, 'Basic $credentials');
  }

  /// Sends a request and buffers the whole response.
  Future<WebDavResponse> _send(
    String method,
    Uri uri, {
    List<int>? body,
  }) async {
    final client = HttpClient();
    client.connectionTimeout = timeout;
    client.idleTimeout = timeout;
    try {
      final request = await client.openUrl(method, uri).timeout(timeout);
      _applyAuth(request);
      if (body != null) {
        request.headers.contentType = ContentType.binary;
        request.contentLength = body.length;
        request.add(body);
      } else {
        request.contentLength = 0;
      }
      final response = await request.close().timeout(timeout);
      final bytes = await response.fold<BytesBuilder>(
        BytesBuilder(copy: false),
        (b, chunk) => b..add(chunk),
      );
      final result = WebDavResponse(
        statusCode: response.statusCode,
        headers: response.headers,
        body: bytes.takeBytes(),
      );
      if (result.statusCode >= 400) {
        throw WebDavException(result.statusCode, _describe(result.statusCode));
      }
      return result;
    } catch (e) {
      if (e is WebDavException) rethrow;
      throw WebDavException(-1, '$e');
    } finally {
      client.close();
    }
  }

  static String _describe(int status) {
    return switch (status) {
      401 => '认证失败（请检查用户名/令牌）',
      403 => '没有权限或配额已满',
      404 => '远端路径不存在',
      405 => '方法不允许（目录可能已存在）',
      409 => '父目录不存在',
      507 => '远端空间不足',
      _ => 'HTTP $status',
    };
  }

  /// Remote metadata for a path (null when it does not exist).
  Future<RemoteStat?> stat(String remotePath) async {
    try {
      final response = await _send('HEAD', _resolve(remotePath));
      final length = int.tryParse(
          response.headers.value(HttpHeaders.contentLengthHeader) ?? '');
      final modified =
          response.headers.value(HttpHeaders.lastModifiedHeader);
      return RemoteStat(
        size: length ?? 0,
        lastModified: modified != null ? HttpDate.parse(modified) : null,
        etag: response.headers.value(HttpHeaders.etagHeader),
      );
    } on WebDavException catch (e) {
      if (e.statusCode == 404) return null;
      rethrow;
    }
  }

  Future<bool> exists(String remotePath) async =>
      await stat(remotePath) != null;

  Future<Uint8List> download(String remotePath) async {
    final response = await _send('GET', _resolve(remotePath));
    return response.body;
  }

  /// Uploads bytes, creating parent collections as needed.
  Future<void> upload(String remotePath, List<int> bytes) async {
    await _ensureParents(remotePath);
    await _send('PUT', _resolve(remotePath), body: bytes);
  }

  Future<void> delete(String remotePath) async {
    try {
      await _send('DELETE', _resolve(remotePath));
    } on WebDavException catch (e) {
      if (e.statusCode != 404) rethrow;
    }
  }

  /// Creates the parent collections of [remotePath] when missing.
  ///
  /// MSC and most WebDAV servers return 409 when PUT-ing into a missing
  /// collection, so the chain is built top-down with MKCOL.
  Future<void> _ensureParents(String remotePath) async {
    final segments = remotePath.split('/')..removeWhere((s) => s.isEmpty);
    if (segments.length <= 1) return;
    var current = '';
    for (var i = 0; i < segments.length - 1; i++) {
      current = '$current${segments[i]}/';
      try {
        await _send('MKCOL', _resolve(current));
      } on WebDavException catch (e) {
        // 405/409: the collection already exists — keep walking down.
        if (e.statusCode != 405 && e.statusCode != 409) rethrow;
      }
    }
  }

  /// Verifies credentials and reachability (HEAD on the base collection).
  Future<void> testConnection() async {
    await _send('HEAD', baseUri);
  }
}

/// A fully-buffered WebDAV response.
class WebDavResponse {
  final int statusCode;
  final HttpHeaders headers;
  final Uint8List body;
  const WebDavResponse({
    required this.statusCode,
    required this.headers,
    required this.body,
  });
}

/// Remote file metadata from a HEAD response.
class RemoteStat {
  final int size;
  final DateTime? lastModified;
  final String? etag;
  const RemoteStat({required this.size, this.lastModified, this.etag});
}

class WebDavException implements Exception {
  /// HTTP status code, or -1 for a network-level failure.
  final int statusCode;
  final String message;
  WebDavException(this.statusCode, this.message);

  bool get isNetworkError => statusCode == -1;
  bool get isAuthError => statusCode == 401 || statusCode == 403;

  @override
  String toString() => 'WebDavException($statusCode): $message';
}

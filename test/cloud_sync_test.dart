import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:zenith_audio/models/note.dart';
import 'package:zenith_audio/models/project.dart';
import 'package:zenith_audio/models/track.dart';
import 'package:zenith_audio/services/cloud_sync_config.dart';
import 'package:zenith_audio/services/cloud_sync_service.dart';
import 'package:zenith_audio/services/project_serializer_io.dart';
import 'package:zenith_audio/services/webdav_client.dart';

/// In-memory WebDAV server used to exercise the real sync engine over HTTP.
class FakeWebDavServer {
  late HttpServer _server;
  final Map<String, List<int>> files = {};
  final Map<String, DateTime> modified = {};
  String? expectedAuth;

  int get port => _server.port;
  bool _stopped = false;

  Future<void> start() async {
    _server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    _server.listen(_handle);
  }

  Future<void> stop() async {
    if (_stopped) return;
    _stopped = true;
    await _server.close(force: true);
  }

  String get baseUrl => 'http://127.0.0.1:$port/dav/';

  void _handle(HttpRequest request) {
    // Basic-auth check.
    if (expectedAuth != null) {
      final header = request.headers.value(HttpHeaders.authorizationHeader);
      if (header != 'Basic ${base64Encode(utf8.encode(expectedAuth!))}') {
        request.response.statusCode = 401;
        request.response.close();
        return;
      }
    }

    // Strip the /dav/ prefix back off.
    final rawPath = Uri.decodeComponent(request.uri.path);
    final path = rawPath.startsWith('/dav/')
        ? rawPath.substring('/dav/'.length)
        : rawPath;

    switch (request.method) {
      case 'HEAD':
        _head(request, path);
      case 'GET':
        _get(request, path);
      case 'PUT':
        _put(request, path);
      case 'DELETE':
        _delete(request, path);
      case 'MKCOL':
        _mkcol(request, path);
      default:
        request.response.statusCode = 405;
        request.response.close();
    }
  }

  void _head(HttpRequest request, String path) {
    final bytes = files[path];
    // Collections (empty path = the dav root, or trailing /) exist virtually.
    final isCollection = path.isEmpty || path.endsWith('/');
    if (bytes == null && !isCollection) {
      request.response.statusCode = 404;
    } else {
      request.response.headers.contentLength = bytes?.length ?? 0;
      request.response.headers.set(HttpHeaders.lastModifiedHeader,
          HttpDate.format(modified[path] ?? DateTime.now().toUtc()));
    }
    request.response.close();
  }

  void _get(HttpRequest request, String path) {
    final bytes = files[path];
    if (bytes == null) {
      request.response.statusCode = 404;
      request.response.close();
      return;
    }
    request.response.add(bytes);
    request.response.close();
  }

  Future<void> _put(HttpRequest request, String path) async {
    // Enforce parent-directory existence like MSC does (409 otherwise).
    final parent = path.contains('/')
        ? path.substring(0, path.lastIndexOf('/') + 1)
        : '';
    if (parent.isNotEmpty && !files.containsKey(parent)) {
      request.response.statusCode = 409;
      request.response.close();
      return;
    }
    final builder = BytesBuilder(copy: false);
    await for (final chunk in request) {
      builder.add(chunk);
    }
    files[path] = builder.takeBytes();
    modified[path] = DateTime.now().toUtc();
    request.response.statusCode = 201;
    request.response.close();
  }

  void _delete(HttpRequest request, String path) {
    if (files.remove(path) == null) {
      request.response.statusCode = 404;
    } else {
      modified.remove(path);
      request.response.statusCode = 204;
    }
    request.response.close();
  }

  void _mkcol(HttpRequest request, String path) {
    final normalized = path.endsWith('/') ? path : '$path/';
    if (files.containsKey(normalized)) {
      request.response.statusCode = 405; // already exists
    } else {
      files[normalized] = [];
      request.response.statusCode = 201;
    }
    request.response.close();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  // flutter_test mocks HttpClient to answer 400 for everything; restore the
  // real network stack so the fake WebDAV server can actually be talked to.
  HttpOverrides.global = null;

  late FakeWebDavServer server;
  late Directory workspace;

  setUp(() async {
    server = FakeWebDavServer();
    await server.start();
    workspace = await Directory.systemTemp.createTemp('sync_ws_');
  });

  tearDown(() async {
    await server.stop();
    if (await workspace.exists()) await workspace.delete(recursive: true);
  });

  SyncConfig mscConfig() => SyncConfig(
        kind: SyncProviderKind.msc,
        serverUrl: 'http://127.0.0.1:${server.port}',
        username: 'me@dev.local',
        secret: 'token123',
      );

  SyncConfig genericConfig() => SyncConfig(
        kind: SyncProviderKind.generic,
        serverUrl: server.baseUrl,
        username: 'user',
        secret: 'pass',
      );

  Future<Directory> makeProject(String slug,
      {int pitch = 60, String name = 'Sync Test'}) async {
    final dir = Directory('${workspace.path}/$slug');
    final project = Project(
      id: 'id-$slug',
      name: name,
      bpm: 120,
      tracks: [
        Track(
          id: 't1',
          name: 'Lead',
          type: TrackType.synth,
          instrumentName: 'saw-lead',
          notes: [Note(pitch: pitch, startTime: 0, duration: 1)],
        ),
      ],
    );
    await const ProjectSerializer().writeProjectDirectory(project, dir);
    return dir;
  }

  group('SyncConfig', () {
    test('MSC derives its WebDAV endpoint from the server URL', () {
      final config = SyncConfig(
        kind: SyncProviderKind.msc,
        serverUrl: 'https://cloud.example.com',
        username: 'a@b.c',
        secret: 't',
      );
      expect(config.webDavBaseUrl, 'https://cloud.example.com/dav/');

      final trailing = SyncConfig(
        kind: SyncProviderKind.msc,
        serverUrl: 'https://cloud.example.com/',
        username: 'a@b.c',
        secret: 't',
      );
      expect(trailing.webDavBaseUrl, 'https://cloud.example.com/dav/');
    });

    test('generic config uses the WebDAV URL verbatim', () {
      final config = SyncConfig(
        kind: SyncProviderKind.generic,
        serverUrl: 'https://dav.example.com/dav/',
        username: 'u',
        secret: 'p',
      );
      expect(config.webDavBaseUrl, 'https://dav.example.com/dav/');
    });

    test('config round-trips through JSON', () {
      final config = mscConfig();
      final restored = SyncConfig.fromJson(config.toJson());
      expect(restored, isNotNull);
      expect(restored!.kind, SyncProviderKind.msc);
      expect(restored.serverUrl, config.serverUrl);
      expect(restored.username, config.username);
      expect(restored.secret, config.secret);
    });

    test('isComplete requires all three fields', () {
      expect(mscConfig().isComplete, isTrue);
      expect(
        SyncConfig(
          kind: SyncProviderKind.msc,
          serverUrl: '',
          username: 'a@b.c',
          secret: 't',
        ).isComplete,
        isFalse,
      );
    });
  });

  group('WebDavClient against a fake server', () {
    test('upload → exists → download → delete round trip', () async {
      final client = WebDavClient(
        baseUrl: server.baseUrl,
        username: 'u',
        password: 'p',
      );

      expect(await client.exists('zenith-audio/a.txt'), isFalse);
      await client.upload('zenith-audio/a.txt', utf8.encode('hello'));
      expect(await client.exists('zenith-audio/a.txt'), isTrue);

      final bytes = await client.download('zenith-audio/a.txt');
      expect(utf8.decode(bytes), 'hello');

      final stat = await client.stat('zenith-audio/a.txt');
      expect(stat, isNotNull);
      expect(stat!.size, 5);

      await client.delete('zenith-audio/a.txt');
      expect(await client.exists('zenith-audio/a.txt'), isFalse);
    });

    test('upload creates missing parent collections', () async {
      final client = WebDavClient(baseUrl: server.baseUrl);
      // zenith-audio/ does not exist yet — MKCOL must be issued first.
      await client.upload('zenith-audio/deep/nested/f.bin', [1, 2, 3]);
      expect(server.files.containsKey('zenith-audio/'), isTrue);
      expect(server.files.containsKey('zenith-audio/deep/'), isTrue);
      expect(server.files['zenith-audio/deep/nested/f.bin'], [1, 2, 3]);
    });

    test('CJK path segments are encoded and decoded correctly', () async {
      final client = WebDavClient(baseUrl: server.baseUrl);
      await client.upload('zenith-audio/我的工程.zaproj', [9, 9]);
      expect(server.files.containsKey('zenith-audio/我的工程.zaproj'), isTrue);
      final bytes = await client.download('zenith-audio/我的工程.zaproj');
      expect(bytes, [9, 9]);
    });

    test('auth header is sent and a wrong token gets 401', () async {
      server.expectedAuth = 'me@dev.local:good-token';
      final good = WebDavClient(
        baseUrl: server.baseUrl,
        username: 'me@dev.local',
        password: 'good-token',
      );
      await good.testConnection(); // must not throw

      final bad = WebDavClient(
        baseUrl: server.baseUrl,
        username: 'me@dev.local',
        password: 'wrong',
      );
      expect(
        () => bad.testConnection(),
        throwsA(isA<WebDavException>()
            .having((e) => e.statusCode, 'status', 401)
            .having((e) => e.isAuthError, 'auth', true)),
      );
    });

    test('404 stat returns null instead of throwing', () async {
      final client = WebDavClient(baseUrl: server.baseUrl);
      expect(await client.stat('nope/missing.bin'), isNull);
    });
  });

  group('CloudSyncService status machine', () {
    test('notSynced → push → synced → local edit → ahead', () async {
      final service = CloudSyncService(genericConfig());
      final dir = await makeProject('song-a');
      var record = const ProjectSyncRecord(slug: 'song-a');

      // 1. Nothing in the cloud yet.
      var check = await service.check(dir, 'song-a', record);
      expect(check.status, SyncStatus.notSynced);

      // 2. Push.
      record = await service.push(dir, 'song-a', record);
      expect(record.status, SyncStatus.synced);
      expect(record.lastLocalHash, isNotEmpty);
      expect(record.lastRemoteHash, record.lastLocalHash);
      expect(server.files.containsKey('zenith-audio/song-a.zaproj'), isTrue);
      expect(server.files.containsKey('zenith-audio/song-a.meta.json'),
          isTrue);

      // 3. Unchanged → still synced.
      check = await service.check(dir, 'song-a', record);
      expect(check.status, SyncStatus.synced);

      // 4. Local edit → ahead.
      await File('${dir.path}/spec/extra.md').writeAsString('changed');
      check = await service.check(dir, 'song-a', record);
      expect(check.status, SyncStatus.ahead);
    });

    test('remote change with clean local → behind', () async {
      final service = CloudSyncService(genericConfig());
      final dir = await makeProject('song-b');
      var record = const ProjectSyncRecord(slug: 'song-b');
      record = await service.push(dir, 'song-b', record);

      // Another device pushes a different version.
      final other = await makeProject('song-b-tmp', pitch: 72);
      await CloudSyncService(genericConfig())
          .push(other, 'song-b', const ProjectSyncRecord(slug: 'song-b'));

      final check = await service.check(dir, 'song-b', record);
      expect(check.status, SyncStatus.behind);
    });

    test('both sides changed → conflict', () async {
      final service = CloudSyncService(genericConfig());
      final dir = await makeProject('song-c');
      var record = const ProjectSyncRecord(slug: 'song-c');
      record = await service.push(dir, 'song-c', record);

      // Remote change from another device.
      final other = await makeProject('song-c-tmp', pitch: 71);
      await CloudSyncService(genericConfig())
          .push(other, 'song-c', const ProjectSyncRecord(slug: 'song-c'));

      // Local change too.
      await File('${dir.path}/spec/local.md').writeAsString('local edit');

      final check = await service.check(dir, 'song-c', record);
      expect(check.status, SyncStatus.conflict);
      expect(check.localHash, isNot(check.remoteHash));
    });

    test('pull → unpack → content survives a round trip', () async {
      final service = CloudSyncService(genericConfig());
      final source = await makeProject('song-d', pitch: 64);
      await service.push(source, 'song-d', const ProjectSyncRecord(slug: 'song-d'));

      // Simulate a second machine pulling the project.
      final target = Directory('${workspace.path}/song-d-restored');
      final bytes = await service.pull('song-d');
      await const ProjectSerializer().unpackProjectArchive(bytes, target);

      expect(File('${target.path}/info.json').existsSync(), isTrue);
      expect(File('${target.path}/spec/project.json').existsSync(), isTrue);

      final read =
          await const ProjectSerializer().readProjectDirectory(target);
      expect(read, isNotNull);
      expect(read!.project.name, 'Sync Test');
      expect(read.project.tracks.single.notes.single.pitch, 64);

      // And the restored copy hashes identically to the source.
      expect(await service.contentHash(target),
          await service.contentHash(source));
    });

    test('push failure is recorded as failed with the error text', () async {
      final config = genericConfig(); // capture the port before stopping
      await server.stop(); // kill the server → network error
      final service = CloudSyncService(config);
      final dir = await makeProject('song-e');
      final record = await service.push(
          dir, 'song-e', const ProjectSyncRecord(slug: 'song-e'));
      expect(record.status, SyncStatus.failed);
      expect(record.lastError, isNotNull);
    });

    test('contentHash ignores work/ and dist/', () async {
      final service = CloudSyncService(genericConfig());
      final dir = await makeProject('song-f');
      final before = await service.contentHash(dir);
      await File('${dir.path}/work/scratch.tmp').writeAsString('x');
      await File('${dir.path}/dist/old.zaproj').writeAsString('x');
      await File('${dir.path}/.hidden').writeAsString('x');
      expect(await service.contentHash(dir), before);
    });

    test('contentHash is stable across identical packs', () async {
      final service = CloudSyncService(genericConfig());
      final dir = await makeProject('song-g');
      expect(await service.contentHash(dir), await service.contentHash(dir));
    });
  });

  group('MSC-shaped flow', () {
    test('MSC config works end-to-end over /dav/', () async {
      server.expectedAuth = 'me@dev.local:token123';
      final service = CloudSyncService(mscConfig());
      await service.testConnection();

      final dir = await makeProject('msc-song');
      final record = await service.push(
          dir, 'msc-song', const ProjectSyncRecord(slug: 'msc-song'));
      expect(record.status, SyncStatus.synced);
      expect(server.files.keys.any((k) => k.endsWith('msc-song.zaproj')),
          isTrue);
    });
  });
}

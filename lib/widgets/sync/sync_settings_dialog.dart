import 'package:easy_localization/easy_localization.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../providers/sync_provider.dart';
import '../../services/cloud_sync_config.dart';
import '../../services/webdav_client.dart';

/// Cloud sync settings: choose the provider (MSC 云同步 / 第三方同步) and
/// enter the connection details, then test & save.
class SyncSettingsDialog extends ConsumerStatefulWidget {
  const SyncSettingsDialog({super.key});

  static Future<void> show(BuildContext context) {
    return showDialog(
      context: context,
      builder: (_) => const SyncSettingsDialog(),
    );
  }

  @override
  ConsumerState<SyncSettingsDialog> createState() => _SyncSettingsDialogState();
}

class _SyncSettingsDialogState extends ConsumerState<SyncSettingsDialog> {
  final _serverCtrl = TextEditingController();
  final _userCtrl = TextEditingController();
  final _secretCtrl = TextEditingController();

  SyncProviderKind _kind = SyncProviderKind.msc;
  bool _testing = false;
  bool _saving = false;
  String? _testResult; // null = not tested; '' = ok; otherwise error text
  bool _obscure = true;

  @override
  void initState() {
    super.initState();
    final existing = ref.read(syncConfigProvider).value;
    if (existing != null) {
      _kind = existing.kind;
      _serverCtrl.text = existing.serverUrl;
      _userCtrl.text = existing.username;
      _secretCtrl.text = existing.secret;
    }
  }

  @override
  void dispose() {
    _serverCtrl.dispose();
    _userCtrl.dispose();
    _secretCtrl.dispose();
    super.dispose();
  }

  SyncConfig get _current => SyncConfig(
        kind: _kind,
        serverUrl: _serverCtrl.text,
        username: _userCtrl.text,
        secret: _secretCtrl.text,
      );

  Future<void> _test() async {
    setState(() {
      _testing = true;
      _testResult = null;
    });
    try {
      await ref.read(syncConfigProvider.notifier).testConnection(_current);
      setState(() => _testResult = '');
    } on WebDavException catch (e) {
      setState(() => _testResult = e.message);
    } catch (e) {
      setState(() => _testResult = '$e');
    } finally {
      if (mounted) setState(() => _testing = false);
    }
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    try {
      await ref.read(syncConfigProvider.notifier).save(_current);
      if (mounted) Navigator.of(context).pop();
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  Future<void> _disconnect() async {
    await ref.read(syncConfigProvider.notifier).clear();
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final isMsc = _kind == SyncProviderKind.msc;
    final connected = ref.watch(syncConfigProvider).value != null;

    return AlertDialog(
      title: Text('sync.title'.tr()),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              // ── Provider selection ──
              SegmentedButton<SyncProviderKind>(
                segments: [
                  ButtonSegment(
                    value: SyncProviderKind.msc,
                    icon: const Icon(Icons.cloud_outlined, size: 16),
                    label: Text('sync.provider.msc'.tr()),
                  ),
                  ButtonSegment(
                    value: SyncProviderKind.generic,
                    icon: const Icon(Icons.public_rounded, size: 16),
                    label: Text('sync.provider.generic'.tr()),
                  ),
                ],
                selected: {_kind},
                onSelectionChanged: (s) => setState(() {
                  _kind = s.first;
                  _testResult = null;
                }),
              ),
              const SizedBox(height: 14),

              // ── Connection fields ──
              TextField(
                controller: _serverCtrl,
                decoration: InputDecoration(
                  labelText: isMsc
                      ? 'sync.field.server'.tr()
                      : 'sync.field.webdavUrl'.tr(),
                  hintText: isMsc
                      ? 'https://cloud.example.com'
                      : 'https://dav.example.com/remote.php/dav/',
                  isDense: true,
                ),
                onChanged: (_) => setState(() => _testResult = null),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _userCtrl,
                decoration: InputDecoration(
                  labelText: isMsc
                      ? 'sync.field.email'.tr()
                      : 'sync.field.username'.tr(),
                  isDense: true,
                ),
                onChanged: (_) => setState(() => _testResult = null),
              ),
              const SizedBox(height: 10),
              TextField(
                controller: _secretCtrl,
                obscureText: _obscure,
                decoration: InputDecoration(
                  labelText: isMsc
                      ? 'sync.field.token'.tr()
                      : 'sync.field.password'.tr(),
                  isDense: true,
                  suffixIcon: IconButton(
                    icon: Icon(
                      _obscure ? Icons.visibility_off : Icons.visibility,
                      size: 16,
                    ),
                    onPressed: () => setState(() => _obscure = !_obscure),
                  ),
                ),
                onChanged: (_) => setState(() => _testResult = null),
              ),
              const SizedBox(height: 8),

              // MSC: where to get the token.
              if (isMsc)
                Text(
                  'sync.mscHint'.tr(),
                  style: TextStyle(
                    color: cs.onSurfaceVariant,
                    fontSize: 11,
                    height: 1.5,
                  ),
                ),
              const SizedBox(height: 10),

              // ── Test result ──
              Row(
                children: [
                  Expanded(
                    child: _testResult == null
                        ? const SizedBox.shrink()
                        : Row(
                            children: [
                              Icon(
                                _testResult!.isEmpty
                                    ? Icons.check_circle_outline
                                    : Icons.error_outline,
                                size: 14,
                                color: _testResult!.isEmpty
                                    ? Colors.greenAccent
                                    : cs.error,
                              ),
                              const SizedBox(width: 6),
                              Expanded(
                                child: Text(
                                  _testResult!.isEmpty
                                      ? 'sync.testOk'.tr()
                                      : _testResult!,
                                  style: TextStyle(
                                    fontSize: 11,
                                    color: _testResult!.isEmpty
                                        ? Colors.greenAccent
                                        : cs.error,
                                  ),
                                ),
                              ),
                            ],
                          ),
                  ),
                  TextButton.icon(
                    onPressed: _testing || !_current.isComplete ? null : _test,
                    icon: _testing
                        ? const SizedBox(
                            width: 12,
                            height: 12,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.wifi_tethering_rounded, size: 14),
                    label: Text('sync.test'.tr()),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
      actions: [
        if (connected)
          TextButton(
            onPressed: _disconnect,
            child: Text('sync.disconnect'.tr(),
                style: TextStyle(color: cs.error)),
          ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text('common.cancel'.tr()),
        ),
        FilledButton(
          onPressed: _saving || !_current.isComplete ? null : _save,
          child: Text('common.save'.tr()),
        ),
      ],
    );
  }
}

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:easy_localization/easy_localization.dart';
import '../core/constants/app_constants.dart';
import '../core/constants/app_config.dart';
import '../core/utils/theme_colors.dart';
import '../core/utils/responsive_utils.dart';
import '../core/utils/platform_utils.dart';
import '../providers/settings_provider.dart';
import '../providers/project_provider.dart';

enum SettingsTab { general, playback, pianoRoll, about }

final settingsTabProvider = StateProvider<SettingsTab>((ref) => SettingsTab.general);

class SettingsPage extends ConsumerWidget {
  const SettingsPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final settings = ref.watch(settingsProvider);
    final tab = ref.watch(settingsTabProvider);
    final isMobile = getScreenSize(context) == ScreenSize.mobile;

    return Scaffold(
      appBar: AppBar(
        title: Text('settings.title'.tr()),
        leading: IconButton(
          icon: const Icon(Icons.arrow_back),
          onPressed: () => Navigator.of(context).pop(),
        ),
      ),
      body: SafeArea(
        bottom: isMobile,
        child: isMobile
            ? _buildMobileLayout(context, ref, settings, tab)
            : _buildDesktopLayout(context, ref, settings, tab),
      ),
    );
  }

  Widget _buildDesktopLayout(BuildContext context, WidgetRef ref, SettingsState settings, SettingsTab tab) {
    return Row(
      children: [
        SizedBox(
          width: 200,
          child: _Sidebar(currentTab: tab, onTabChanged: (t) => ref.read(settingsTabProvider.notifier).state = t),
        ),
        Container(width: 1, color: Theme.of(context).dividerColor),
        Expanded(
          child: _buildContent(context, ref, settings, tab),
        ),
      ],
    );
  }

  Widget _buildMobileLayout(BuildContext context, WidgetRef ref, SettingsState settings, SettingsTab tab) {
    return Column(
      children: [
        SizedBox(
          height: 48,
          child: _MobileTabBar(currentTab: tab, onTabChanged: (t) => ref.read(settingsTabProvider.notifier).state = t),
        ),
        Container(height: 1, color: Theme.of(context).dividerColor),
        Expanded(
          child: _buildContent(context, ref, settings, tab),
        ),
      ],
    );
  }

  Widget _buildContent(BuildContext context, WidgetRef ref, SettingsState settings, SettingsTab tab) {
    switch (tab) {
      case SettingsTab.general:
        return _GeneralContent(settings: settings, ref: ref);
      case SettingsTab.playback:
        return _PlaybackContent(settings: settings, ref: ref);
      case SettingsTab.pianoRoll:
        return _PianoRollSettingsContent(settings: settings, ref: ref);
      case SettingsTab.about:
        return _AboutContent();
    }
  }
}

// ──────────────────────────── Sidebar ────────────────────────────

class _Sidebar extends StatelessWidget {
  final SettingsTab currentTab;
  final ValueChanged<SettingsTab> onTabChanged;

  const _Sidebar({required this.currentTab, required this.onTabChanged});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      color: cs.surface,
      child: Column(
        children: [
          const SizedBox(height: 8),
          _SidebarItem(
            icon: Icons.tune_outlined,
            label: 'settings.general'.tr(),
            selected: currentTab == SettingsTab.general,
            onTap: () => onTabChanged(SettingsTab.general),
          ),
          _SidebarItem(
            icon: Icons.play_circle_outline,
            label: 'settings.playbackSection'.tr(),
            selected: currentTab == SettingsTab.playback,
            onTap: () => onTabChanged(SettingsTab.playback),
          ),
          _SidebarItem(
            icon: Icons.grid_view_outlined,
            label: 'settings.pianoRollSection'.tr(),
            selected: currentTab == SettingsTab.pianoRoll,
            onTap: () => onTabChanged(SettingsTab.pianoRoll),
          ),
          const Spacer(),
          _SidebarItem(
            icon: Icons.info_outline,
            label: 'settings.aboutSection'.tr(),
            selected: currentTab == SettingsTab.about,
            onTap: () => onTabChanged(SettingsTab.about),
          ),
          const SizedBox(height: 8),
        ],
      ),
    );
  }
}

class _SidebarItem extends StatelessWidget {
  final IconData icon;
  final String label;
  final bool selected;
  final VoidCallback onTap;

  const _SidebarItem({required this.icon, required this.label, required this.selected, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      child: Material(
        color: selected ? cs.primaryContainer : Colors.transparent,
        borderRadius: BorderRadius.circular(8),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(8),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            child: Row(
              children: [
                Icon(icon, size: 18, color: selected ? cs.primary : cs.onSurfaceVariant),
                const SizedBox(width: 12),
                Text(label, style: TextStyle(
                  color: selected ? cs.primary : cs.onSurface,
                  fontSize: 13,
                  fontWeight: selected ? FontWeight.w500 : FontWeight.normal,
                )),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

// ──────────────────────────── Mobile TabBar ────────────────────────────

class _MobileTabBar extends StatelessWidget {
  final SettingsTab currentTab;
  final ValueChanged<SettingsTab> onTabChanged;

  const _MobileTabBar({required this.currentTab, required this.onTabChanged});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Container(
      color: cs.surface,
      child: Row(
        children: [
          _MobileTabItem(
            icon: Icons.tune_outlined,
            label: 'settings.general'.tr(),
            selected: currentTab == SettingsTab.general,
            onTap: () => onTabChanged(SettingsTab.general),
          ),
          _MobileTabItem(
            icon: Icons.play_circle_outline,
            label: 'settings.playbackSection'.tr(),
            selected: currentTab == SettingsTab.playback,
            onTap: () => onTabChanged(SettingsTab.playback),
          ),
          _MobileTabItem(
            icon: Icons.grid_view_outlined,
            label: 'settings.pianoRollSection'.tr(),
            selected: currentTab == SettingsTab.pianoRoll,
            onTap: () => onTabChanged(SettingsTab.pianoRoll),
          ),
          _MobileTabItem(
            icon: Icons.info_outline,
            label: 'settings.aboutSection'.tr(),
            selected: currentTab == SettingsTab.about,
            onTap: () => onTabChanged(SettingsTab.about),
          ),
        ],
      ),
    );
  }
}

class _MobileTabItem extends StatelessWidget {
  final IconData icon;
  final String label;
  final bool selected;
  final VoidCallback onTap;

  const _MobileTabItem({required this.icon, required this.label, required this.selected, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Expanded(
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(vertical: 12),
          decoration: BoxDecoration(
            border: Border(
              bottom: BorderSide(color: selected ? cs.primary : Colors.transparent, width: 2),
            ),
          ),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(icon, size: 16, color: selected ? cs.primary : cs.onSurfaceVariant),
              const SizedBox(width: 6),
              Text(label, style: TextStyle(
                color: selected ? cs.primary : cs.onSurfaceVariant,
                fontSize: 13,
                fontWeight: selected ? FontWeight.w500 : FontWeight.normal,
              )),
            ],
          ),
        ),
      ),
    );
  }
}

// ──────────────────────────── General Content ────────────────────────────

class _GeneralContent extends StatelessWidget {
  final SettingsState settings;
  final WidgetRef ref;

  const _GeneralContent({required this.settings, required this.ref});

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        // Theme
        Text('settings.theme'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        InputDecorator(
          decoration: InputDecoration(
            border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
            contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 0),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<ThemeMode>(
              value: settings.themeMode,
              isExpanded: true,
              items: [
                DropdownMenuItem(value: ThemeMode.system, child: Text('settings.theme.system'.tr())),
                DropdownMenuItem(value: ThemeMode.light, child: Text('settings.theme.light'.tr())),
                DropdownMenuItem(value: ThemeMode.dark, child: Text('settings.theme.dark'.tr())),
              ],
              onChanged: (v) {
                if (v != null) ref.read(settingsProvider.notifier).setThemeMode(v);
              },
            ),
          ),
        ),
        const SizedBox(height: 24),
        const SizedBox(height: 24),
        // Auto-Save
        Text('settings.autoSave'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        SwitchListTile(
          title: Text('settings.autoSave.enable'.tr()),
          subtitle: Text('settings.autoSave.enableDesc'.tr()),
          value: settings.autoSaveEnabled,
          onChanged: (v) {
            ref.read(settingsProvider.notifier).setAutoSaveEnabled(v);
            if (v) {
              ref.read(projectProvider.notifier).startAutoSave();
            } else {
              ref.read(projectProvider.notifier).stopAutoSave();
            }
          },
        ),
        if (settings.autoSaveEnabled) ...[
          const SizedBox(height: 8),
          Text('settings.autoSave.interval'.tr(), style: TextStyle(
            color: context.outline, fontSize: 10,
            fontWeight: FontWeight.w600, letterSpacing: 1,
          )),
          const SizedBox(height: 8),
          InputDecorator(
            decoration: InputDecoration(
              border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
              contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 0),
            ),
            child: DropdownButtonHideUnderline(
              child: DropdownButton<int>(
                value: settings.autoSaveIntervalMinutes,
                isExpanded: true,
                items: [
                  DropdownMenuItem(value: 1, child: Text('settings.autoSave.minutes'.tr(namedArgs: {'n': '1'}))),
                  DropdownMenuItem(value: 5, child: Text('settings.autoSave.minutes'.tr(namedArgs: {'n': '5'}))),
                  DropdownMenuItem(value: 15, child: Text('settings.autoSave.minutes'.tr(namedArgs: {'n': '15'}))),
                  DropdownMenuItem(value: 30, child: Text('settings.autoSave.minutes'.tr(namedArgs: {'n': '30'}))),
                ],
                onChanged: (v) {
                  if (v != null) {
                    ref.read(settingsProvider.notifier).setAutoSaveIntervalMinutes(v);
                    ref.read(projectProvider.notifier).startAutoSave();
                  }
                },
              ),
            ),
          ),
        ],
        const SizedBox(height: 24),
        // Editor Mode
        Text('settings.editorMode'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        InputDecorator(
          decoration: InputDecoration(
            border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
            contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 0),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<String>(
              value: settings.editorMode,
              isExpanded: true,
              items: [
                DropdownMenuItem(value: 'fullscreen', child: Text('settings.editorMode.fullscreen'.tr())),
                DropdownMenuItem(value: 'float', child: Text('settings.editorMode.float'.tr())),
              ],
              onChanged: (v) {
                if (v != null) ref.read(settingsProvider.notifier).setEditorMode(v);
              },
            ),
          ),
        ),
        const SizedBox(height: 24),
        // Language
        Text('settings.language'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        InputDecorator(
          decoration: InputDecoration(
            border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
            contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 0),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<Locale>(
              value: context.locale,
              isExpanded: true,
              items: [
                DropdownMenuItem(value: Locale('zh'), child: Text('settings.language.zh'.tr())),
                DropdownMenuItem(value: Locale('en'), child: Text('settings.language.en'.tr())),
              ],
              onChanged: (v) {
                if (v != null) {
                  ref.read(settingsProvider.notifier).setLocale(v);
                  context.setLocale(v);
                }
              },
            ),
          ),
        ),
      ],
    );
  }
}

// ──────────────────────────── Playback Content ────────────────────────────

class _PlaybackContent extends StatelessWidget {
  final SettingsState settings;
  final WidgetRef ref;

  const _PlaybackContent({required this.settings, required this.ref});

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text('settings.playback.autoLoop'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        SwitchListTile(
          title: Text('settings.playback.autoLoopDesc'.tr()),
          value: settings.autoLoop,
          onChanged: (v) => ref.read(settingsProvider.notifier).setAutoLoop(v),
        ),
      ],
    );
  }
}

// ──────────────────────────── Piano Roll Settings ────────────────────────────

class _PianoRollSettingsContent extends StatelessWidget {
  final SettingsState settings;
  final WidgetRef ref;

  const _PianoRollSettingsContent({required this.settings, required this.ref});

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text('settings.pianoRoll.editMode'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        InputDecorator(
          decoration: InputDecoration(
            border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
            contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 0),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<String>(
              value: settings.pianoRollEditMode,
              isExpanded: true,
              items: [
                DropdownMenuItem(value: 'basic', child: Text('settings.pianoRoll.basic'.tr())),
                DropdownMenuItem(value: 'advanced', child: Text('settings.pianoRoll.advanced'.tr())),
              ],
              onChanged: (v) {
                if (v != null) ref.read(settingsProvider.notifier).setPianoRollEditMode(v);
              },
            ),
          ),
        ),
        const SizedBox(height: 24),
        Text('settings.pianoRoll.snapToGrid'.tr(), style: TextStyle(
          color: context.outline, fontSize: 10,
          fontWeight: FontWeight.w600, letterSpacing: 1,
        )),
        const SizedBox(height: 8),
        SwitchListTile(
          title: Text('settings.pianoRoll.snapToGridDesc'.tr()),
          value: settings.snapToGrid,
          onChanged: (v) => ref.read(settingsProvider.notifier).setSnapToGrid(v),
        ),
        if (settings.snapToGrid) ...[
          const SizedBox(height: 16),
          Text('settings.pianoRoll.gridResolution'.tr(), style: TextStyle(
            color: context.outline, fontSize: 10,
            fontWeight: FontWeight.w600, letterSpacing: 1,
          )),
          const SizedBox(height: 8),
          InputDecorator(
            decoration: InputDecoration(
              border: OutlineInputBorder(borderRadius: BorderRadius.circular(8)),
              contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 0),
            ),
            child: DropdownButtonHideUnderline(
              child: DropdownButton<double>(
                value: settings.gridResolution,
                isExpanded: true,
                items: const [
                  DropdownMenuItem(value: 1.0, child: Text('1/4')),
                  DropdownMenuItem(value: 0.5, child: Text('1/8')),
                  DropdownMenuItem(value: 0.25, child: Text('1/16')),
                ],
                onChanged: (v) {
                  if (v != null) ref.read(settingsProvider.notifier).setGridResolution(v);
                },
              ),
            ),
          ),
        ],
      ],
    );
  }
}

class _AboutContent extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        _InfoCard(
          icon: Icons.waves,
          title: 'settings.appInfo'.tr(),
          cs: cs,
          child: _AppInfoContent(),
        ),
        const SizedBox(height: 16),
        _InfoCard(
          icon: Icons.phone_android_outlined,
          title: 'settings.deviceInfo'.tr(),
          cs: cs,
          child: _DeviceInfoContent(),
        ),
        const SizedBox(height: 16),
        _InfoCard(
          icon: Icons.link_outlined,
          title: 'settings.links'.tr(),
          cs: cs,
          child: _LinksContent(),
        ),
      ],
    );
  }
}

class _InfoCard extends StatelessWidget {
  final IconData icon;
  final String title;
  final Widget child;
  final ColorScheme cs;

  const _InfoCard({
    required this.icon,
    required this.title,
    required this.child,
    required this.cs,
  });

  @override
  Widget build(BuildContext context) {
    final isDark = Theme.of(context).brightness == Brightness.dark;
    return Container(
      decoration: BoxDecoration(
        color: isDark ? const Color(0xFF232328) : const Color(0xFFFFFFFF),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: Theme.of(context).dividerColor.withAlpha(128)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 14, 16, 0),
            child: Row(
              children: [
                Icon(icon, size: 18, color: cs.primary),
                const SizedBox(width: 8),
                Text(title, style: TextStyle(
                  color: cs.primary,
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                )),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 14),
            child: child,
          ),
        ],
      ),
    );
  }
}

// ─── App Info ───

class _AppInfoContent extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Column(
      children: [
        Container(
          width: 48, height: 48,
          decoration: BoxDecoration(
            color: cs.primary,
            borderRadius: BorderRadius.circular(10),
          ),
          child: const Icon(Icons.waves, size: 28, color: Colors.white),
        ),
        const SizedBox(height: 10),
        Text(AppConstants.appName, style: TextStyle(
          fontSize: 16, fontWeight: FontWeight.w700, color: cs.onSurface,
        )),
        const SizedBox(height: 2),
        Text(AppConstants.appNameEn, style: TextStyle(
          fontSize: 10, color: cs.primary, fontWeight: FontWeight.w600, letterSpacing: 1.5,
        )),
        const SizedBox(height: 14),
        _InfoRow(label: 'about.version'.tr(), value: AppConfig.appVersion),
        const SizedBox(height: 6),
        _InfoRow(label: 'about.build'.tr(), value: AppConfig.appBuildNumber),
        const SizedBox(height: 12),
        Text(AppConfig.appCopyright,
          style: TextStyle(fontSize: 11, color: context.outline),
          textAlign: TextAlign.center,
        ),
      ],
    );
  }
}

// ─── Device Info ───

class _DeviceInfoContent extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _InfoRow(label: 'settings.deviceOS'.tr(), value: getOperatingSystem()),
        const SizedBox(height: 8),
        _InfoRow(label: 'settings.deviceName'.tr(), value: getDeviceName()),
        const SizedBox(height: 8),
        _InfoRow(label: 'settings.deviceId'.tr(), value: getDeviceIdentifier()),
      ],
    );
  }
}

// ─── Links ───

class _LinksContent extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Column(
      children: [
        _LinkRow(
          label: 'settings.privacyPolicy'.tr(),
          url: 'https://example.com/privacy',
          cs: cs,
        ),
        const SizedBox(height: 8),
        _LinkRow(
          label: 'settings.repository'.tr(),
          url: 'https://github.com/example/repo',
          cs: cs,
        ),
      ],
    );
  }
}

class _LinkRow extends StatelessWidget {
  final String label;
  final String url;
  final ColorScheme cs;

  const _LinkRow({required this.label, required this.url, required this.cs});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          Text(label, style: TextStyle(fontSize: 13, color: cs.onSurface)),
          const Spacer(),
          Text(url, style: TextStyle(fontSize: 11, color: context.outline)),
          Icon(Icons.open_in_new, size: 14, color: context.outline),
        ],
      ),
    );
  }
}

// ─── Shared Info Row ───

class _InfoRow extends StatelessWidget {
  final String label;
  final String value;
  const _InfoRow({required this.label, required this.value});

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Text('$label: ', style: TextStyle(fontSize: 12, color: context.outline)),
        Text(value, style: TextStyle(fontSize: 12, color: cs.onSurface, fontWeight: FontWeight.w500)),
      ],
    );
  }
}

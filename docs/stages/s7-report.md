# S7 — 插件宿主（Agent-A，SDK 无关部分）

**状态**：🟡 **槽位状态/序列化与搜索路径已落地**；CLAP FFI 加载器待做（需 CLAP SDK，桌面限定）
**日期**：2026-10-06
**计划依据**：`docs/PLAN_DAW_PARITY.md` §3 S7

---

## 1. 结论摘要

S7 要求「插件 ABI 抽象层 + CLAP 宿主」。`PluginHost`/`PluginInstance` 接口在 S0 已定（`lib/plugins/plugin_host.dart`）。本轮完成**不依赖 CLAP SDK**的部分：槽位状态模型、JSON 序列化、搜索路径解析、以及槽位集合的增删改序。

| 验收要求（PLAN §3.S7） | 结果 |
|---|---|
| 插件 ABI 抽象层 | ✅ S0 已有 `PluginHost` 接口；本轮补数据层 |
| **插件状态（预设）随工程序列化** | ✅ `PluginSlotState`（不透明预设 blob，base64）+ `PluginSlotBank` JSON 往返 |
| 插件参数 → 宿主自动化 | ⬜ 需 CLAP 实例；参数桥接属 FFI 加载器 |
| 插件延迟 → PDC | ⬜ 同上 |
| 沙箱化：插件崩溃不拖垮宿主 | ⬜ 需真实加载器 |
| **不使用 VST** | ✅ 只定义 CLAP + 自研 ABI |
| 移动/Web 不加载外部插件 | ✅ 搜索路径与加载都是桌面路径；数据层平台无关 |

**门禁**

| 门禁 | 结果 |
|---|---|
| `flutter analyze` | ✅ **0 error** |
| `flutter test` | ✅ **369 passed**（+17：`plugin_slot_test`）；10 skipped（FFI 需 dylib）；唯一失败为 Windows 专属 `registry_quoting_test.dart` |

---

## 2. 交付内容

| 文件 | 内容 |
|---|---|
| `lib/plugins/plugin_slot.dart` | `PluginSlotState`（pluginId / format / path / bypassed / enabled / **不透明 state blob** / trackId；`toJson`/`fromJson`）、`PluginSlotBank`（按轨分组的槽位：`addSlot`/`removeSlot`/`replaceSlot`/`moveSlot`/`pruneMissing`/JSON 往返）、`PluginSearchPaths.clapPaths`。 |
| `test/plugin_slot_test.dart` | **17 项**。 |

### 2.1 关键设计

- **预设是不透明 blob**：CLAP 插件按自己的格式序列化状态，宿主不应试图理解它。存为 `Uint8List`（JSON 里 base64），任何插件的预设都能往返，而宿主不需要知道「cutoff」是什么。
- **损坏预设降级而非致命**：`fromJson` 遇到坏 base64 只丢该槽位的预设，**不让整个工程打不开**——丢一个预设远好过丢一个工程。
- **`pruneMissing`**：扫描后清掉已卸载插件的槽位，并报告受影响轨道。否则每次播放都对一个不存在的插件报错。
- **未知格式名回退 clap**，不抛异常（版本前向兼容）。
- **越界索引是 no-op 而非抛异常**：UI 上一句过期的点击不能让编辑器崩溃。

---

## 3. 已知缺口 / 后续

1. **CLAP FFI 加载器**：真正 `dlopen` 插件二进制、调用 CLAP 入口、把参数桥接到 S2 参数存储、把延迟桥接到 S4 S5 PDC。**需要 CLAP SDK/头**，本机离线且不安装依赖，故未做。这是 S7 剩下的一块。
2. **插件 UI 嵌入**：对外窗口嵌入，需加载器与平台窗口 API。
3. **沙箱化（子进程隔离，P1）**：需加载器。
4. **工程集成**：✅ **已接入**——`Project.pluginSlots`（可选字段）写入/读回 `lgdf_project_codec` 的 `plugin_slots`；有文档往返测试。旧工程无此键照常解析。

---

## 4. 回滚方式

删除 `lib/plugins/plugin_slot.dart` 与 `test/plugin_slot_test.dart` 即可；不涉及 ABI、不影响其他目录。

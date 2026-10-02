# S9 — 收尾（Agent-A，部分完成）

**状态**：🟡 **CI 守卫与旧工程迁移检测已落地**；其余收尾项待做
**日期**：2026-10-06
**计划依据**：`docs/PLAN_DAW_PARITY.md` §3 S9

---

## 1. 结论摘要

S9 有 6 项：A/B 对比、性能压测、文档、迁移工具、移除 media_kit、构建固化。本轮完成其中**自包含且可验证**的两项——**构建固化（CI 守卫）**与**迁移工具（格式检测与报告）**。

| 验收要求（PLAN §3.S9） | 结果 |
|---|---|
| 移除旧依赖 `media_kit` | ⬜ 未做（需确认无引用；引擎未接线，贸然移除会静音） |
| 构建固化：hook 在六平台 CI 稳定产出原生库 | 🟡 **新增 CI 守卫**：Rust clippy/test/wasm32、Dart analyze/test、六平台 `rustup target add` 由 `dtolnay/rust-toolchain` 承担 |
| 迁移工具：旧 `.zap`/`.zaproj` 一键迁移向导 | 🟡 **格式检测 + 报告 + 打开前确认向导 UI** 完成 |
| A/B 工程对比 | ⬜ 未做 |
| 性能：128 轨压力测试 | ⬜ 未做 |
| 文档：用户手册/快捷键/架构/Rust 指南 | ⬜ 未做 |

**门禁**

| 门禁 | 结果 |
|---|---|
| `flutter analyze` | ✅ **0 error**（105 项既有 info/warning） |
| `flutter test` | ✅ **269 passed**（+8：`project_migration_test`）；8 skipped（FFI 需 dylib）；唯一失败为 Windows 专属 `registry_quoting_test.dart` |
| `cargo clippy --locked -- -D warnings` | ✅ exit 0 |
| `cargo check --target wasm32-unknown-unknown` | ✅ exit 0 |
| `ci.yml` 语法 | ✅ 经 Ruby YAML 解析器校验 |

---

## 2. 交付内容

### 2.1 CI 守卫（`.github/workflows/ci.yml`，新增）

原 `build.yml` 只在 `workflow_dispatch` 手动触发时打包产物，**不跑测试、不装 Rust**——PLAN §4.2/§8.3 明确要求每次推送都要有的门禁缺失。新 `ci.yml` 与 `build.yml` 职责分离（前者拦回归，后者出产物），含 4 个 job：

| job | 内容 |
|---|---|
| `brand-guard` | grep 拦截第三方 DAW 品牌名（§0.2）。**刻意排除 `reason`/`reaper`**——它们是常见英文词，用于守卫会让 CI 长期假红而失去意义。 |
| `flutter` | `flutter analyze`（0 error）+ `flutter test` |
| `rust` | `cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`、`cargo check --target wasm32-unknown-unknown`（P7 硬门禁） |
| `ffi-smoke` | 构建 dylib 后跑 `native_ffi_smoke_test.dart`，真正验证「Dart 能调用 Rust」（其余 Dart 测试在缺库时 skip） |

### 2.2 迁移检测（`lib/services/project_migration.dart`，新增）

`ProjectMigrationService.inspect(bytes, fileName)` → `ProjectProbe`：

- 按**内容**而非扩展名判定三种格式：`.zap`（裸 info.json）/ 旧 LGDF（registry 或 assets 但无 spec）/ LGDF v2.0（有 spec/overview.md 或 spec/project.json）；
- 报告项目名、`document_version`、是否需要迁移、是否可打开、格式标签；
- **永不抛异常**：损坏字节流报 `unknown`，向导的第一职责是清楚地说「这不是工程」；
- `isFromNewerApp()` 在打开前预警「需要更新的应用」。

### 2.3 迁移向导 UI

`lib/widgets/dialogs/project_migration_dialog.dart`：在打开旧工程前显示检测到的格式、
工程名，以及「将升级为当前格式 / 原文件不被修改，仅在保存时写入新格式」。`openProject`
新增可选 `confirmMigration` 回调，工作区「打开工程」与菜单栏「打开工程」两处调用点传入。
非破坏性：源文件不动，迁移在内存中完成，只有用户保存时才写出新格式。

> 说明：检测与向导 UI 已就位，`openProject` 现在会先 probe 再 deserialize；把
> `unknown` 直接拒绝，避免把非工程压缩包当作损坏工程打开。

---

## 3. 已知缺口 / 后续

1. **迁移向导 UI**：检测与报告就位；把它接进打开流程、给用户看「将升级为 v2.0」并确认，属 UI 接线。
2. **`media_kit` 下线**：依赖仍在（`audioEngineProvider` 走旧 `AudioService`）。**在引擎真正接线前移除会静音**，故不动。
3. **A/B 对比、128 轨压测、四类文档**：未做。
4. **CI 未含六平台 native_assets 构建**：`rust` job 只做 `cargo check`（含 wasm32）；各平台完整 `flutter build` 仍在 `build.yml`。计划要求的「每平台 CI job 补 `rustup target add`」由 `dtolnay/rust-toolchain` 的 `targets:` 覆盖，但未在 `build.yml` 的 6 个 job 内逐平台验证 native_assets 链接。

---

## 4. 回滚方式

删除 `.github/workflows/ci.yml`、`lib/services/project_migration.dart`、`test/project_migration_test.dart` 即可；不涉及 ABI、不影响其他目录。

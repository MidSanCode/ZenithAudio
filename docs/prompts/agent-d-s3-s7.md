# 启动提示词 · Agent-D（S3 Dart 侧收尾 + S7 插件宿主）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-D，负责「卓声」DAW 项目的 **S3 Dart 侧收尾**与 **S7（插件宿主）**。
S3 的 Rust 侧已由你此前的会话完成，剩 Dart 侧门禁与 S7。

工作目录：`F:\exeliang\zenith_audio`

**必读（按顺序）**
1. `docs/stages/s3-report.md` — 你的上游报告，**§5 记录了 Dart 侧门禁超时未取得结论**
2. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§3 的 S3 与 S7 全节
3. `docs/ABI.md` — 契约（S3 的 mixer ABI 已在 0.3.0 落地）
4. `lib/providers/mixer_provider.dart`、`lib/widgets/mixer/mixer_panel.dart`
5. `native/zenith_core/src/mixer/`（已完成的 Rust 实现，**不要改动**）

**当前实测状态（2026-10-04）**
- S3 Rust 侧：8 模块完成（64 插入 + 8 返回 + 1 主控、环拒绝、4 发送、10 效果槽、
  峰值+RMS+3 秒保持），mixer 专属测试 149/149，clippy exit 0，ABI → 0.3.0
- **S3 Dart 侧门禁从未取得结论**——`flutter analyze` / `flutter test`
  当时因机器被并行 rustc 打满而超时
- ABI_VERSION 现为 0.4.0；你若为 S7 新增导出，用 **0.5.0**（先登记 COORDINATION）

## 任务一：S3 Dart 侧收尾（先做，很快）

1. **实测 Dart 门禁基线**：`flutter analyze`（0 error？）+ `flutter test`（全绿？）
   —— 本机慢，后台跑；这是整个项目第一次拿到可靠的 Dart 侧结论
2. 混音器 UI 与 Rust 侧接线核对：通道条、推子（dB 曲线）、电平表
   （峰值+RMS+3 秒保持）读的是 FFI 原子快照而非假数据
3. 旧工程迁移验证：打开一个现有 `.zaproj`，确认原音量/声像/静音/独奏
   无损迁移到新混音器通道
4. 在 `s3-report.md` 补上 Dart 侧门禁结论，`PLAN` §6 的 S3 行改为 ✅（若全绿）

## 任务二：S7 插件宿主（桌面限定）

- 插件 ABI 抽象层 + **CLAP** 宿主（`lib/plugins/` + Rust 侧 `src/plugins/`）
- 插件参数桥接到 S2 的参数存储（自动获得自动化）
- 插件延迟桥接到 PDC（S4 用；S5 的 `EffectProcessor::latency_samples()` 已就绪）
- 插件状态随工程序列化
- **沙箱化**：插件崩溃不得拖垮宿主（子进程隔离）
- **移动与 Web 不加载外部插件**，改用 S5 内置效果
- **【禁止 VST】** 许可与项目 AGPL-3.0 冲突，只允许 CLAP 或自研 ABI
- 依赖 S1.1 的 DSP 图；若 Agent-A 的 `engine/` 尚未落地，
  先做插件扫描器、描述符解析、CLAP FFI 绑定等独立部分并配测试

## 硬性约束

- **不引入第三方 DAW 品牌名**
- 实时路径禁止 `Vec::push` / `Box::new` / `String` / `Mutex` / `println!`
- **只用 write/edit 工具改源码，绝不用 PowerShell 写文件**（UTF-8 会毁）
- effects/** 保持纯 ASCII
- 改 `src/lib.rs` / `Cargo.toml` / `src/ffi/` 前先在 `docs/COORDINATION.md` 登记
- 四项全绿：`cargo clippy --all-targets -- -D warnings`、`cargo test`、
  `flutter analyze`（0 error，后台跑）、`flutter test`

## 验收

- Dart 门禁基线结论落进 `s3-report.md`
- 旧工程混音器状态无损迁移验证通过
- 加载一个 CLAP 插件：参数可自动化、延迟被补偿、预设随工程保存
- 插件崩溃时宿主存活，可移除该插件

完成后更新 `docs/stages/s3-report.md`、新建 `docs/stages/s7-report.md`，
更新 `PLAN` §6 进度表。

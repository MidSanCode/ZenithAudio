# 未来步骤总路线图（ROADMAP-FUTURE）

> **本文是全部剩余工作的单一索引。** 按「批次 → 阶段」组织，每个阶段给出：
> 负责会话、依赖、任务清单、验收标准、产出物。
> 详细规格见 `PLAN_DAW_PARITY.md` §3；跨语言契约见 `ABI.md`；所有权登记见 `COORDINATION.md`。
>
> **状态快照**：2026-10-04。已完成：S0 ✅、S1.0 前置项 ✅、S2 ✅、S3-Rust ✅、S5-主体 🟡（852/8）。
> ABI 线性序列：S2=0.2.0 → S3=0.3.0 → S5=0.4.0 → **下一个新增导出 = 0.5.0**。

---

## 总依赖图（剩余部分）

```
批次1（现在，可并行）
  Agent-A: S1.1 Rust 引擎主体 ──────────────┐  ★关键路径
  Agent-C: S5 收尾（8 红测/clippy/SIMD）    │  （与 S1.1 零目录冲突）
                                            │
批次2（可并行）                              │
  Agent-E: S6b 钢琴卷帘 + S6c MIDI          │
  Agent-D: S3 Dart 门禁收尾                 │
                                            │
批次3（可并行）                              │
  Agent-B: S1.5 第一段（WASM 流水线/降级 UI）│
  Agent-F: S8 算法层（stretch/analysis）    │
                                            │
批次4（全部等 S1.1 落地）────────────────────┤
  Agent-A: S4 PDC/冻结/导出/OfflineDriver   │
  Agent-D: S7 CLAP 插件宿主                 │
  Agent-B: S1.5 第二段（worklet_driver 接线）│
  Agent-F: S8 接线（非破坏播放/交叉淡化）    │
  （S2 自动化接线：Agent-A 在 S1.1 内完成）  │
                                            │
批次5                                        │
  Agent-F: S9 收尾（A/B/压测/文档/迁移）     │
  全体: 移除 media_kit（S4 验证通过后才允许）
```

---

## 批次 1（立即开工，两条线并行）

### ① S1.1 — Rust 实时引擎主体 【Agent-A · ★关键路径】

**依赖**：无（S1.0 前置项已全部完成）。
**新建目录**：`src/engine/`、`src/driver/`、`src/transport/`、`src/voice/`（均不存在，即本次任务）。

| # | 任务 | 要点 |
|---|---|---|
| 1 | `engine/graph.rs` | 有向图 + 拓扑排序 + 环检测 |
| 2 | `engine/node.rs` | `DspNode` trait（prepare/process/reset） |
| 3 | `driver/mod.rs` | `AudioDriver` trait（**Agent-B 等这个签名**，先定且文档化） |
| 4 | `driver/cpal_driver.rs` | 桌面/移动：cpal 回调（cfg 非 wasm） |
| 5 | `driver/worklet_driver.rs` | 仅骨架，Agent-B 批次 4 接管 |
| 6 | `transport/` | tick 级采样精确调度（PPQ=960，与 `musical_time.dart` 一致）、无锁事件队列 |
| 7 | `voice/` | 复音分配、采样播放、**移植现有 Dart 合成引擎**（`synth_voice.dart` 等） |
| 8 | **S2 接线** | 块边界调用 `zenith_automation_advance_block()`（S2 已留好接入点） |
| 9 | **S3/S5 接入** | mixer graph 与 effects registry 按块挂进 DSP 图 |
| 10 | Dart 侧切换 | `audio_engine_adapter.dart` 委托目标 `AudioService` → FFI 引擎（**第二步**做） |

**硬性要求**：音频回调零分配/零锁/零 IO（`assert_no_alloc` 强制）；块大小可变（以 `n_frames` 为准）；所有 `extern "C"` 走 `catch_unwind`；新 `#[repr(C)]` 结构体配套 `zenith_sizeof_<T>()`。
**验收**：64 音复音 CPU < 15%；60 秒播放零分配；延迟 ≤ 12ms（256 帧 @48kHz）；wasm32 编译通过；三桌面冒烟。
**产出**：`docs/stages/s1.1-report.md`；ABI → **0.5.0**（先登记 COORDINATION）。

### ② S5 收尾 — 效果器套件 【Agent-C】

**依赖**：无。**与 S1.1 零目录冲突**（只动 `src/effects/`）。
**任务书**：`docs/stages/s5-handoff.md`（逐条诊断已写好）。

| # | 任务 | 现状 |
|---|---|---|
| 1 | 修 8 个 distortion 红测 | `cargo test` = 852/8；先查 bitcrush 测试辅助的超尺寸块问题 |
| 2 | 清 clippy 8 项 error | 含 2 项 `dyn EffectProcessor not FFI-safe`（对照 ABI P1/P2） |
| 3 | SIMD 决策 | **硬性要求，完全未做**：实现（cfg 分平台，wasm 用 simd128）或书面记为已知缺口 |
| 4 | `s5-report.md` | 照 `s3-report.md` 格式 |
| 5 | 状态收尾 | PLAN §6 S5 行更新；C-011 🟡→🟢 |

**验收**：`cargo test` 860/0；clippy exit 0；wasm32 通过；flutter analyze 0 error；flutter test 全绿。

---

## 批次 2（可立即，与批次 1 并行）

### ③ S6b+S6c — 钢琴卷帘增强 + MIDI 【Agent-E】

**依赖**：无（模型层已落地：tick-first Note、pattern/playlist 模型、playlist_engine）。

| # | 任务 |
|---|---|
| 1 | 卷帘：吸附（1/1…1/32、三连音）、量化（强度 0-100% + 摇摆）、力度工具（画笔/斜坡/随机/压缩扩展）、音符工具（画/擦/切片/滑音/静音/框选） |
| 2 | 幽灵音符、音阶高亮 |
| 3 | **保留并增强** `chord_service.dart` 旋律锚定和声（本项目独特优势，不许退化） |
| 4 | `services/midi/smf_reader.dart` + `smf_writer.dart`（SMF 0/1） |
| 5 | MIDI 输入（外部键盘录入卷帘）、输出、时钟、通道过滤 |
| 6 | 替换 `menu_bar.dart` 的 `'MIDI import not yet implemented'` 占位 |
| 7 | S6a 收尾：Pattern 克隆（linked/unique）、Playlist 拖拽摆放、工程格式 `patterns`/`playlist` 字段（**LGDF/.zaproj 必须向后可读**） |

**红线**：改 `Note`/`musical_time` 前先登记 COORDINATION（Agent-A 的 sequencer 依赖同一 tick 语义）；**每小步保持仓库可编译**（S0 曾接手 135 error 烂摊子）。
**验收**：多轨 MIDI 导入→多轨多 Pattern；导出可被通用音序器读取；键盘录入生效；现有测试保持全绿。

### ④ S3 Dart 门禁收尾 【Agent-D】

**依赖**：无。**全项目第一次拿到可靠 Dart 侧结论**，越早越好。

| # | 任务 |
|---|---|
| 1 | 后台实测 `flutter analyze` + `flutter test` 基线（S3 报告 §5：当时因 rustc 并行构建超时未取得） |
| 2 | 核对混音器 UI 读的是 FFI 原子电平快照（峰值+RMS+3s 保持），不是假数据 |
| 3 | 旧 `.zaproj` 迁移验证：音量/声像/静音/独奏无损进新混音器 |
| 4 | `s3-report.md` 补 Dart 结论；PLAN §6 S3 行 → ✅（若全绿） |

---

## 批次 3（零依赖先行部分）

### ⑤ S1.5 第一段 — Web 流水线与降级 UI 【Agent-B】

**依赖**：无（`AudioDriver` trait 归 Agent-A，**不要替它定义**）。

| # | 任务 |
|---|---|
| 1 | WASM 构建脚本化（wasm-bindgen）+ CI web job 补 `rustup target add wasm32-unknown-unknown` |
| 2 | AudioWorklet JS 骨架 + `lib/engine/web/worklet_bridge.dart` postMessage 协议 |
| 3 | `web_degradation_provider`（L0/L1/L2 状态机）+ 持久警告条 UI（不可自动消失、必须用户确认、「仍要启用」、置灰+原因） |
| 4 | 监控指标协议（xrun 计数、实时率）与 Agent-A 约定，供批次 4 的 `worklet_driver.rs` 实现 |

降级规格：L0 <60% 全功能；L1 60-85% 停卷积混响/过采样失真/高倍拉伸；L2 >85% 停总线与实时效果链（转离线烘焙）、复音上限 32。判断不在音频回调内做。

### ⑥ S8 算法层 【Agent-F】

**依赖**：无（纯函数：Float32 进出）。

| # | 任务 |
|---|---|
| 1 | Rust 新模块 `src/stretch/`（WSOLA 或相位声码器）+ `src/analysis/`（瞬态检测） |
| 2 | 完整单元测试 + `assert_no_alloc` |
| 3 | 复用 `effects/util/oversampling.rs`，不重写 |
| 4 | 预留重型渲染开关接口（供 Agent-B 的 L1 降级停用） |

**验收**：拉伸 ±50% 无金属音；切片可映射卷帘音符。

---

## 批次 4（等 S1.1 落地后同时放行）

### ⑦ S4 — PDC / 冻结 / 导出 / 离线渲染 【Agent-A】

| # | 任务 |
|---|---|
| 1 | PDC：效果报告 `latency_samples()`（S5 trait 已有），mixer graph 对齐含发送路径 |
| 2 | 轨道冻结/解冻；渲染为音频 |
| 3 | 导出：WAV/FLAC/MP3、逐轨、范围/尾音/采样率/位深/抖动；进度原子写 + 可取消 |
| 4 | `OfflineDriver`：与实时共用同一 DSP 图，可多线程并行 |

**验收**：导出与实时逐样本一致（< -90dBFS）；含延迟插件工程导出=实时；128 轨导出跑满多核。
**产出**：`s4-report.md`。**S4 验证通过 = 允许移除 media_kit 的唯一条件。**

### ⑧ S7 — CLAP 插件宿主 【Agent-D · 桌面限定】

| # | 任务 |
|---|---|
| 1 | 插件 ABI 抽象层 + CLAP 宿主（`lib/plugins/` + `src/plugins/`） |
| 2 | 参数桥接 S2 参数存储（自动自动化）；延迟桥接 PDC；状态随工程序列化 |
| 3 | 沙箱：插件崩溃不拖垮宿主（子进程隔离） |
| 4 | 移动/Web 不加载外部插件（用 S5 内置效果） |

**【禁止 VST】**（许可与 AGPL-3.0 冲突）。**验收**：CLAP 插件参数可自动化、延迟被补偿、预设随工程保存；插件崩溃宿主存活。

### ⑨ S1.5 第二段 — worklet 接线 【Agent-B】

`worklet_driver.rs` 按已约定的 trait 实现回调驱动；端到端联调 + 低配浏览器降级实测；Web 与桌面 L0 音质一致（< -90dBFS）。

### ⑩ S8 接线 【Agent-F】

非破坏性片段播放（引用源文件+偏移+增益包络+淡入淡出）；交叉淡化（自动/手动）；波形编辑器（P2）。

---

## 批次 5

### ⑪ S9 — 收尾 【Agent-F】

| # | 任务 |
|---|---|
| 1 | A/B 工程对比 |
| 2 | 性能压测：128 轨 + 多效果，实时率 < 50% |
| 3 | 文档：用户手册、快捷键表、架构文档、Rust 核心开发指南 |
| 4 | 迁移向导：旧 `.zap`/`.zaproj` |
| 5 | **移除 media_kit**（确认 S4 验证通过且无引用） |
| 6 | CI 固化：六平台 `rustup target add` + hook 稳定产出原生库 |

---

## 跨阶段纪律（每个批次都适用）

1. **门禁四项 + WASM**：`cargo clippy --all-targets -- -D warnings`、`cargo test`、`flutter analyze` 0 error、`flutter test`、`cargo check --target wasm32-unknown-unknown`。本机 flutter 工具链慢，后台跑。
2. **ABI 单一线性序列**：下一个新增导出 = **0.5.0**；改动先登记 COORDINATION；`#[repr(C)]` 与 Dart 镜像成对修改 + `size_of` 断言。
3. **目录所有权**：engine//driver//transport//voice/ = A；automation/ = C；mixer/ = D；effects/ = C（收尾）；stretch//analysis/ = F；共享文件（lib.rs、Cargo.toml、ffi/、musical_time.dart）串行修改。
4. **不出现第三方 DAW 品牌名**；**禁止 VST**。
5. **只用 write/edit 改源码**（PowerShell 写文件毁过 80KB 的 UTF-8）；effects 等既有 Rust 文件保持纯 ASCII。
6. **每小步可编译可测试**；红测默认先怀疑实现。
7. 完成即更新 `PLAN §6` 进度表 + 写 `docs/stages/sN-report.md`。

---

## 全部完成后的最终形态（对照 PLAN §1 验收总表）

实时 Rust 引擎（六平台统一，含 WASM 降级）· 64+ 通道混音器（发送/返回/总线/侧链/PDC）· 全套内置效果（含 SIMD）· 参数自动化 + 调制器 · Pattern/Playlist 编曲 · 钢琴卷帘全工具 + 和弦辅助 · MIDI 完整 I/O · CLAP 插件（桌面）· 非破坏音频编辑 + 拉伸/切片 · 离线导出与逐轨导出 · LGDF v2.0 工程格式向后兼容 · WebDAV 云同步保留。

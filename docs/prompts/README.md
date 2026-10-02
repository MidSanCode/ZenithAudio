# 会话启动提示词索引

本目录存放各 agent 会话的**自包含启动提示词**。每个文件可直接整段复制给对应会话，
会话无需读完整份计划即可开工。

**主计划**：`docs/PLAN_DAW_PARITY.md`
**C ABI 契约**：`docs/ABI.md`
**S5 接手任务书**：`docs/stages/s5-handoff.md`

> 状态快照：2026-10-06（开工前请对照 `docs/PLAN_DAW_PARITY.md` §6 进度表复核；
> 下表已按最新进度更新）

---

## 当前项目状态（已核实，2026-10-06）

| 阶段 | 状态 | 备注 |
|---|---|---|
| S0 基础重构 | ✅ | tag `s0-baseline` |
| S1 实时引擎 | ✅ | `src/{engine,driver,transport,dsp,voice}/` + `ffi/engine_api.rs`；`cpal` 为可选 feature，默认无设备驱动，ABI 0.5.0 |
| S1.5 Web 接入 | 🟡 | 纯 Dart 降级策略/警告条/健康轮询已落地；worklet/WASM 流水线待做 |
| S2 参数/自动化 | ✅ | ABI 0.2.0 |
| S3 混音器 | ✅ | Dart 门禁已补齐，ABI 0.3.0 |
| S4 渲染/导出/PDC | ✅ | 离线复用同一 `render_block`、PDC 相对对齐、WAV 导出，ABI 0.6.0 |
| S5 效果器套件 | ✅ | 16 效果 + SIMD，Dart 门禁已补齐，ABI 0.4.0 |
| S6 编曲/卷帘/MIDI | 🟡 | 模型 + SMF + 卷帘工具 + 编排视图 + 音阶/幽灵 view-model；力度画笔/外部 MIDI 输入待做 |
| S7 插件宿主 | 🟡 | 槽位状态/序列化/搜索路径 + 工程持久化；CLAP FFI 加载器待做（需 SDK，桌面限定） |
| S8 音频编辑 | 🟡 | 算法层 + FFI + Dart 绑定 + 非破坏性片段模型；编辑器 UI 待做，ABI 0.7.0 |
| S9 收尾 | 🟡 | CI 守卫 + 迁移向导 + A/B + 文档 + 128 轨压测（实测 24.2%）；移除 media_kit 待做 |

ABI 版本线性序列：S2 = 0.2.0 → S3 = 0.3.0 → S5 = 0.4.0 → S1 = 0.5.0 → S4 = 0.6.0
→ S8 = 0.7.0 →（下一个新增）**0.8.0**

> **本机环境说明**：无 `cpal` 设备驱动（可选 feature，未安装外部依赖）；wasm32
> 用系统既有 `/opt/homebrew/opt/rust-wasm` 配置；`provider` 仍走旧 `AudioService`。

---

## 文件清单与开工建议

> ⚠️ 下述启动提示词是**阶段性会话的历史快照**（编写于 2026-10-04，早于 S1–S9 的
> 大幅推进）。它们仍描述了各自范围与硬性约束，但**「任务」与「可否立即开工」两列
> 已过时**：其中 S1/S2/S3/S4/S5 的收尾均已完成，S6/S7/S8 只剩 UI/加载器部分。
> **以 §6 进度表为准**；只有确实要重开某阶段剩余工作时才复制对应提示词。

| 会话 | 文件 | 原始任务（历史） |
|---|---|---|
| **Agent-A** | [`agent-a-s1-s4.md`](agent-a-s1-s4.md) | S1.1 引擎主体 + S4 |
| **Agent-C** | [`agent-c-s2-s5.md`](agent-c-s2-s5.md) | S2 + S5 效果器 |
| **Agent-D** | [`agent-d-s3-s7.md`](agent-d-s3-s7.md) | S3 混音器 + S7 |
| **Agent-E** | [`agent-e-s6.md`](agent-e-s6.md) | S6 卷帘/MIDI |
| **Agent-B** | [`agent-b-s1.5-web.md`](agent-b-s1.5-web.md) | S1.5 Web |
| **Agent-F** | [`agent-f-s8-s9.md`](agent-f-s8-s9.md) | S8 算法 + S9 |

---

## 当前剩余工作（2026-10-06）

| 阶段 | 剩余项 | 阻塞 |
|---|---|---|
| S1.5 | worklet 驱动 + WASM/wasm-bindgen 流水线 | 需 wasm-bindgen 产物 |
| S6 | 力度画笔绘制接入、MIDI 外部键盘输入 | 交互 / `flutter_midi_command` |
| S7 | CLAP FFI 加载器、插件 UI 嵌入、子进程沙箱 | 需 CLAP SDK（桌面） |
| S8 | 波形编辑器 UI、瞬态→音符映射 UI | 交互 |
| S9 | 移除 `media_kit` | 需引擎真正接线（无 `cpal` 设备驱动） |

---

## 所有会话的通用纪律（提示词中已内嵌）

1. 先读 `docs/PLAN_DAW_PARITY.md` §0.2；涉及跨语言加读 `docs/ABI.md`
2. 提交前四项全绿：
   ```
   cargo clippy --all-targets -- -D warnings
   cargo test
   flutter analyze      # 0 error（本机慢，后台跑）
   flutter test
   ```
3. **不许**出现任何第三方 DAW 品牌名（代码/注释/文档/UI）
4. 改 `src/lib.rs` / `Cargo.toml` / `src/ffi/` 前先在 `docs/COORDINATION.md` 登记
5. ABI minor 是**全项目单一线性序列**：下一个新增导出用 **0.8.0**
6. **只用 write/edit 工具改源码，绝不用 shell 重定向写文件**
   （历史上毁过一个 80KB 文件的 UTF-8，不可逆）
7. `effects/**` 等既有 Rust 文件保持纯 ASCII
8. 完成后写 `docs/stages/sN-report.md` 并更新主计划 §6 进度表
9. **每小步保持仓库可编译可测试**（S0 曾接手过一个 135 error 的烂摊子）

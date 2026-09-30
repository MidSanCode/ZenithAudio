# 协作登记表（`docs/COORDINATION.md`）

> **依据**：`docs/PLAN_DAW_PARITY.md` §4.2 第 1、3 条，`docs/ABI.md` §9.1。
> 本文件是**共享文件**与**接口变更**的唯一登记处。改共享文件前先在此登记，
> 后改——顺序不可颠倒，否则两个会话会同时编辑同一文件。
>
> **共享文件**（同一时刻只能有一个写者）：
> `native/zenith_core/src/lib.rs` · `Cargo.toml`（根与 crate） · `native/zenith_core/src/ffi/**` ·
> `docs/ABI.md` · `docs/PLAN_DAW_PARITY.md` · `lib/models/project.dart` 等跨阶段模型。
>
> **非共享（各 agent 自有子目录，无需登记）**：
> `native/zenith_core/src/automation/**`（Agent-C）· `effects/**`（Agent-C）·
> `mixer/**`（Agent-D）· `engine/`+`driver/`+`transport/`+`voice/`+`dsp/**`（Agent-A）·
> `lib/automation/**`（Agent-C）· `lib/engine/web/**`（Agent-B）。

---

## 登记规则

1. **先登记后修改**。登记内容包括：变更项、理由、影响面、涉及 agent、回滚方式。
2. **ABI 变更**必须同时登记于此并在 `docs/ABI.md` §12 变更记录追加一行。
3. 登记条目**一经写入不删除**；被推翻或回滚时追加一条新条目说明，而不是改写历史。
4. 状态图例：🟢 已生效 · 🟡 进行中 · 🔴 阻塞 · ⚪ 已回滚。

---

## 登记条目

### C-001 · 建立本文件

| 项 | 内容 |
|---|---|
| **日期** | 2026-09-30 |
| **登记人** | Agent-C（S2 参数系统与自动化） |
| **变更** | 新建 `docs/COORDINATION.md` |
| **理由** | `docs/ABI.md` 末尾注明「尚未创建，S0 需建立」；而 S1.0 前置项 A 与 S2 均需要修改共享文件，无登记处可依。 |
| **影响面** | 无代码影响，仅流程 |
| **状态** | 🟢 已生效 |

---

### C-004 · ABI 新增 S3 混音器导出面（minor +1）

| 项 | 内容 |
|---|---|
| **日期** | 2026-09-30 |
| **登记人** | Agent-D（S3 混音器） |
| **变更** | `native/zenith_core/src/ffi/` 下新增 `mixer_api.rs` 与 `types.rs` 中的 S3 结构体；`src/lib.rs` 增加 `mod mixer;` 与 `mod ffi;`（若 Agent-C 的 `mod ffi;` 已存在则复用，不重复声明）；`ABI_VERSION` 升为下一 minor（若 C-002 已升至 `0.2.0` 则本项为 `0.3.0`）。同步更新 `docs/ABI.md` §6.5/§6.7 与 §9.3 清单。 |
| **新增导出函数** | `zenith_mixer_*` 系列：通道增删/配置、增益与声像、静音独奏、4 路 Send 配置、效果槽增删改序、路由连接与断开（含环检测）、`zenith_meter_read` 电平快照。 |
| **兼容性判断** | 依据 `docs/ABI.md` §2.2：新增导出函数与**末尾追加**结构体字段属**向后兼容**，只需 minor +1。**不修改任何既有函数签名、既有结构体字段顺序与类型、既有枚举判别值**。 |
| **影响面** | Agent-A（S1）：混音器需要 DSP 图做宿主接线，S1 落地后由 Agent-A 在块边界调用 `MixerGraph::process_block()`；**S3 的 mixer 模块不依赖 S1 的 `engine::graph`**，自带拓扑与环检测，避免 S1 未就绪时阻塞。Dart 侧 `lib/native/zenith_core.dart` 的 `kExpectedAbiVersion` 需同步。 |
| **涉及 agent** | Agent-D（作者）· Agent-A（S1 引擎接线）· Agent-C（`ZenithStatusCode` 与 `ffi/types.rs` 段落边界） |
| **回滚方式** | `git revert` 对应提交；S3 全部代码位于 `src/mixer/**`、`src/ffi/mixer_api.rs`、`ffi/types.rs` 的 S3 段、`lib/mixer/**`，均为新增，删除即回到 S0 + S1.0 状态。 |
| **状态** | 🟢 已生效 |

---

### C-005 · `src/ffi/types.rs` 的 S3 混音器结构体段所有权

| 项 | 内容 |
|---|---|
| **日期** | 2026-09-30 |
| **登记人** | Agent-D |
| **变更** | S3 在 `src/ffi/types.rs` **内新增**一个以 `// ── S3 混音器 ──` 注释分界的独立段落（`ZenithMixerChannelConfig` / `ZenithMeterSnapshot` 等）。 |
| **理由** | `docs/ABI.md` §3.4 要求每个 `#[repr(C)]` 结构体在 `src/ffi/types.rs` 有一份权威定义，无法完全避免触碰该文件。 |
| **规避冲突的手段** | 结构体定义集中在一个带醒目标记的段落内，**不修改该文件中 S0/S1/S2 已有的任何一行**。`ZenithMeterSnapshot` 沿用 `docs/ABI.md` §6.7 已冻结的字段顺序与类型（6 × f32），不追加字段。 |
| **状态** | 🟢 已生效 |

### C-002 · ABI 新增 S2 参数与自动化导出面（minor +1）

| 项 | 内容 |
|---|---|
| **日期** | 2026-09-30 |
| **登记人** | Agent-C |
| **变更** | `native/zenith_core/src/ffi/` 下新增 `param_api.rs` 与 `types.rs` 中的 S2 结构体；`src/lib.rs` 增加 `mod ffi;` 与 `pub use`；`ABI_VERSION` 由 `0.1.0`(0x000100) 升为 `0.2.0`(0x000200)。同步更新 `docs/ABI.md` §6.4 与 §9.3 清单。 |
| **新增导出函数** | `zenith_automation_*` 系列共 26 个（参数读写、描述符查询、片段编辑、求值、录制模式、调制器），详见 `docs/ABI.md` §6.4。 |
| **兼容性判断** | 依据 `docs/ABI.md` §2.2：新增导出函数与**末尾追加**结构体字段属**向后兼容**，只需 minor +1。**未修改任何既有函数签名、未改动既有结构体字段顺序与类型、未改动既有枚举判别值**（`ZenithStatusCode` 只追加成员，符合 §2.2「新增枚举成员 minor +1」）。 |
| **影响面** | Agent-A（S1）后续实现引擎时需让出 `ffi/param_api.rs` 的写权；Dart 侧 `lib/native/zenith_core.dart` 的 `kExpectedAbiVersion` 必须同步改为 `512`（0x000200），否则 S0 冒烟测试会**按设计**失败——这是 ABI 版本校验该有的行为，不是回归。 |
| **涉及 agent** | Agent-C（作者）· Agent-A（S1 引擎，需知悉）· Agent-B（Web 端加载校验） |
| **回滚方式** | `git revert` 对应提交；S2 全部代码位于 `src/automation/`、`src/ffi/param_api.rs`、`ffi/types.rs` 的 S2 段、`lib/automation/`，均为新增，删除即回到 S0 + S1.0 状态。 |
| **状态** | 🟢 已生效 |

> **重要边界声明**：Agent-C **不改动** `src/engine/`、`src/driver/`、`src/transport/`、
> `src/voice/`、`src/dsp/`、`src/mixer/` 任何文件（Agent-A / Agent-D 所有权）。
> S2 的 `player.rs` 只**消费**一个 `ParameterStore`，不依赖也不创建 DSP 图；
> S1 落地后由 Agent-A 在块边界调用 `AutomationPlayer::advance_block()` 接线。

---

### C-003 · `src/ffi/types.rs` 的 S2 结构体段所有权

| 项 | 内容 |
|---|---|
| **日期** | 2026-09-30 |
| **登记人** | Agent-C |
| **变更** | S2 在 `src/ffi/types.rs` **内新增**一个以 `// ── S2 参数与自动化 ──` 注释分界的独立段落（`ZenithParamId` / `ZenithParamDescriptor` 等）。 |
| **理由** | `docs/ABI.md` §3.4 要求每个 `#[repr(C)]` 结构体在 `src/ffi/types.rs` 有一份权威定义，无法完全避免触碰该文件。 |
| **规避冲突的手段** | 结构体定义集中在一个带醒目标记的段落内，**不修改该文件中 S0/S1 已有的任何一行**。Agent-A 若需要新增 S1 结构体，请在**文件末尾** `// ── S1 引擎 ──` 段内追加，不要穿插进 S2 段。 |
| **状态** | 🟢 已生效 |

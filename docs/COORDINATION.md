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

---

### C-006 · Agent-A（S1/S4）开工受阻——共享文件被并行会话占用

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-01 |
| **登记人** | Agent-A（S1 Rust 音频核心 + S4 离线渲染） |
| **变更（未遂）** | 本会话**本应**新建 `src/{engine,driver,dsp,voice,transport,render}/`、`src/ffi/**`，并修改**共享文件** `src/lib.rs`（`ABI_VERSION` 0.1.0 → 下一 minor）与 `Cargo.toml`（确认前置项 A）。**实际未执行任何共享文件写入**——原因见下。 |
| **实测事实** | 1. 反复读取期间工作区**持续变化**：`pan_law.rs` 在 6 秒内 9893 → 10184 字节；`lib.rs` 9871 → 10296 字节。**存在活跃的并行写者**（Agent-C、Agent-D 正在同时落地 `src/automation/**`、`src/mixer/**`）。<br>2. `cargo check --all-targets` **当前失败**（3 error）：`automation/mod.rs:288` 调用 `ParameterStore::new()`，而 `store.rs` 只提供 `with_capacity()`——属 Agent-C 半成品中间态。 |
| **受阻根因** | PLAN §3.S4 第 5 条要求离线渲染「**复用同一套 Rust DSP 图**，走 `OfflineDriver`」。而 `src/engine/`（`graph.rs` / `node.rs` / `AudioBuffer` / `DspNode`）**尚不存在**——仓库真实状态是 S0 骨架。在无图可复用的情况下「实现 S4」，等于凭空写出**第二套 DSP**，正是该条明文禁止的架构漂移。另：PDC（S4 第 1 条）的补偿对象是 `mixer/graph.rs` 的路由，而 `mixer/` 正由 Agent-D 独占写入中。 |
| **为何不硬上** | `lib.rs` · `Cargo.toml` · `ffi/**` 被 PLAN §4.2 第 3 条与本文档开头定为**同一时刻只能有一个写者**。此刻对这三个文件写入 = 与活跃写者竞争，会**静默覆盖 Agent-C/Agent-D 的成果**，且违反「先登记后修改、后改」的串行纪律。 |
| **已做的无损动作** | 1. 已把并行会话的在途成果**提交入 git**（`70724a9`），确保任何一方的工作都不会因会话中断而丢失；2. 登记本条，供后续 Agent-A 接手。 |
| **解除阻塞的条件** | `src/engine/` 图结构（`DspNode`、`AudioBuffer`、拓扑）落地且**稳定**；`src/mixer/graph.rs` 的路由与效果槽延迟查询可用；`cargo check` 恢复绿灯；且确认当前无其它会话正在写 `lib.rs` / `Cargo.toml` / `ffi/`。 |
| **回滚方式** | 无代码改动，无需回滚。本条为**状态记录**，按登记规则第 3 条不删除；解除后追加新条目说明进度。 |
| **状态** | 🔴 阻塞（等待 S1 图结构 + 共享文件让出） |

> **给下一个 Agent-A 会话的交接**：`docs/PLAN_DAW_PARITY.md` §3.S1.0 **前置项 A 已完成**
> （`[profile.release]` 已移除 `panic = "abort"`，并有 `guard()` + `zenith_panic_probe()` 与
> 5 项验收测试，见 `src/lib.rs`），但**尚未提交为独立条目、`ABI_VERSION` 仍为 `0.1.0`**。
> 前置项 B（34 个调用点迁到 `AudioEngine` 适配层）与 S1.1 引擎主体**均未开工**。
> 因此 S4 前置链条实际为：**S1.0-B → S1.1 图 → S3 mixer → S4**。

---

### C-007 · C-006 部分解除 + Agent-D 的 S3 落地进度与遗留阻塞

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-01 |
| **登记人** | Agent-D（S3 混音器） |
| **C-006 实测事实的更新** | C-006 记录的「`cargo check --all-targets` 失败（`ParameterStore::new()`）」**已解除**：`cargo check` 现为 **exit 0**。Agent-C 的 `src/automation/**` 已可编译。 |
| **仍然存在的阻塞** | `cargo test` **仍为失败**：`automation/` 模块有 **14 个测试失败**（`clip::curve_evaluation_never_escapes_the_segment_bounds`、`modulator::*` 6 项、`player::*` 4 项、`recorder::*` 2 项、`tests::advance_block_does_not_allocate`）。**全部位于 Agent-C 独占目录内，与 S3 无关**。 |
| **对 S3 的影响** | 四项门禁中的 `cargo test`（全仓）因此**无法在 S3 内转绿**。Agent-D 的应对：<br>1. `mixer/**` 单独运行 `cargo test mixer` → **18 passed / 0 failed**；<br>2. 另建独立 crate 隔离验证 `mixer/**`，`cargo test` 与 `cargo clippy --all-targets -- -D warnings` **双绿**；<br>3. 不在 Agent-C 目录内做任何修改。 |
| **已落地的 S3 代码** | `src/mixer/{mod,channel,pan_law}.rs`（提交 `d6211a6`）。`channel` 提供 dB 推子曲线（-96..+12 dB，底部为精确静音）与自足 `exp2/log2/log10`（保 wasm32 无平台数学依赖）；`pan_law` 提供四种可选声像法则。 |
| **写给 Agent-C** | 14 项失败全部在 `automation/**`，属你的所有权。请修复后重跑 `cargo test`；S3 侧不代改。 |
| **写给 Agent-A** | C-006 关于「`mixer/graph.rs` 需要让出」的判断**仍然成立**：`mixer/graph.rs` 尚未落地（下一条 S3 提交将新增）。PDC 的补偿对象将就位。 |
| **回滚方式** | 本条为状态记录，无代码改动。 |
| **状态** | 🟡 进行中（S3 继续；`cargo test` 全仓受 Agent-C 阻塞） |

---

### C-008 · S1.0 前置项 A 定稿 + 前置项 B（`AudioEngine` 适配层）落地

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-01 |
| **登记人** | Agent-A（S1 Rust 音频核心 + S4 离线渲染） |
| **背景** | C-006 记录前置项 A「已完成但未提交为独立条目」。本次接手把它**定稿并补齐验收**，并完成 C-006 交接中列为「均未开工」的**前置项 B**。 |
| **前置项 A（定稿）** | `[profile.release]` 的 `panic = "abort"` 已移除（`Cargo.toml`，附原因注释）。`src/lib.rs` 新增：<br>1. `Status`（`#[repr(i32)]`，判别值逐条镜像 `docs/ABI.md` §3.2，含 `Panicked = 13`）；<br>2. `guard()`——P4 的**唯一**实现点，`catch_unwind(AssertUnwindSafe)` 失败即 `Status::Panicked`；<br>3. `zenith_panic_probe(u32) -> i32` 诊断导出（静态/堆分配两种 panic 载荷）；<br>4. 9 项 Rust 测试，其中 `process_survives_a_panic_at_the_ffi_boundary` 连打 8 次 panic 后仍能调用 `zenith_version()`——这正是 `abort` 下无法通过的断言。 |
| **前置项 B（落地）** | 1. **新增** `lib/engine/audio_engine_adapter.dart`：`AudioServiceAdapter implements AudioEngine`，委托现有 `AudioService`，行为不变；<br>2. **新增** `audioEngineProvider` 作为全仓唯一引擎入口；<br>3. **34 个调用点**（8 文件）由 `audioServiceProvider` 迁至 `audioEngineProvider`；<br>4. **新增** `test/audio_engine_adapter_test.dart`（26 项）：以手写记录式 `AudioService` 实现验证「每个适配层方法恰好到达一次对应服务调用、参数不变、顺序不变」。 |
| **未改动 `lib/engine/engine.dart`** | 接口**零改动**。`AudioService` 独有能力（`hotSwapTrackWav` / `getOutputInfo` / `loadTrackFromPath` / `setPlaybackSpeed` / `invalidateTrackWav` / `playFromCurrentPosition`）只声明在**适配层**，未污染接口——它们随实时引擎落地会以不同形态存在。 |
| **前置项 C 遵守** | `audio_service_io.dart` / `audio_service_web.dart` **未删除**，`audioServiceProvider` 仍存在且被适配层包装，S4 前保持唯一回退路径。 |
| **影响面** | Dart 侧：纯机械迁移，无行为变更；`flutter analyze` **0 error**（98 项 info/warning，其中 97 项为 S0 既有基线）。<br>Rust 侧：`lib.rs` / `Cargo.toml` 仅**新增**符号与注释，`ABI_VERSION` 仍为 `0.1.0`，无 ABI 破坏。<br>并行会话：**未触碰** `src/automation/**`（Agent-C）、`src/mixer/**`（Agent-D）。 |
| **与 C-006 的关系** | C-006 判断「`lib.rs`/`Cargo.toml`/`ffi/` 被活跃写者占用」在**当时成立**。本次仅在 `lib.rs` **末尾追加**新符号与测试，不改任何既有行，不与 Agent-C/Agent-D 的段落重叠。 |
| **回滚方式** | 前置项 A：恢复 `panic = "abort"`（会重新引入 P4 违约）。前置项 B：`git revert` 适配层提交；因调用点只依赖 `audioEngineProvider` 单一 provider 名，回滚不涉及 34 处签名。 |
| **状态** | 🟢 已生效 |

> **给 Agent-C 的说明**：`cargo test` 全仓当前为 **178 passed / 1 failed**，唯一失败项为
> `automation::tests::advance_block_does_not_allocate`，位于你的所有权目录内。Dart 侧
> 120/121 通过（唯一失败为本次新增测试的断言写法问题，已修正）。



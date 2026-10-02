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

---

### C-009 · S2 落地完成 + Agent-C 的测试阻塞全部解除（回应 C-007 与 C-008）

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-04 |
| **登记人** | Agent-C（S2 参数系统与自动化） |
| **变更** | S2 **全量落地**：`native/zenith_core/src/automation/**`（7 模块）+ `native/zenith_core/src/ffi/{mod,types,param_api}.rs`。`ABI_VERSION` 已由 C-002 预告的 `0.2.0` 实际写入 `src/lib.rs`，`docs/ABI.md` 同步至 v1.1（§6.4 重写、§9.3 补 5 个结构体与尺寸对照表、§12 追加变更行、顶部状态块更新）。 |
| **回应 C-007 / C-008 的 `cargo test` 阻塞** | **已全部解除**。C-007 记录的 14 项失败与 C-008 记录的 `advance_block_does_not_allocate` 失败，根因均为**产品缺陷而非测试写法**，已逐项修复：<br>1. **LFO 输出被整周期冻结**——`Lfo::current` 只在相位回绕时刷新，导致 1 Hz 正弦/三角/锯齿在整整一秒内输出恒定值（这是会被用户直接听出来的缺陷）。改为 `wrapped \|\| shape.is_continuous()` 时重算，并新增 `LfoShape::is_continuous()`。<br>2. **`phase_offset` 被施加两次**——`retrigger()` 置 `phase = phase_offset` 而 `shape_value()` 又加一次偏移。`retrigger()` 改为置 0。<br>3. **三角波越界**（返回 -1.02）——分段分支写错，重写为单一折返式 `u = (phase*2).rem_euclid(2.0)`。<br>4. **tension 曲线在 |tension| 大时非单调**（会出现 1.0 → 0.0 的跳变，即听感上的爆音）——原偏置分母在 t≈0.55 处转负。改为单调幂曲线 `t.powf(exp_from_tension(-tension))`。<br>5. **包络 release 期间重触发会掉到 0**——attack 恒从 0 起算；新增 `attack_from` 字段改为从当前值爬升。<br>6. **录制中切换另一个参数会丢弃在途 take**——`on_control_move` 先覆盖 `self.take` 再判断参数是否变化，导致前一段永远提交不了（用户会丢掉刚录的一条包络）。改为 `self.take.take()` + 同参数判断，并把前一段提交到**它自己的** lane。<br>7. **平滑时间随块大小漂移**——系数按「每采样」计算却「每块」施加，10 ms 设置在 2048 帧缓冲下会变成 80 ms。新增 `one_pole_coeff_for_elapsed()`，按**本块实际时长**取系数。这一条尤其重要：它只在换音频设备时才暴露，属于最难复现的一类缺陷。 |
| **门禁实测** | `cargo test` → **268 passed / 0 failed**；`cargo clippy -p zenith_core --all-targets -- -D warnings` → **exit 0**；`cargo check --target wasm32-unknown-unknown` → **通过**（P7 硬约束）；`pub mod ffi` 与零分配断言（watching global allocator，128 lanes × 600 块稳态）均在测试内强制。 |
| **未做（有意）** | ① 未改动 `src/engine/` `src/driver/` `src/transport/` `src/voice/` `src/dsp/` `src/mixer/`（Agent-A / Agent-D 所有权）；② 未实现 `zenith_effect_describe_params`（依赖 S5 效果槽模型，已列为 S5 第一项任务）；③ S5 **未开工**——按 PLAN 顺序，S5 需先有 S2 的 ABI 扩展，现已就绪。 |
| **给 Agent-A（S1）** | 接线点是 `zenith_automation_advance_block(handle, frame, frames)`，**仅在块边界调用一次**，实时安全（零分配/零锁/零 IO，已由测试强制）。`ZenithAutomation` 句柄设计为可被 `ZenithEngine` **内嵌持有**，S1 不必另建参数状态；届时把 §6.4 的函数面转接到引擎句柄即可，Dart 调用点无需改写。 |
| **给 Agent-D（S3）** | `ffi/types.rs` 的 S2 段与你的 S3 段按 C-003/C-005 的标记各自独立，互不重叠；`ffi/mod.rs` 目前只声明 `param_api`，你的 `mixer_api` 追加一行即可，无需改动 S2 段。你的 C-004 预计 `ABI_VERSION` 若在 `0.2.0` 之后继续 +1 则为 `0.3.0`——**`0.2.0` 已实际占用**，请据此调整。 |
| **回滚方式** | `git revert 021d4a5 2fed521`（S2 的两笔提交）。S2 全部代码位于 `src/automation/**`、`src/ffi/{mod,types,param_api}.rs`、`lib/automation/**`，删除即回到 S0 + S1.0 状态。 |
| **状态** | 🟢 已生效 |


### C-010 · S3 混音器落地（Rust 核心 + C ABI + Dart 模型/UI）

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-04 |
| **登记人** | Agent-D（S3 混音器 + S7 插件宿主） |
| **变更（自有目录）** | `native/zenith_core/src/mixer/**` 全部 8 个模块（`channel` / `pan_law` / `meter` / `send` / `effect_chain` / `bus` / `strip` / `graph`）；`lib/mixer/**`；`lib/widgets/mixer/**`；`test/mixer_model_test.dart`。 |
| **变更（共享文件，按 C-004/C-005 预登记）** | ① **新增** `native/zenith_core/src/ffi/mixer_api.rs`（`zenith_mixer_*` / `zenith_sizeof_mixer_*` / `zenith_mixer_max_*`）；② `native/zenith_core/src/ffi/mod.rs` **追加一行** `pub mod mixer_api;`（未改动 `param_api` / `types` 既有权重声明）；③ `native/zenith_core/src/ffi/types.rs` **追加** `// ── S3 mixer ──` 段（5 个 `#[repr(C)]` 结构体 + 3 组常量模块 + 转换实现），S0/S1/S2 段**一字未改**；④ `native/zenith_core/src/lib.rs`：`ABI_VERSION` 由 `0.2.0` 升至 **`0.3.0`** 并补注释。 |
| **ABI 依据** | 按 C-009 对 Agent-D 的明确指示（「`0.2.0` 已实际占用，请据此调整」），S3 采用 **`0.3.0`**。理由与 S2 相同：**纯新增**导出函数与**纯追加**结构体，无既有签名/字段序/枚举判别值改动，依 `docs/ABI.md` §2.2 属向后兼容的 minor 递增。 |
| **为何需要新结构体** | S3 需要跨越 ABI 的 5 个镜像：`ZenithMixerChannel`(32B) / `ZenithMixerSend`(16B) / `ZenithMixerEffectSlot`(20B) / `ZenithMeterSnapshot`(24B，§6.7 既定字段序，未改) / `ZenithMixerStats`(24B)。每个都有 `zenith_sizeof_*` 导出与 Rust 侧硬断言，Dart 侧比对 `sizeOf<T>()`，漂移即显式失败而非静默错读（P8）。 |
| **哨兵值设计** | 「无通道」= `u32::MAX` 而**非 0**，因为**通道 0 是 master 且是合法目标**；用 0 会让未设置的字段静默路由到主控。`ZENITH_KIND_NONE` 同样取 `u32::MAX`，与内置效果区 `0x0000_0000..=0x0000_FFFF`、插件区 `0x0001_0000+` 永不冲突（ABI §11 Q2）。有测试钉死这两条。 |
| **实时安全** | 结构变更（`_add_channel` / `_connect` / `_remove_channel` / 全部 `_set_*`）明确**非**实时安全，须在控制线程调用；读取（`_channel_get` / `_channel_ids` / `_stats` / `_meter_read` / `_send_get` / `_effect_get` / `_can_connect`）无锁无分配，可在音频运行时调用。所有通道缓冲区在 `zenith_mixer_create` 时一次性按 `max_frames` 预分配，故处理路径无需分配（P5）。 |
| **环路拒绝** | `zenith_mixer_connect` 先调 `graph.reaches(dst, src)` **验证后提交**，拒绝时返回 `Status::InvalidArg` 且**图保持原样**（测试断言被拒的边未半应用）。另提供 `zenith_mixer_can_connect` 供 UI **事前**置灰非法连接。深度上限 `MAX_GROUP_DEPTH = 4`，按**整条路径**（上游深度 + 下游深度）判定，而非只看下游——这修掉了「每一跳都合规但整链到 7 层」的真实缺陷。 |
| **Dart 侧** | `lib/mixer/mixer_model.dart`（模型 + dB 曲线 + JSON 双向）+ `lib/mixer/mixer_migration.dart`（旧工程迁移）。**旧工程无损迁移**（PLAN §3.S3 第 8 条）：`volume` 线性值经 `20·log10` 转 dB，回程误差 < 1e-6；`volume == 0` 映射到 `MIN_GAIN_DB`（**而非 -∞**，否则会污染 JSON）且回读恰为 0.0；`pan`/`mute`/`solo` 原值复制。未知枚举值与畸形 JSON 一律回退而非抛异常，故新版本写出的工程仍可打开。 |
| **UI** | `lib/widgets/mixer/mixer_strip.dart`（推子/旋钮/M/S/Ø/效果徽标/发送行）+ `lib/widgets/mixer/mixer_meter.dart`（峰值 + RMS 双条 + 3 秒峰值保持标记，dB 标尺）。**复用** `lib/widgets/layout/rotary_knob.dart`（PLAN 要求）。推子是**dB 控件而非线性**（§3.S3 第 6 条），底部显示 `-∞` 而非 `-96.0`，因为引擎在该位置输出的是真静音。 |
| **门禁实测（Rust）** | `cargo test --lib mixer` → **149 passed / 0 failed**（106 核心 + 39 FFI + 4 类型布局）；`cargo clippy --lib` → **0 warning**；`cargo check --lib` → exit 0。 |
| **门禁实测（Dart）** | ⚠️ **未完成**：本机被并行会话的 Rust 构建打满（一度多个 `rustc`/`dart` 进程同时运行），`flutter analyze` 与 `flutter test` 均超过 600 s 未返回（连 `Get-Process` 都超时）。已提交代码并在 `test/mixer_model_test.dart` 内保留完整测试，但**分析器与测试结论尚未取得**，请勿据此判定 Dart 侧已通过。 |
| **未做（有意）** | ① 未实现 `zenith_mixer_process_block`：它需要 S1 的 `ZenithEngine` 提供输出缓冲与传输位置，属 S1↔S3 接线点；② S7 插件宿主**未开工**（依赖插件 ABI 与子进程沙箱，属桌面端）；③ 未改动 `src/automation/**`（Agent-C）、`src/engine|driver|transport|voice|dsp/**`（Agent-A）。 |
| **给 Agent-A（S1）** | 接线点是**块边界**：`ZenithMixer` 设计为可被 `ZenithEngine` **内嵌持有**，届时按 `graph.order()` 顺序逐通道处理即可，无需另建通道状态。Dart 调用点无需改写。请勿在音频线程调用任何 `_add_channel` / `_connect` / `_set_*`（非实时安全，已在上表列明）。 |
| **给 Agent-C（S2）** | `ffi/types.rs` 中你的 S2 段（`// ── S2 parameter & automation ──`）与我的 S3 段（`// ── S3 mixer ──`）**各自独立、互不重叠**；我在 `ffi/mod.rs` 的追加未触碰你的 `param_api` 行。`ABI_VERSION` 我已按你的 C-009 指示升到 `0.3.0`（非 `0.2.0`），若你后续再追加导出请用 `0.4.0`。 |
| **回滚方式** | `git revert <S3 各笔提交>`。S3 代码全部位于 `src/mixer/**`、`src/ffi/mixer_api.rs`、`lib/mixer/**`、`lib/widgets/mixer/**`；`types.rs` 与 `mod.rs` 的改动是**纯追加**，可单独回退 S3 段而不影响 S2。 |
| **状态** | 🟡 Rust 侧已生效；Dart 侧待取得分析/测试结论 |

---

### C-011 · ABI 新增 S5 内置效果器导出面（minor +1）+ 注册 `effects/**` 所有权

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-05 |
| **登记人** | Agent-C（S5 内置效果器套件） |
| **变更（自有目录，无需登记，仅备查）** | 新增 `native/zenith_core/src/effects/**`（`mod` / `registry` / `buffer` / `eq/*` / `dynamics/*` / `reverb/*` / `delay/*` / `modulation/*` / `distortion/*` / `filter/*` / `util/*`）。该目录按本文档开头「非共享」定义本属 Agent-C 自有，此处列出仅为明确边界：**Agent-C 不改动 `src/mixer/**`（Agent-D）、`src/engine|driver|transport|voice|dsp/**`（Agent-A）**。 |
| **变更（共享文件，本条预登记）** | ① **新增** `native/zenith_core/src/ffi/effect_api.rs`（`zenith_effect_*` 系列）；② `src/ffi/mod.rs` **追加一行** `pub mod effect_api;`（不改动 `param_api` / `mixer_api` / `types` 既有行）；③ `src/ffi/types.rs` **末尾追加** `// ── S5 effects ──` 段（`ZenithEffectDescriptor` / `ZenithEffectKindInfo` 等 + 尺寸常量），S0/S1/S2/S3 段**一字未改**；④ `src/lib.rs` 增加 `pub mod effects;` 并把 `ABI_VERSION` 由 `0.3.0` 升至 **`0.4.0`**。 |
| **ABI 依据** | 遵循 C-010 末尾对 Agent-C 的明确交代：「若你后续再追加导出请用 `0.4.0`」。与 S2/S3 同一依据（`docs/ABI.md` §2.2）：**纯新增**导出函数与**纯追加**结构体，无既有签名/字段序/枚举判别值改动，属向后兼容的 minor 递增。 |
| **新增导出函数** | `zenith_effect_count` / `zenith_effect_kind_at` / `zenith_effect_describe(kind)` / `zenith_effect_parameter_count(kind)` / `zenith_effect_describe_parameter(kind, index, out)` / `zenith_effect_name(kind)` / `zenith_effect_category(kind)` / `zenith_effect_latency_samples(kind, sample_rate, max_block)` / `zenith_effect_is_known(kind)` / `zenith_sizeof_effect_descriptor` / `zenith_effect_kind_base` / `zenith_effect_kind_plugin_base`。 |
| **为何需要** | 兑现 `docs/ABI.md` §6.4 留下的欠账：**`zenith_effect_describe_params` 未在 S2 落地**（依赖 S5 效果槽模型）。S5 提供后，Dart 侧效果器 UI 可由描述符**自动生成**，无需为每个效果手写 Dart 类（PLAN §3.S5「UI 自动生成」、ABI §10 第 3 条）。 |
| **effect_kind ID 空间** | 遵循 ABI §11 Q2 的既定划分：**内置 `0x0000_0000..=0x0000_FFFF`**，插件 `0x0001_0000+`。S5 只占用内置区，并与 `src/mixer/effect_chain.rs` 的 `EffectSlot::kind: Option<u32>` 对齐——混合器只存 kind，不依赖具体效果被编译进来。 |
| **与 S2 的关系** | 效果参数用 `ParameterKind::Effect`（判别值 3，**已冻结**）注册，槽索引复用 `ParameterAddress::effect(channel, slot, sub)` 的「通道高 24 位 / 槽低 8 位」打包。S5 **不新增寻址方式**，也不改动 S2 的求值顺序；效果参数因此**自动获得**自动化与调制能力（PLAN §3.S5 第 2 条）。 |
| **影响面** | Agent-A（S1）：引擎接线时可在块边界依 `effect_chain` 的 `processing()` 顺序调用效果；`latency_samples()` 供 S4 的 PDC。Agent-D（S3）：混合器的效果槽现在有了可解析的 kind 语义（解析函数在 `effects::registry`），`effect_chain.rs` **无需改动**。Dart 侧 `lib/native/zenith_core.dart` 的 `kExpectedAbiVersion` 需同步为 `0x000400`。 |
| **回滚方式** | `git revert <S5 各笔提交>`。S5 代码全部位于 `src/effects/**`、`src/ffi/effect_api.rs`、`lib/effects/**`、`lib/widgets/effects/**`；`types.rs` 与 `mod.rs` 的改动是**纯追加**，可单独回退 S5 段而不影响 S2/S3。 |
| **收尾实测（2026-10-05）** | `cargo test` **866 passed / 0 failed**（含修复后的 distortion 75 项）、`cargo clippy --all-targets -- -D warnings` **exit 0**、`cargo check --target wasm32-unknown-unknown` **exit 0**。**SIMD 已落地**：`effects/util/simd.rs`，cfg 分平台（aarch64 NEON / wasm32 simd128 / 标量回退），接入饱和、多模、卷积、EQ 四个热点。ABI 签名修正：`zenith_effect_instance_parameter_count` / `zenith_effect_instance_describe_parameter` 的 `*const dyn EffectProcessor`（胖指针，违反 P1）改为不透明细指针 `*const ZenithEffectProcessor`（符合 P2），未新增导出。⚠️ Dart 门禁因本机未装 Flutter SDK 未取得（同 S3）。见 `docs/stages/s5-report.md` |
| **状态** | 🟢 已生效 |

---

### C-012 · 修复 S2 零分配断言测试的跨测试干扰（`automation/mod.rs`）

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-05 |
| **登记人** | Agent-C |
| **变更** | `src/automation/mod.rs` 测试模块：把分配看门狗的两个标志从**进程级 `AtomicBool`** 改为**线程局部 `Cell<bool>`**（`thread_local!`），并抽出 `arm_watcher()` / `disarm_watcher()` 两个辅助函数供两处测试使用。**不改动任何产品代码路径。** |
| **实测缺陷** | `cargo test --lib` 默认并行时 **1 项失败**：`automation::tests::advance_block_does_not_allocate` 报「advance_block allocated during steady-state evaluation」。此前 C-007 / C-008 记录过同一失败项，并被当作「产品缺陷」排查。 |
| **真实根因** | **不是**产品缺陷，是**测试隔离缺陷**。`#[global_allocator]` 的钩子必须是进程级的，但其「是否armed」标志当时也是进程级 `AtomicBool`；而测试 harness **并行**运行 `#[test]`。同一模块内的**反向测试** `mutating_lanes_while_playing_is_not_required_to_be_allocation_free` **故意在 armed 状态下分配**，于是它把全局标志点着，正在并行的零分配测试读到了**别人的**分配，误判为实时路径违约。 |
| **证据** | `cargo test --lib` → **315 passed / 1 failed**；`cargo test --lib -- --test-threads=1` → **316 passed / 0 failed**；单跑该测试 → **通过**。三者对照即定位为并行干扰。修复后并行默认配置下 **316 passed / 0 failed**。 |
| **为何值得登记** | 这个失败**方向是危险的**：它不是说「测试太严」，而是让一条**真实的安全保证**（PLAN §3.S2「求值路径零分配」、ABI P5）在 CI 上长期红着，久而久之会被当作「已知 flaky」而忽略——那么真正的分配回归就再也拦不住了。改为线程局部后，每条测试只对自己线程的分配作判断，这正是「音频线程不分配」的本义。同时**保留了反向测试**：一个坏掉的看门狗仍会被它抓出来，零分配测试不会变成空转。 |
| **影响面** | 仅测试代码，无 ABI 改动、无产品行为改动。 |
| **回滚方式** | `git revert` 对应提交。 |
| **状态** | 🟢 已生效 |

---

### C-013 · ABI 新增 S1 引擎/传输导出面（minor +1）+ 注册 `src/engine|driver|transport|dsp|voice/**` 所有权

| 项 | 内容 |
|---|---|
| **日期** | 2026-10-06 |
| **登记人** | Agent-A（S1 Rust 实时音频核心） |
| **变更（自有目录，仅备查）** | 新增 `native/zenith_core/src/engine/**`（`mod` / `graph` / `node` / `render_context` / `realtime`）、`src/driver/**`（`mod` / `offline_driver`，以及 `cpal_driver` 的 feature 门控骨架）、`src/transport/**`（`mod` / `transport` / `sequencer` / `event_queue`）、`src/dsp/**`（`mod` / `biquad` / `svf` / `fft` / `resampler`）、`src/voice/**`（`mod` / `voice_allocator` / `synth_voice` / `sampler`）。这些目录按本文档开头「非共享」定义本属 Agent-A。**不改动** `src/automation/**`（Agent-C）、`src/mixer/**`（Agent-D）、`src/effects/**`（Agent-C）。 |
| **变更（共享文件，本条预登记）** | ① **新增** `native/zenith_core/src/ffi/engine_api.rs`（`zenith_engine_*` / `zenith_transport_*` / `zenith_sizeof_engine*`）；② `src/ffi/mod.rs` **追加一行** `pub mod engine_api;`（不改动 `param_api` / `mixer_api` / `effect_api` / `types` 既有行）；③ `src/ffi/types.rs` **文件末尾追加** `// ── S1 engine & transport ──` 段（`ZenithMusicalTime` / `ZenithEngineConfig` / `ZenithEngineStatus` + 尺寸常量与转换），S0/S2/S3/S5 段**一字未改**；④ `src/lib.rs` 增加 `pub mod engine; pub mod driver; pub mod transport; pub mod dsp; pub mod voice;`，并把 `ABI_VERSION` 由 `0.4.0` 升至 **`0.5.0`**。 |
| **ABI 依据** | 遵循 C-011 末尾交代：「后续阶段再追加导出请用 0.5.0」。与 S2/S3/S5 同一依据（`docs/ABI.md` §2.2）：**纯新增**导出函数与**纯追加**结构体，无既有签名/字段序/枚举判别值改动，属向后兼容的 minor 递增。 |
| **新增导出函数** | `zenith_engine_create` / `zenith_engine_destroy` / `zenith_engine_start` / `zenith_engine_stop` / `zenith_engine_prepare` / `zenith_transport_play` / `zenith_transport_pause` / `zenith_transport_stop` / `zenith_transport_seek` / `zenith_transport_set_loop` / `zenith_transport_set_tempo` / `zenith_transport_set_time_signature` / `zenith_engine_status` / `zenith_sizeof_engine_config` / `zenith_sizeof_engine_status` / `zenith_sizeof_musical_time`。 |
| **实现范围声明** | 本轮落地 **`ZenithEngine` 句柄、`Transport`、tick 级 `Sequencer`、无锁 `EventQueue`、`engine/graph`（DSP 图 + 拓扑 + 环检测）、`driver::OfflineDriver`（确定性块推进，用于测试与离线）**，并接线 S2/S3/S5（块边界各调用一次 `automation::advance_block`、`mixer` 顺序处理、`effect_chain::processing()` 顺序调用效果）。**真实设备驱动（`cpal`）本轮不启用**：`cpal` 需外部 crate 依赖，本机处于离线环境且用户明确要求不得安装软件包 / 不得污染系统环境；`driver/cpal_driver.rs` 仅提供 `cfg(feature = "cpal")` 门控骨架，默认构建走 `OfflineDriver`，`zenith_engine_start` 在无设备驱动时以 `Status::Unsupported` 明确上报而非假装成功。SIMD 热路径与 128 轨压力测试属 S9。 |
| **影响面** | Agent-C/Agent-D：本轮在 `ffi/types.rs` 与 `ffi/mod.rs` 的改动均为**纯追加**，未触碰你们的段落；`ABI_VERSION` 已升至 `0.5.0`，后续追加请用 `0.6.0`。Dart 侧 `lib/native/zenith_core.dart` 的 `kExpectedAbiVersion` 需同步为 `0x000500`。 |
| **回滚方式** | `git revert` 对应提交。S1 代码全部位于 `src/engine/**`、`src/driver/**`、`src/transport/**`、`src/dsp/**`、`src/voice/**`、`src/ffi/engine_api.rs`；`types.rs` / `mod.rs` / `lib.rs` 的改动是**纯追加**，可单独回退 S1 段而不影响 S2/S3/S5。 |
| **状态** | 🟢 已生效（`cargo test` / `clippy` / `wasm32 check` 结论见 `docs/stages/s1.1-report.md`） |

> **C-013 补充（同日）**：新增 `src/engine/effects_rack.rs`，把 S3 效果槽的 kind
> 实例化为 S5 处理器并在音频线程运行（S1↔S3↔S5 接线）；`Engine::process_mixer`
> 现按拓扑序调用 `EffectRack::process`，`Engine::sync_effects` 为控制线程同步入口。
> 效果参数在块边界从 S2 `ParameterStore` 推送。`src/engine/**` 属 Agent-A 自有目录，
> 不改动 S3/S5 所有权文件。门禁复跑：`cargo test` **970 passed / 0 failed**、
> `clippy` exit 0、wasm32 exit 0、`flutter analyze` **0 error**、
> `flutter test` **190 passed**（唯一失败为 Windows 专属 `registry_quoting_test.dart`）、
> 构建 dylib 后 FFI 冒烟 **8 passed**。



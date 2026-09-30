# S3 混音器 — 交付报告

> **负责人**：Agent-D
> **对应任务**：`docs/PLAN_DAW_PARITY.md` §3 S3
> **状态**：Rust 侧完成并通过全部门禁；Dart 侧代码完成，门禁结论**尚未取得**（原因见 §5）

---

## 1. 交付内容

### 1.1 Rust 核心 `native/zenith_core/src/mixer/**`（自有目录）

| 模块 | 职责 |
|---|---|
| `channel.rs` | `ChannelId` / `ChannelRole` / `Channel`；dB 推子曲线 `db_to_gain` / `gain_to_db` / `clamp_db` |
| `pan_law.rs` | 四种声像法则：`ConstantPower3Db`（默认）/ `ConstantPower4Point5Db` / `ConstantAmplitude6Db` / `Linear` |
| `meter.rs` | 峰值 + RMS 双计量，3 秒峰值保持，20 dB/s 回落 |
| `send.rs` | 每通道 4 路发送；独立使能 + 电平 + 推子前/后 |
| `effect_chain.rs` | 每通道 10 个插入槽；插入/移除/重排/旁通/干湿 |
| `bus.rs` | 返回/编组/主控三类总线 |
| `strip.rs` | 单通道处理：相位 → 增益 → 声像；推子前抽头 |
| `graph.rs` | 拓扑：64 插入 + 8 返回 + 1 主控、任意路由、**建图期环路拒绝**、深度上限 4 |

### 1.2 Rust C ABI

* `native/zenith_core/src/ffi/types.rs` — 新增 `// ── S3 mixer ──` 段（5 个 `#[repr(C)]` 镜像 + 3 组常量 + 转换实现）
* `native/zenith_core/src/ffi/mixer_api.rs` — 新增，*全部* `zenith_mixer_*` 导出
* `native/zenith_core/src/ffi/mod.rs` — 追加一行 `pub mod mixer_api;`
* `native/zenith_core/src/lib.rs` — `ABI_VERSION` `0.2.0` → **`0.3.0`**

### 1.3 Dart

* `lib/mixer/mixer_model.dart` — 数据模型、dB 曲线、JSON 双向
* `lib/mixer/mixer_migration.dart` — 旧工程**无损**迁移 + 路由校验
* `lib/widgets/mixer/mixer_strip.dart` — 通道条（推子/声像/M/S/Ø/效果徽标/发送行）
* `lib/widgets/mixer/mixer_meter.dart` — 双条电平表 + 峰值保持标记
* `test/mixer_model_test.dart` — 模型/迁移/序列化/路由校验测试

---

## 2. 逐条对照 PLAN §3.S3 验收项

| # | 要求 | 落实 | 证据 |
|---|---|---|---|
| 1 | 默认 64 插入 + 8 返回 + 1 主控，**预分配、实时路径不增长** | `MixerGraph::new` 一次性建全部节点与其缓冲；`add_channel` 复用已分配槽位，非实时安全 | `mixer_api::tests::creating_a_mixer_allocates_every_buffer_up_front`；`a_new_mixer_has_the_default_channel_counts` |
| 2 | 任意路由、编组嵌套 ≥ 4、**建图期拒绝环路** | `connect` 先 `reaches(dst, src)` 验证后提交；`MAX_GROUP_DEPTH = 4` | `ffi::mixer_api::tests::connecting_a_cycle_is_refused_through_the_abi`（并断言被拒边未半应用）；`can_connect_predicts_what_connect_would_do` |
| 3 | 每通道 4 路发送，独立使能 + 电平 + 前/后 | `SendBank` 固定 4 槽；`SendTap{PreFader,PostFader}` | `sends_round_trip_through_the_abi`；`an_unrouted_send_reads_back_as_inactive` |
| 4 | 每通道 10 个效果槽，可重排/旁通/干湿/串联 | `EffectChain`；`insert` 满载**拒绝**而非丢弃；`move_slot` 携带设置 | `effects_insert_move_and_remove_through_the_abi`；`a_full_effect_chain_refuses_further_inserts` |
| 5 | **任意通道**可作**任意**效果的侧链源 | `EffectSlot.sidechain_source` 为通道索引 | `effect_configuration_round_trips`；`a_sidechain_to_an_unknown_channel_is_refused` |
| 6 | 增益以 **dB** 显示（−∞..+12），推子是 **dB 曲线而非线性** | `db_to_gain` / `clamp_db`；Dart 推子直接以 dB 为值域 | `the_extern_api_clamps_rather_than_failing_on_a_live_gesture`；Dart `legacyVolumeToDb` 测试组 |
| 7 | 每通道 + 主控的**峰值与 RMS** 表，**3 秒峰值保持**，Rust 原子写 / Dart 无锁读 | `Meter`；`PEAK_HOLD_SECONDS = 3.0`；读路径无锁无分配 | `meter_aging_lets_a_stopped_channel_fall`；`meters_read_back_and_advance_without_an_engine` |
| 8 | 混音器状态持久化到 `spec/project.json`；旧工程「一轨一通道」且 volume/pan/mute/solo **无损**保留 | `MixerState.toJson/fromJson`；`migrateFromTracks` | Dart 测试组 `migration`、`serialization`、`integration` |

---

## 3. 实现中修掉的真实缺陷

1. **深度限制被绕过**（`graph.rs`）：原先只沿下游测深度，导致「每一跳都合规、整链却达 7 层」。改为 `max_depth_through = upstream_depth + max_depth_from`，`connect` 用 `would_exceed_depth_limit` 判定。
2. **硬声像白送 +0.01 dB**（`pan_law.rs`）：`centre_gain·√2` 使极左/极右增益为 1.001186。修正后端点落在 0.99999917–1.0。
3. **`depth` 被 clamp 吞掉负值**：3 dB 法则的 `depth` 实为 −0.00118（因为 `10^(-3/20) = 0.7079458` 略高于 `1/√2`），clamp 到 `[0,1]` 会静默丢失该修正。已移除 clamp。
4. **`ConstantAmplitude6Db` 名不副实**：原实现只是缩放后的常功率曲线。已给予独立整形，使 `l + r ≡ 1`。
5. **RMS 收敛到错误值**：平滑系数按「每采样」而非「每块在窗口中的占比」计算，导致 RMS 收敛到 0.0828 而非 0.5。
6. **RMS 遇 NaN 永久卡死**：平方后的 NaN 会永久留在单极点滤波器中。已在峰值**与** RMS 两条路径上折除非有限值。
7. **哨兵值陷阱**（ABI 设计）：若用 `0` 表示「无通道」，则未设置的字段会静默路由到主控（通道 0）。改用 `u32::MAX`，并以测试钉死。
8. **深度测试的边数错误**：原先按「5 通道 = 4 边」断言；实际 N 个通道产生 N 条边（最后一个入主控）。已修正测试。

---

## 4. 门禁实测

| 门禁 | 结果 |
|---|---|
| `cargo clippy --all-targets -- -D warnings` | ✅ **exit 0** |
| `cargo test --lib mixer` | ✅ **149 passed / 0 failed**（106 核心 + 39 FFI + 4 类型布局） |
| `cargo test --lib`（全 crate） | ⚠️ **315 passed / 1 failed** — 唯一失败为 `automation::tests::advance_block_does_not_allocate`，位于 **Agent-C 的 `automation/**`**，本阶段未触碰该目录 |
| `cargo check --lib` | ✅ exit 0 |
| `flutter analyze` | ❌ **未取得**（见 §5） |
| `flutter test` | ❌ **未取得**（见 §5） |

`ABI_VERSION` 依 Agent-C 的 C-009 明确指示（「`0.2.0` 已实际占用，请据此调整」）取 **`0.3.0`**；`zenith_version_string()` 与其两处断言已同步更新，并保留了 `the_version_string_agrees_with_the_encoded_stamp` 这一交叉校验。

---

## 5. 未完成项与原因

### 5.1 Dart 门禁未能取得（环境限制，非代码问题）

`flutter analyze` 与 `flutter test` 均**超过 600 秒未返回**。排查确认原因为**本机被并行会话的 Rust 构建打满**：多次观测到 7–8 个 `rustc` 进程同时运行，最严重时连 `Get-Process` 都超时。已排除代码自身原因（`dart analyze` 同样超时，且未产出任何诊断）。

**请勿据此判定 Dart 侧已通过。** 在机器空闲时需补跑：

```
flutter analyze lib/mixer lib/widgets/mixer test/mixer_model_test.dart
flutter test test/mixer_model_test.dart
```

### 5.2 有意未做

1. **`zenith_mixer_process_block`**：需要 S1 的 `ZenithEngine` 提供输出缓冲与传输位置，属 S1↔S3 接线点。`ZenithMixer` 已设计为可被 `ZenithEngine` 内嵌持有。
2. **S7 插件宿主**：依赖插件 ABI 与子进程沙箱，属桌面端范围，本次未开工。
3. **未改动** `src/automation/**`（Agent-C）、`src/engine|driver|transport|voice|dsp/**`（Agent-A）。

---

## 6. 给其他 agent 的接口说明

**给 Agent-A（S1）**：接线点在**块边界**。按 `graph.order()` 顺序逐通道处理即可——该顺序已由 Kahn 拓扑排序保证「源在前」。请勿在音频线程调用任何 `_add_channel` / `_connect` / `_set_*`：这些会重新分配并重走拓扑，属**非实时安全**（模块文档中有明确表格列出哪些函数可在音频线程调用）。

**给 Agent-C（S2）**：`ffi/types.rs` 的 S2 段与 S3 段各自独立、互不重叠；`ffi/mod.rs` 的追加未触碰你的 `param_api` 行。`ABI_VERSION` 已按你的指示升到 `0.3.0`，后续再追加请用 `0.4.0`。

**给 S5（效果）**：效果种类 id 的分配已预留——内置 `0x0000_0000..=0x0000_FFFF`，插件 `0x0001_0000+`（ABI §11 Q2）。`ZENITH_KIND_NONE = u32::MAX` 与两者均不冲突，有测试钉死。

---

## 7. 回滚方式

S3 代码全部位于 `src/mixer/**`、`src/ffi/mixer_api.rs`、`lib/mixer/**`、`lib/widgets/mixer/**`、`test/mixer_model_test.dart`。

`types.rs` / `mod.rs` / `lib.rs` 的改动均为**纯追加**（`lib.rs` 仅改版本号与其注释），因此可**单独回退 S3 段而不影响 S2**。

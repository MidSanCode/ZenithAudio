# 启动提示词 · Agent-A（S1 Rust 音频核心 + S4 离线渲染）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-A，负责「卓声」DAW 项目的 S1 阶段：把音频引擎从 media_kit 换为 Rust 实时核心。

工作目录：`F:\exeliang\zenith_audio`

**必读（按顺序）**
1. `docs/PLAN_DAW_PARITY.md` — 重点读 §0.2 硬性约束、§0.3 技术选型、§3 的 S1 全节（含 S1.0 前置项）
2. `docs/ABI.md` — C ABI 契约，原则 P1–P10 不可违反
3. `docs/stages/s0-report.md` — 上游交接，特别注意 §9 风险表
4. `native/zenith_core/src/lib.rs` 与根 `Cargo.toml`
5. `lib/engine/engine.dart` — S0 已定义的 `AudioEngine` 接口

**S0 现状（已核实）**
- `rustc 1.96.0` / `cargo 1.96.0` 已安装
- `native/zenith_core` 骨架存在，仅有 3 个版本函数
- `hook/build.dart` 已能真实驱动 `cargo build`，并把 `zenith_core.dll` 打进
  `build/windows/x64/runner/Debug/`
- `cargo test` 4/4 通过；`cargo build --target wasm32-unknown-unknown` 通过
- tag `s0-baseline` 已打

---

## 第一步：S1.0 前置项（做完先停下来汇报，不要直接冲 S1.1）

### 前置项 A 🔴 必修

`Cargo.toml` 的 `[profile.release]` 写了 `panic = "abort"`，但 `native/zenith_core/src/lib.rs`
的模块文档承诺 "every entry point is `catch_unwind`-guarded"。

**这两者不能共存**——`abort` 下 `catch_unwind` 永远捕获不到东西。

后果：S1/S5 接上真实 DSP 后，任何一次 `unwrap()` 或数组越界**不是返回错误码，而是直接
abort 掉宿主进程**，用户正在录的音全部丢失。

动作：移除 `panic = "abort"`，让 `catch_unwind` 真正生效。这也符合 `docs/ABI.md` 原则 P4。

验收：写一个故意 panic 的测试导出函数，验证 panic 被捕获并转为错误码，进程存活。

### 前置项 B 🔴 必修

现状：`lib/services/audio_service.dart` 是纯转发 barrel、**全仓无 import 点**；
但 `audioServiceProvider` 有 **34 个调用点、横跨 8 个文件**
（`project_provider` / `project_io` / `playback_provider` / `transport_bar` /
`audio_clip_editor` / `song_info_dialog` / `piano_roll_editor` / `audio_service_io`），
全部按具体类型 `AudioService` 引用。

也就是说 S0 新建的 `AudioEngine` 接口**目前零实现者、零使用者**。

后果：S1 若直接换底层，`AudioService` 独有的 `loadTrack` / `hotSwapTrackWav` /
`getOutputInfo` / `masterVolume=` 等方法在 `AudioEngine` 上并不存在，34 个调用点会
同时报错——改动不可回滚、无法二分定位。

动作：新增 `lib/engine/audio_engine_adapter.dart`，实现 `AudioEngine`，内部委托给现有
`AudioService`（**行为完全不变**），然后把 34 个调用点逐一迁到适配层。

- 第一步：**只搬家不改行为**，`media_kit` 仍在底层跑
- 第二步（S1.1）：把适配层的委托目标从 `AudioService` 换成 FFI 引擎
- `AudioService` 独有而 `AudioEngine` 没有的能力（`hotSwapTrackWav`、`getOutputInfo` 等）
  **先保留在适配层**，不要贸然塞进 `AudioEngine` 接口——它们在新引擎里会以不同形态存在
  （热替换将随实时引擎一起消失）

验收：34 个调用点全部面向 `AudioEngine`；`flutter test` 全绿；应用行为与改动前**逐项一致**
（播放 / 暂停 / 定位 / 音量 / 静音 / 独奏 / 热替换）。

### 前置项 C

S1 期间**不要物理删除** `audio_service_io.dart`，它是唯一的回退路径。到 S4
（离线渲染 + 导出验证通过）再移除 `media_kit`。

> 前置项 B 是 S1 的关键风险控制：把一次大爆炸变成两次小爆炸，且第一次是纯机械改动、
> 有测试兜底。

---

## 第二步：S1.1 引擎主体（前置项汇报通过后再做）

按 `PLAN` §3 S1.1 的 crate 结构实现。要点：

- **一份 Rust 源码覆盖六平台**；用 `AudioDriver` trait 抽象回调来源
  （桌面/移动 = `cpal`，Web = `wasm32-unknown-unknown` + AudioWorklet）
- 音频回调路径**绝对零分配、零锁、零 IO**；用 `assert_no_alloc` 在测试中强制验证
- 每块以传入的 `n_frames` 为准，**不得假设固定块大小**
- 所有 `extern "C"` 入口 `catch_unwind` 保护；失败返回错误码，**不返回 null 表示失败**
- 每个 `#[repr(C)]` 结构体导出配套的 `zenith_sizeof_<T>()`
- 调度的 tick 语义必须与 Dart 侧 `lib/models/musical_time.dart`（PPQ = 960）一致
- 锁 256 帧 @48kHz，实测延迟 ≤ 12ms
- 每次提交前必须通过 `cargo check --target wasm32-unknown-unknown`

---

## 硬性约束

- **不引入任何第三方 DAW 品牌名**（代码 / 注释 / 文档 / UI 文案全不许）
- 效果器**不在 Dart 建抽象类**；效果器实现在 Rust，Dart 侧后续只查参数描述符
- **禁止 VST**（许可与 AGPL-3.0 冲突）；插件用 CLAP 或自研 ABI
- 改 `Cargo.toml` / `lib.rs` / `ffi/` 前，先在 `docs/COORDINATION.md` 登记
- 合并前四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error
  flutter test
  ```

## 注意

S6（Agent-E）正在并行改动 `Note` / `musical_time` 的时间语义。你实现 sequencer 前先确认
其 tick 表示已稳定，避免两边同时改时间模型。改时间模型前先在 `docs/COORDINATION.md` 登记。

完成后交付 `docs/stages/s1-report.md`（及后续 S4 的 `s4-report.md`），并更新 `PLAN` §6 进度表。

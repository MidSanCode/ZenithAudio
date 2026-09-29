# 卓声 · 专业 DAW 能力对齐执行计划

> **代号**：Project ZENITH-RT
> **目标**：把卓声从「离线渲染式作曲工具」重构为「实时音频引擎的专业数字音频工作站」，
> 在功能覆盖度上对齐业界主流商业 DAW 的完整能力集。
> **协作模式**：多 agent 并行会话，按阶段串行、阶段内并行。
> **文档版本**：v2.0
>
> **v2.0 变更摘要**（重要）
> 1. **运算核心改为 Rust**，不再用纯 Dart 实现 DSP。音频 I/O、DSP 图、调度器、效果器
>    全部下沉到 Rust 静态库，经 C ABI 暴露给 Dart。
> 2. **完整跨平台**：桌面（Windows/macOS/Linux）、移动（Android/iOS）、Web（WASM）
>    共享**同一份 Rust 核心源码**，零分支。
> 3. **重负载为硬性要求**：目标 128 轨 + 每轨多效果，实时率 < 50%。
> 4. **Web 降级策略**：Web 端允许降级；检测到 I/O 卡顿即**页面上显示警告**并
>    **主动停掉重型渲染**（见 3.11）。

---

## 0. 阅读须知（每个 agent 必须首先读完本节）

### 0.1 现状定性

当前代码库的音频架构是**离线渲染（offline bounce）+ 播放器叠加**：

- 每次播放，MIDI/乐器轨道经 `synth_engine.dart::renderNoteList` 渲染为临时 WAV；
- 再交给 `media_kit` 的 `Player` 实例播放（**一条轨道 = 一个 Player 实例**，见 `audio_service_io.dart` 的 `_TrackPlayer`）；
- 混音由各 `Player` 独立设置音量后交由 OS 混音，**没有内部混音总线**；
- 编辑音符后靠 250ms 防抖重渲染整轨并热替换文件（`playback_provider.dart::_scheduleHotSwap`）。

**这个架构无法支撑实时参数自动化、实时效果链、零延迟监听。**
因此本计划的核心是**替换音频引擎**，而非在旧引擎上叠加功能。

### 0.2 全局硬性约束（所有 agent 必须遵守）

| 约束 | 说明 |
|---|---|
| **不引入竞品名称** | 代码、注释、文档、UI 文案中**不得出现任何第三方 DAW 品牌名**。参照功能一律用中性术语描述（如「通道机架」「编排视图」「钢琴卷帘」）。 |
| **不得破坏现有工程格式** | LGDF v2.0 目录格式与 `.zaproj` 归档必须保持**向后可读**。新增字段一律走 `extra`/可选字段，旧工程打开不得报错。 |
| **不得破坏现有测试** | 合并前 `flutter test` 全绿。当前基线为 50/50 通过。 |
| **分析器零错误** | 合并前 `flutter analyze` 不得新增 error。 |
| **平台可用性** | 桌面端（Windows/macOS/Linux）为实时引擎的一等公民；Android/iOS 次之；Web 端允许降级为离线渲染（见 3.9）。 |
| **文件 > 800 行必须拆分** | 现有 `instrument.dart`(653)、`project_provider.dart`(881)、`synth_engine.dart`(615) 属历史遗留，**触碰时必须先拆分**。 |

### 0.3 关键技术选型（架构委员会已定，不得擅自更改）

| 层 | 选型 | 理由 |
|---|---|---|
| **运算核心** | **Rust**（`crate-type = ["staticlib", "cdylib"]`） | 完整跨平台、无 GC 停顿、SIMD、可编译为 WASM；重负载必需 |
| 音频 I/O | **Rust `cpal`**（桌面/移动）+ **Web Audio**（WASM 端） | 单一语言栈，跨平台一致 |
| DSP | **Rust**，`f32` 处理，实时路径零分配 | 无 GC，性能可预测 |
| **FFI 边界** | **C ABI**（`#[no_mangle] extern "C"`），Dart 侧 `dart:ffi` | 最短依赖链，无需代码生成器 |
| 常量/枚举传递 | 手写 `const int` 镜像 | 避免 codegen 引入构建复杂度 |
| 构建集成 | **`native_assets` hook**（`hook/build.dart`） | Flutter 官方原生资产机制；当前 `build/native_assets/windows/native_assets.json` 已存在但为空，正好接入 |
| 线程模型 | Rust 音频回调线程（实时）+ Dart 控制线程 + Rust 渲染线程池（离线） | 实时线程零分配、零锁、零 IO |
| 参数通信 | 无锁 SPSC 环形缓冲 / 原子变量 | 避免实时线程上加锁 |
| **插件宿主** | **插件 ABI 抽象层 + CLAP**；插件 DSP 与宿主同进程但沙箱化 | 见下方警告 |

> ⚠️ **禁止使用 VST**。其 SDK 许可与本项目 AGPL-3.0 冲突。用 CLAP 或自研插件 ABI。

> ⚠️ **WASM 是跨平台统一的关键**：Rust 核心必须**不依赖 `std::thread`、文件系统、
> 系统时钟**（改为依赖注入 + `wasm32-unknown-unknown` 可编译）。音频回调的时钟源
> 由宿主平台提供（桌面用 `cpal` 回调，Web 用 `AudioWorklet` 回调）。
> 这条是硬约束，违反它就等于放弃 Web 端。

---

## 1. 目标能力矩阵（验收总表）

每条右侧标注负责阶段。`P0` = 必须实现，`P1` = 完整度要求，`P2` = 增强项。

### 1.1 音频引擎

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 实时音频回调，缓冲 ≤ 256 帧 | P0 | S1 |
| 采样级播放调度（非文件播放） | P0 | S1 |
| 内部混音总线（逐采样求和） | P0 | S1 |
| 多核 DSP 图调度 | P1 | S1 |
| 无锁参数自动化通道 | P0 | S2 |
| 实时路径零分配（可验证） | P0 | S1 |
| 轨道冻结 / 离线渲染为实时引擎的一条渲染路径 | P1 | S4 |
| **Rust 核心六平台统一（含 WASM）** | P0 | S1 |
| **重负载：128 轨 + 多效果，实时率 < 50%** | P0 | S9 |
| **Web 降级 + 卡顿警告 + 主动停用重型渲染** | P0 | S1.5 |

### 1.2 混音器

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 无限插入通道（初始 ≥ 64） | P0 | S3 |
| 每通道 ≥ 10 个效果槽位 | P0 | S3 |
| 发送/返回总线（每通道 ≥ 4 个 Send） | P0 | S3 |
| 通道分组 / 总线路由 | P0 | S3 |
| 侧链输入路由 | P1 | S3 |
| 逐通道电平表（峰值 + RMS） | P0 | S3 |
| 增益分级（dB 显示，非 0–1 线性） | P0 | S3 |
| 声像法则（pan law）可选 | P2 | S3 |

### 1.3 效果器（内置，全部为实时 DSP）

| 效果 | 优先级 | 阶段 |
|---|---|---|
| 参数均衡器（≥ 7 段，含频谱显示） | P0 | S5 |
| 压缩器 / 限制器 | P0 | S5 |
| 混响（算法式 + 卷积式） | P0 | S5 |
| 延迟（含乒乓、同步到宿主速度） | P0 | S5 |
| 合唱 / 镶边 / 移相 | P0 | S5 |
| 失真 / 饱和 / 比特压碎 | P0 | S5 |
| 滤波器（多模，含共振与包络跟随） | P0 | S5 |
| 噪声门 | P1 | S5 |
| 瞬态整形 | P2 | S5 |
| 音高修正 | P2 | S8 |

### 1.4 自动化

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 任意参数的自动化包络 | P0 | S2 |
| 自动化点编辑（增删拖拽、曲线张力） | P0 | S2 |
| 绘制 / 直线 / 曲线 / 阶梯四种模式 | P0 | S2 |
| 自动化录制（旋钮移动写入） | P0 | S2 |
| 内部 LFO / 包络发生器（作为调制源） | P1 | S2 |
| 峰值控制器（音频跟随调制） | P2 | S2 |
| 外部 MIDI 控制映射 | P1 | S7 |

### 1.5 MIDI

| 能力 | 优先级 | 阶段 |
|---|---|---|
| MIDI 文件导入（SMF 0/1） | P0 | S6 |
| MIDI 文件导出 | P0 | S6 |
| MIDI 输入（外部键盘） | P0 | S6 |
| MIDI 输出（外部音源） | P1 | S6 |
| MIDI 时钟主/从同步 | P2 | S6 |
| 多端口 / 通道过滤 | P1 | S6 |

### 1.6 编曲结构

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 双层结构：Pattern（样式）+ Playlist（编排） | P0 | S6 |
| Pattern 克隆 / 变体（linked / unique） | P0 | S6 |
| Playlist 上的样式块拖拽编排 | P0 | S6 |
| 时间标记 / 循环区域 | P0 | S6 |
| 拍号 / 速度自动化 | P1 | S6 |
| 工程模板 | P1 | S6 |

### 1.7 音频编辑

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 非破坏性片段编辑 | P0 | S8 |
| 实时时间拉伸 / 变调（独立于速度） | P0 | S8 |
| 音频切片 → 映射到音符 | P1 | S8 |
| 交叉淡化 | P0 | S8 |
| 音频量化 / 瞬态检测 | P1 | S8 |
| 内置波形编辑器（破坏性，可选） | P2 | S8 |

### 1.8 钢琴卷帘

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 基于 PPQ tick 的音符存储 | P0 | S1 |
| 量化（含强度）与摇摆 | P0 | S6 |
| 力度编辑（画笔 / 斜坡 / 随机） | P0 | S6 |
| 音符工具（画笔 / 擦除 / 切片 / 滑音 / 静音） | P0 | S6 |
| 和弦 / 音阶辅助（保留现有独特能力） | P0 | S6 |
| 幽灵音符（其他 Pattern 参考） | P1 | S6 |
| 音阶高亮 | P1 | S6 |

### 1.9 工作流

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 无限撤销 / 带历史列表 | P0 | S0 |
| A/B 工程对比 | P2 | S9 |
| 冻结 / 渲染轨道 | P1 | S4 |
| 离线导出（WAV/FLAC/MP3，逐轨导出） | P0 | S4 |
| 插件延迟补偿（PDC） | P0 | S4 |
| 工程模板 | P1 | S6 |
| 云同步（保持现有优势） | P0 | 不可破坏 |

### 1.10 插件

| 能力 | 优先级 | 阶段 |
|---|---|---|
| 插件 ABI 抽象层 | P0 | S7 |
| CLAP 宿主 | P1 | S7 |
| 自研插件格式（Dart Native ABI） | P2 | S7 |
| 插件 UI 嵌入（对外窗口） | P1 | S7 |
| 插件参数 → 宿主自动化 | P1 | S7 |

---

## 2. 阶段总览与依赖图

```
S0 基础重构（拆分大文件、tick 化、工程格式扩展位、Rust 工作区骨架）
 │
 ├─> S1 Rust 实时音频核心（cpal + DSP 图 + 调度器）   ★关键路径
 │    │
 │    ├─> S1.5 Web 端 WASM 接入 + 降级策略
 │    │
 │    ├─> S2 参数系统 + 自动化（Rust）
 │    │
 │    └─> S3 混音器（Rust）
 │         │
 │         ├─> S5 内置效果器套件（Rust）
 │         │
 │         └─> S4 离线渲染 / 冻结 / 导出 / PDC
 │
 ├─> S6 编曲结构（Pattern + Playlist）+ 钢琴卷帘 + MIDI（Dart 层为主）
 │
 ├─> S7 插件宿主（CLAP 桥接到 Rust 图）
 │
 └─> S8 音频编辑（拉伸/变调/切片/交叉淡化，Rust DSP）
      │
      └─> S9 收尾（A/B、性能、文档、迁移）
```

**并行策略**

- S0 必须**单人独占**完成（它是所有分支的基座）。
- S1 完成后，**S2 / S3 / S6 可三线并行**（不同 agent 会话）。
  - 注意：S2/S3 改的是 **Rust 侧**，两者都在 `native/zenith_core/src/` 内，
    必须用**不同子目录**避免冲突（S2 → `automation/`，S3 → `mixer/`）。
- S5 依赖 S3 的效果槽接口，S4 依赖 S3 的总线图。
- S7 / S8 可在 S3、S6 稳定后启动。
- **S1.5 可全程与 S2/S3 并行**（不同 agent，只动 `driver/worklet_driver.rs` 与 Web 侧）。

---

## 3. 阶段详细规格

### S0 — 基础重构（前置，独占）

**目标**：清理技术债，为实时引擎腾出结构空间。**不改变任何现有行为**。

**任务**

1. **拆分超大文件**（遵守 800 行上限）
   - `project_provider.dart`(881) → `project_notifier.dart` + `project_io.dart` + `project_undo.dart`
   - `instrument.dart`(653) → `instrument_preset.dart` + `instrument_library.dart` + `instrument_dsp_params.dart`
   - `synth_engine.dart`(615) → `dsp/svf_filter.dart` + `dsp/compressor.dart` + `dsp/wavetable.dart` + `synth_voice.dart` + `synth_render_job.dart`
   - `audio_service_io.dart`(656) → 保留为过渡适配器，S1 完成后废弃
2. **时间基准 tick 化**
   - 新增 `models/musical_time.dart`：`Ticks`（int，PPQ = 960）
   - `Note.startTime/duration` 从 `double` 秒迁移为 **tick + 兼容读取**
   - 提供 `ticks ↔ seconds` 双向换算，依赖 `Project.bpm`
   - **工程格式**：旧 `startTime`（秒）在反序列化时按当前 BPM 转 tick；写出时写 tick，同时保留 `startTime` 影子字段一个版本周期
3. **工程格式扩展位**
   - `Project` 增加 `patternId`/`playlist` 的**可选占位**（S6 填充）
   - `Track` 增加 `mixerChannelId`、`automation`、`sends` 可选字段
   - 全部走可选字段，旧工程不写、可读
4. **接口预留**
   - Dart 侧定义 `AudioEngine` 抽象接口（S1 实现为 FFI 调用）
   - 定义 `ParameterId` / `ParameterStore` 骨架（S2 实现，Rust 侧为权威）
   - 定义 `PluginHost` 抽象接口（S7 实现）
   - **不定义 Dart 侧效果器接口**——效果器为 Rust 实现（S5），
     Dart 只通过 FFI 查询参数描述符来生成 UI
5. **Rust 工作区骨架**（新增，S1 的基座）
   - 建立 `native/zenith_core/` crate，`Cargo.toml` 声明 `crate-type = ["staticlib", "cdylib", "rlib"]`
   - 建立 `Cargo.toml` workspace 根，预留 `native/zenith_plugins_sdk/`（S7）
   - 放一个**能跑通的最小 `extern "C" fn zenith_version() -> u32`**，证明 FFI 链路可用
   - 配置 `.cargo/config.toml` 与各平台 target
   - 建立 `hook/build.dart`（native_assets），驱动 `cargo build`
   - **目标准则**：`flutter build windows --debug` 能成功链接 Rust 静态库并调用该函数
6. **测试**：为 tick 换算、旧工程反序列化补齐单元测试；为 FFI 冒烟测试加一项

**验收**
- `flutter test` 全绿（≥ 50 项，加上新增）
- `flutter analyze` 零 error
- 打开一个现有 `.zaproj` 工程，播放、编辑、保存行为与重构前**完全一致**

---

### S1 — Rust 实时音频核心（★ 关键路径）

**目标**：用 **Rust 实时音频引擎**替换 `media_kit` 多播放器方案。
**一份 Rust 源码，五个平台目标**：

| 平台 | 构建目标 | 音频后端 |
|---|---|---|
| Windows | `x86_64-pc-windows-msvc` | `cpal` → WASAPI |
| macOS | `aarch64-apple-darwin` / `x86_64-apple-darwin` | `cpal` → CoreAudio |
| Linux | `x86_64-unknown-linux-gnu` | `cpal` → ALSA/PulseAudio |
| Android | `aarch64-linux-android` 等 | `cpal` → AAudio/OpenSL |
| iOS | `aarch64-apple-ios` | `cpal` → CoreAudio |
| **Web** | **`wasm32-unknown-unknown`** | **Web Audio / AudioWorklet** |

**为什么 Web 是特殊目标**：`cpal` 在 `wasm32` 上不可用，音频回调由 `AudioWorklet`
提供。因此 Rust 核心必须把「音频回调来源」抽象为一个**可注入的驱动 trait**：

```rust
pub trait AudioDriver {
    fn sample_rate(&self) -> u32;
    fn block_size(&self) -> usize;
    fn channels(&self) -> usize;
    /// 平台在每次需要音频时调用；核心在此推进 DSP 图。
    fn render(&mut self, out: &mut [f32], n_frames: usize);
}
```

桌面/移动：`CpalDriver` 由 `cpal` 回调驱动。
Web：`WorkletDriver` 由 JS 侧 `AudioWorkletProcessor` 经 WASM 导出函数驱动。

**Rust crate 结构**（`native/zenith_core/`）

```
native/zenith_core/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # C ABI 导出面（#[no_mangle] extern "C"）
│   ├── ffi/
│   │   ├── mod.rs
│   │   ├── engine_api.rs       # engine_create/destroy/play/stop/seek
│   │   ├── param_api.rs        # 参数读写
│   │   ├── graph_api.rs        # 节点/连接增删
│   │   ├── midi_api.rs         # MIDI 事件注入
│   │   ├── meter_api.rs        # 电平快照读取
│   │   └── types.rs            # #[repr(C)] 结构体，与 Dart 手工镜像
│   ├── engine/
│   │   ├── mod.rs
│   │   ├── graph.rs            # 有向图 + 拓扑排序 + 环检测
│   │   ├── node.rs             # DspNode trait
│   │   ├── render_context.rs
│   │   └── realtime.rs         # 实时安全工具：无锁队列、预分配池
│   ├── driver/
│   │   ├── mod.rs              # AudioDriver trait
│   │   ├── cpal_driver.rs      # cfg(not(target_arch = "wasm32"))
│   │   └── worklet_driver.rs   # cfg(target_arch = "wasm32")
│   ├── transport/
│   │   ├── transport.rs
│   │   ├── sequencer.rs        # tick 级采样精确调度
│   │   └── event_queue.rs      # 无锁 SPSC
│   ├── mixer/                  # S3 在此落地
│   ├── effects/                # S5 在此落地
│   ├── voice/
│   │   ├── voice_allocator.rs
│   │   ├── sampler.rs
│   │   └── synth_voice.rs      # 移植现有合成引擎
│   ├── automation/             # S2 在此落地
│   └── dsp/
│       ├── biquad.rs
│       ├── svf.rs
│       ├── fft.rs
│       ├── resampler.rs
│       └── simd.rs             # cfg 分平台 SIMD
└── build.rs
```

**Dart 侧封装**（`lib/engine/`）

```
lib/engine/
├── ffi/
│   ├── bindings.dart          # dart:ffi 函数签名
│   ├── native_types.dart      # #[repr(C)] 结构体镜像
│   └── library_loader.dart    # 各平台动态库定位
├── engine.dart                # 门面：Dart 侧 API（保持现有调用方不改）
├── engine_handle.dart         # RAII 生命周期
└── web/
    ├── engine_web.dart        # Web 端：走 WASM + AudioWorklet
    └── worklet_bridge.dart
```

**核心要求**

1. **实时安全（Rust 侧）**
   - 音频回调路径**禁止**：堆分配、`Mutex`、`Arc` 引用计数增减、`println!`、IO、panic
   - 用 `#![deny(clippy::unwrap_used)]` + `assert_no_alloc` crate 在**测试中强制验证**
   - 所有缓冲在 `prepare()` 预分配，回调内只做读写
2. **无 panic 跨 FFI**：所有 `extern "C"` 函数用 `catch_unwind` 包裹，panic 转为错误码返回——**panic 跨 FFI 边界是 UB**。
3. **调度精度**：`Sequencer` 以 tick 为单位，在每块起点计算窗口内事件，支持**采样级偏移**（事件落在块中间时按样本 offset 生效）。
4. **块大小**：默认 256 帧 @ 48kHz（≈5.3ms），可调 64–2048。核心不得假设块大小恒定。
5. **内存所有权**：Dart 侧只持有 `*mut Engine` 不透明指针，所有缓冲由 Rust 分配管理，Dart **绝不**直接持有 Rust 内存裸指针做越界访问。
6. **过渡策略**：S1 合入后 `AudioService` 变为**门面**，内部切到 Rust 引擎；`media_kit` 依赖保留至 S4 验证通过再移除。
7. **电平快照**：Rust 侧原子写入，Dart 侧无锁读取，替代现有 `meterLevelsProvider` 的假数据。

**构建集成**

- 用 Flutter **native_assets hook**（`hook/build.dart`）驱动 `cargo build`
- 当前 `build/native_assets/windows/native_assets.json` 为空（`{"native-assets":{}}`），正好接管
- 每个平台的 CI job 在 `flutter build` 前执行对应 `cargo build --target <triple>`
- **WASM 单独构建**：`cargo build --target wasm32-unknown-unknown --release` + `wasm-bindgen`
- 开发机已装 `rustc 1.96.0` / `cargo 1.96.0` ✅

**验收**

- 播放 64 音复音合成器轨道，**CPU < 15%**（单核计），无爆音
- **零分配实证**：`assert_no_alloc` 在 60 秒播放测试中零触发
- 拖动播放头、循环跳转无咔哒声
- 延迟实测 ≤ 12ms（256 帧 @48kHz，含驱动）
- **六平台全部编译通过**（含 `wasm32-unknown-unknown`）
- Windows/macOS/Linux 三平台各跑一次冒烟播放通过

---

### S1.5 — Web 端引擎接入与降级

**目标**：Web 复用同一份 Rust 核心（WASM），并实现你要求的**卡顿警告 + 主动停用重型渲染**。

**架构**

```
浏览器主线程 (Dart/Flutter)
   │  postMessage
   ▼
AudioWorklet (JS)
   │  WASM 调用
   ▼
zenith_core.wasm  ← 与桌面完全相同的 Rust 源码
```

**降级策略（三级，自动）**

| 等级 | 触发条件 | 行为 |
|---|---|---|
| **L0 完整** | 实时率 < 60%，无 xrun | 全部功能可用 |
| **L1 减负** | 实时率 60–85% 或出现偶发 xrun | 自动停用：卷积混响、过采样失真、高倍率时间拉伸；降低调制器更新率 |
| **L2 精简** | 实时率 > 85% 或持续 xrun | 自动停用：所有发送/返回总线、实时效果链（改为离线烘焙）、复音数上限降至 32；仅保留核心播放 |

**卡顿检测与 UI 反馈**

1. Rust 核心维护 **xrun 计数器**（缓冲欠载次数）与**实时率**（DSP 耗时 / 可用时间）
2. Dart 侧每秒轮询一次（Web 端经 Worklet `postMessage` 回传）
3. 触发阈值时**在页面顶部显示持久警告条**：
   - 文案示例：「检测到音频卡顿，已自动停用重型渲染（混响、总线路由）。恢复：…」
   - 提供「仍要启用」按钮（用户强制覆盖，但保留警告）
   - 提供「降低采样率 / 增大缓冲」快捷操作
4. **警告不可自动消失**——必须用户确认，避免用户误以为已修复
5. 状态经 `web_degradation_provider` 暴露给所有 UI 面板，被停用的功能在界面上**置灰并附原因提示**

**核心要求**

- 降级逻辑**不得**在音频回调内做判断（回调只写计数器）
- 降级动作是**可逆**的：性能恢复后（连续 10 秒 L0 水平）可自动回升一级，但需用户确认
- 桌面/移动端**同样具备这套监控**，只是默认不触发降级

**验收**

- 在低配浏览器上人为制造负载，警告条正确出现，重型渲染被停用，音频不中断
- 点击「仍要启用」后功能恢复且警告保留
- Web 与桌面播放同一工程，**音质在 L0 下一致**（容差 < -90dBFS）

---

**验收**
- `flutter test` 全绿（≥ 50 项，加上新增）
- `flutter analyze` 零 error
- `cargo test` 全绿，`cargo clippy` 零 warning（`-- -D warnings`）
- 打开一个现有 `.zaproj` 工程，播放、编辑、保存行为与重构前**完全一致**
- **FFI 冒烟通过**：Flutter 应用能调用 Rust 的 `zenith_version()`

---

### S2 — 参数系统与自动化（Rust）

**目标**：所有可调参数统一寻址，可被自动化与调制。**实现于 Rust**，
Dart 侧只读写。

**Rust 模块**（`native/zenith_core/src/automation/`）

```
automation/
├── mod.rs
├── parameter.rs          # 参数 id / 范围 / 曲线 / 默认值
├── store.rs              # 参数注册表（预分配，实时可读）
├── clip.rs               # 自动化片段（点集 + 插值）
├── lane.rs               # 通道上的自动化轨
├── player.rs             # 实时求值 → 无锁写入参数
├── modulator.rs          # LFO / 包络发生器 / 峰值控制器
└── recorder.rs           # 旋钮移动 → 写入片段
```

**Dart 侧**（`lib/automation/`）
- `ParameterId` 镜像（字符串 ↔ 整数哈希，Rust 侧用 `u32`）
- 自动化片段的**编辑 UI**（绘制、拖拽、曲线张力）
- 录制模式的开关与状态显示

**核心要求**

1. **寻址**：Dart 侧构造 `ParameterId` 形如 `channel/<id>/volume`，
   传入 Rust 时映射为紧凑的 `(kind: u16, index: u32, sub: u32)` 三元组，**热路径不做字符串哈希**。
2. **求值顺序**：基础值 → 自动化 → 调制器累加 → 钳制。顺序固定且文档化。
3. **平滑**：所有参数变更经一阶低通（可配 1–50ms）避免 zipper noise。
4. **录制模式**：Touch / Latch / Write 三种，行为对齐行业惯例。
5. **曲线**：点间插值支持 线性 / 保持 / 曲线（张力 -1..1）。
6. **实时安全**：求值路径零分配；自动化点在加载时预排序并构建索引。

**验收**
- 对混音器音量画一条自动化曲线，播放时曲线正确回放，无 zipper noise
- 实时移动旋钮（Write 模式）能录制出自动化点
- 1000 个自动化点的工程，求值开销实测 < 2% CPU
- `assert_no_alloc` 通过

---

### S3 — 混音器（Rust）

**目标**：完整混音台，替代当前"每轨一个音量条"的简化实现。**实现于 Rust**。

**Rust 模块**（`native/zenith_core/src/mixer/`）

```
mixer/
├── mod.rs
├── channel.rs             # 通道（增益/声像/静音/独奏/相位）
├── strip.rs               # 通道条 DSP（增益→声像→效果链→发送）
├── bus.rs                 # 总线路由
├── send.rs                # 发送（推子前/后）
├── effect_chain.rs        # 效果槽位管理（10 槽）
├── meter.rs               # 峰值/RMS 电平（原子写入）
├── pan_law.rs             # 声像法则
└── graph.rs               # 混音器拓扑 → DSP 图（含环检测）
```

**Dart 侧**：混音器 UI（通道条、推子、旋钮、电平表），复用 `rotary_knob.dart`。

**核心要求**

1. **通道数**：默认 64 插入 + 8 返回 + 1 主控；可为工程配置更多。**预分配**，不在实时路径扩容。
2. **路由**：任意通道可输出到任意总线/返回；支持分组嵌套（深度 ≥ 4，**建图时做环检测**）。
3. **发送**：每通道 4 个 Send，各自独立开关 + 电平 + 推子前/后。
4. **效果槽**：每通道 10 个，支持重排、旁通、湿/干、串联。
5. **侧链**：任意通道可作为任意效果的侧链源。
6. **增益分级**：内部以 dB 显示，-INF..+12dB；推子为 dB 曲线而非线性。
7. **电平**：每通道 + 主控，峰值与 RMS 双表，3 秒峰值保持；Rust 原子写入，Dart 无锁读。
8. **序列化**：混音器状态写入工程 `spec/project.json`；旧工程按"每轨=一个通道"自动迁移。

**验收**
- 建 16 条通道、若干发送/返回，播放无爆音、无相位问题
- 环路由检测正确拒绝成环连接
- 旧工程打开后，原音量/声像/静音/独奏**无损迁移**到新混音器
- `assert_no_alloc` 通过

---

### S4 — 离线渲染、冻结、导出、PDC（Rust）

1. **PDC（插件延迟补偿）**：每个效果报告自身延迟（`latency_samples()`），
   Rust 侧 `mixer/graph.rs` 自动对齐各路径，**含发送路径**。
2. **轨道冻结**：轨道可冻结为音频，释放 CPU；冻结后可解冻还原。
3. **渲染为音频**：轨道 → 音频轨，保留原轨（可选隐藏）。
4. **导出**：
   - 主混音导出 WAV / FLAC / MP3
   - 逐轨导出（分轨）
   - 支持导出范围、尾部尾音处理
   - 可指定采样率/位深/抖动
5. **离线渲染引擎**：**复用同一套 Rust DSP 图**，但走 `OfflineDriver`——
   以更大缓冲块高速运行，**不使用 `cpal`**。同一份 DSP 代码，两种驱动。
   - 这是 Rust 架构的核心收益：**实时与离线保证逐样本一致**，不存在两套实现漂移。
   - 离线渲染可跨线程并行（按轨道/按总线切分），满足重负载导出。
6. **导出进度**：Rust 侧原子写入进度，Dart 侧轮询显示；支持取消。

**验收**
- 导出结果与实时播放逐样本对齐（容差 < -90dBFS）
- 含延迟插件的工程，导出后**声音与实时一致**（PDC 生效）
- 128 轨重负载工程导出能跑满多核
- 含延迟插件的工程，导出后**声音与实时一致**（PDC 生效）

---

### S5 — 内置效果器套件（Rust）

**目标**：全部为**实时 Rust DSP**，可作为通道效果、发送效果或插件槽内容。

**Rust 目录结构**（`native/zenith_core/src/effects/`）

```
effects/
├── mod.rs
├── registry.rs                  # 注册表：ID → 工厂（预分配实例池）
├── eq/parametric.rs             # ≥7 段，含频响计算
├── eq/spectrum.rs               # FFT 频谱分析（供 UI）
├── dynamics/compressor.rs       # 含侧链、前视
├── dynamics/limiter.rs
├── dynamics/gate.rs
├── reverb/algorithmic.rs        # 算法混响（FDN/Schroeder）
├── reverb/convolution.rs        # 卷积混响（加载 IR，分区 FFT）
├── delay/sync_delay.rs          # 同步宿主速度，含乒乓
├── modulation/chorus.rs
├── modulation/flanger.rs
├── modulation/phaser.rs
├── distortion/saturation.rs
├── distortion/bitcrush.rs
├── filter/multimode.rs
└── util/oversampling.rs         # 过采样工具（4x/8x）
```

**统一 trait**

```rust
pub trait EffectProcessor: Send {
    fn prepare(&mut self, sample_rate: u32, max_block: usize, channels: usize);
    fn process(&mut self, buf: &mut AudioBuffer, ctx: &RenderContext);   // 实时安全
    fn reset(&mut self);
    fn latency_samples(&self) -> usize;                                  // 供 PDC
    fn parameters(&self) -> &[ParameterDescriptor];                      // 自动接入 S2
    fn set_parameter(&mut self, id: ParamId, value: f32);
}
```

**Dart 侧**：每个效果一个 UI 面板（复用 `rotary_knob.dart`），
参数清单由 Rust 通过 FFI 查询得到，**UI 自动生成**，无需为每个效果手写 Dart 类。

**核心要求**
- 每个效果**无内存分配**（`prepare` 一次性预分配所有缓冲，含 IR 与 FFT 暂存）
- 每个参数自动注册到参数存储，**自动获得自动化与调制能力**
- 每个效果有 Rust 单元测试：脉冲 / 白噪声 / 正弦输入下的输出正确性
- **SIMD**：热点（EQ、卷积、饱和）用 `std::simd` 或平台 intrinsics，
  `cfg` 分平台，`wasm32` 用 `simd128`
- **过采样**必须走统一工具，避免各效果各写一套

**验收**
- 所有效果在 256 帧缓冲下零分配（`assert_no_alloc`）
- 每个效果的 `latency_samples` 准确，PDC 后多轨对齐
- 128 轨各挂 3 个效果，实时率 < 50%

---

### S6 — 编曲结构、钢琴卷帘、MIDI

**S6a — Pattern + Playlist 双层结构**

- 新增 `models/pattern.dart`：`Pattern{id, name, notes, lengthTicks, color}`
- 新增 `models/playlist.dart`：`PlaylistItem{patternId, startTicks, lengthTicks, trackIndex}`
- Playlist 上拖拽摆放样式块；同一 Pattern 多处引用自动同步
- Pattern 克隆：linked（共享）/ unique（独立）
- 工程格式：新增 `patterns`/`playlist` 字段；旧工程的 `Track.notes` 迁移为**一个 Pattern 一条轨道**的等价结构

**S6b — 钢琴卷帘增强**

- tick 化网格、吸附可选（1/1 … 1/32、三连音）
- 量化：强度 0–100%、摇摆比例
- 力度：画笔、斜坡、随机、压缩/扩展
- 音符工具：画笔、擦除、切片、滑音、静音、选择框
- 和弦/音阶辅助：**保留并增强现有 `chord_service.dart` 的旋律锚定和声能力**
- 幽灵音符、音阶高亮

**S6c — MIDI**

- `services/midi/smf_reader.dart` / `smf_writer.dart`（SMF 0/1）
- MIDI 输入：`flutter_midi_command` 或 FFI 直连系统 MIDI
- MIDI 输出、时钟同步、通道过滤
- 替换 `menu_bar.dart:150` 的 `'MIDI import not yet implemented'` 占位

**验收**
- 导入一个多轨 MIDI 文件 → 正确生成多轨道多 Pattern
- 导出 MIDI 后能被通用音序器正确读取
- 外部键盘弹奏可录入卷帘

---

### S7 — 插件宿主

**目录**（`lib/plugins/`）

```
lib/plugins/
├── plugin_host.dart          # 抽象
├── plugin_descriptor.dart
├── clap/                     # CLAP 宿主实现
│   ├── clap_ffi.dart
│   ├── clap_instance.dart
│   └── clap_ui.dart          # 外部窗口嵌入
├── native/                   # 自研插件 ABI
│   └── native_abi.dart
└── plugin_scanner.dart       # 扫描/缓存插件库
```

**核心要求**
- **不使用 VST**（许可冲突）——用 **CLAP**（MIT）或自研 ABI
- 插件参数桥接到参数存储，自动获得自动化
- 插件延迟桥接到 PDC
- 插件状态（预设）随工程序列化
- 沙箱化：插件崩溃不得拖垮宿主（子进程隔离，P1）
- **跨平台边界**：CLAP 宿主仅对桌面端启用；移动与 Web 端**不加载外部插件**，
  改为使用 S5 的内置效果（这正是 S5 必须完整的原因）

**验收**
- 加载一个 CLAP 插件，参数可自动化，延迟被补偿，预设随工程保存
- 插件崩溃时宿主不崩溃，弹出提示并可移除该插件

---

### S8 — 音频编辑（Rust DSP + Dart UI）

1. **非破坏性编辑**：片段引用源文件 + 偏移 + 增益包络 + 淡入淡出，不改源
2. **实时时间拉伸/变调**：WSOLA 或相位声码器（Rust），独立于宿主速度
3. **交叉淡化**：任意两个片段重叠处自动/手动淡化曲线
4. **音频切片**：瞬态检测 → 切片 → 映射到卷帘音符
5. **音频量化**：瞬态对齐网格
6. **波形编辑器**（P2）：破坏性编辑，独立窗口

> 拉伸/切片算法在 Rust 中实现，桌面可多线程；**Web 端拉伸列为重型渲染**，
> 受 S1.5 降级策略管辖（L1 降级时不可用）。

**验收**
- 拉伸 ±50% 无明显金属音
- 切片后可直接用卷帘重排节奏

---

### S9 — 收尾

1. **A/B 对比**：两份工程状态快速切换
2. **性能**：128 轨 + 多效果的压力测试，目标实时率 < 50%
3. **文档**：用户手册、快捷键表、架构文档、**Rust 核心开发指南**
4. **迁移工具**：旧 `.zap` / `.zaproj` 一键迁移向导
5. **移除旧依赖**：`media_kit` 彻底下线（确认无引用后）
6. **构建固化**：确认 `hook/build.dart` 在六平台 CI 上稳定产出原生库

---

## 4. 多 Agent 协作规范

### 4.1 分工建议

| 会话 | 负责范围 | 依赖 | 主要动到的目录 |
|---|---|---|---|
| **Agent-0** | S0 基础重构（独占） | 无 | 全仓（拆分）+ `native/zenith_core/` 骨架 |
| **Agent-A** | S1 Rust 音频核心 + S4 离线渲染 | S0 | `native/zenith_core/src/{engine,driver,transport,voice,dsp}/`, `lib/engine/` |
| **Agent-B** | S1.5 Web 接入与降级 | S1 | `native/zenith_core/src/driver/worklet_driver.rs`, `lib/engine/web/` |
| **Agent-C** | S2 参数/自动化 + S5 效果器 | S1 | `native/zenith_core/src/{automation,effects}/`, `lib/automation/` |
| **Agent-D** | S3 混音器 + S7 插件宿主 | S1 | `native/zenith_core/src/mixer/`, `lib/mixer/`, `lib/plugins/` |
| **Agent-E** | S6 编曲结构 + 钢琴卷帘 + MIDI | S0 | `lib/models/`, `lib/widgets/editor/`, `lib/services/midi/` |
| **Agent-F** | S8 音频编辑 + S9 收尾 | S3, S6 | `native/zenith_core/src/{stretch,slicing}/`, `lib/widgets/editor/` |

> **Rust 侧目录隔离是关键**：C、D、F 三个会话同时改 `native/zenith_core/src/`，
> 必须严格限定在各自子目录内。**共享文件只有 `lib.rs`、`Cargo.toml` 和 `ffi/`**，
> 这三个文件的修改必须串行（先在 `docs/COORDINATION.md` 登记，后改）。

### 4.2 协作纪律

1. **接口先行**：S0 定义的抽象接口与 **C ABI 头**是**契约**。任何 agent 需要变更
   接口，必须先在 `docs/COORDINATION.md` 提案并标注影响范围。
2. **FFI 边界契约**：`ffi/types.rs` 中的 `#[repr(C)]` 结构体与
   `lib/engine/ffi/native_types.dart` 的 `final class` 镜像**必须字段顺序完全一致**。
   改一侧必须同时改另一侧，并在测试中加一项**结构体大小断言**（`size_of` 双向核对）。
3. **文件所有权**：同一文件**同时只有一个 agent 可写**。跨阶段共享的文件
   （`models/project.dart`、`lib.rs`、`Cargo.toml`、`ffi/`）变更须登记。
4. **分支策略**：每阶段一个分支 `feat/sN-<name>`；S0 直接进 `master` 后打 tag
   `s0-baseline`，后续分支从该 tag 派生。
5. **提交前检查**（每个 agent 必做）：
   ```
   cargo clippy --all-targets -- -D warnings   # Rust 零 warning
   cargo test                                  # Rust 测试全绿
   flutter analyze                             # 零 error
   flutter test                                # 全绿
   ```
6. **不引入竞品名称**：提交前自查，CI 加一条 grep 规则拦截品牌名。
7. **实时安全自查**（Rust）：
   - `native/zenith_core/src/{engine,driver,transport,voice,dsp,mixer,effects,automation}/`
     下的 `process` 路径禁止 `Vec::push`、`Box::new`、`String`、`Mutex`、`println!`
   - 用 `#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]`
   - 用 `assert_no_alloc` 在测试中强制验证
8. **跨平台自查**：Rust 核心代码禁止直接使用 `std::thread`、`std::fs`、`std::time::Instant`
   （除 `cfg(not(target_arch = "wasm32"))` 保护的部分）。
   **提交前必须能通过 `cargo check --target wasm32-unknown-unknown`**。

### 4.3 每阶段的交付物

- 代码 + 单元测试（**Rust 侧 `cargo test`，Dart 侧 `flutter test`**）
- 该阶段的 `docs/stages/sN-report.md`：做了什么、遗留什么、接口变更、验证结果
- 更新本文件底部的 **进度表**
- **若改动了 C ABI**：同步更新 `docs/ABI.md`（结构体布局与函数签名清单）

---

## 5. 风险登记

| 风险 | 影响 | 缓解 |
|---|---|---|
| FFI 音频后端在 6 平台表现不一 | 高 | S1 早期在 Windows/macOS/Linux 各跑冒烟；移动端次之；Web 走独立驱动 |
| **WASM 目标编译失败**（依赖了 `std::thread`/`fs`） | 高 | 核心禁止平台依赖；**每次提交强制 `cargo check --target wasm32-unknown-unknown`** |
| panic 跨 FFI 边界（UB） | 高 | 所有 `extern "C"` 用 `catch_unwind` 包裹并转为错误码 |
| Dart/Rust 结构体镜像漂移 | 高 | `size_of` 双向断言测试 + 字段顺序契约（4.2 第 2 条） |
| 实时线程分配导致爆音 | 高 | 严格零分配 + `assert_no_alloc` 在 CI 中强制 |
| Rust 工具链在 CI 上缺失 | 中 | CI 各平台 job 增加 `rustup target add <triple>` 步骤 |
| native_assets hook 在六平台不稳定 | 中 | S0 阶段先验证 Windows，其余平台在 S1 逐平台打通 |
| tick 化破坏旧工程 | 中 | 双写影子字段一个版本周期 + 迁移测试 |
| 三会话同时改 Rust 核心冲突 | 中 | 子目录所有权隔离 + `lib.rs`/`ffi/` 串行修改 |
| Web 降级策略误判 | 中 | 阈值可配置；警告不可自动消失；降级可逆且需用户确认 |
| 范围蔓延 | 中 | 严格遵守优先级标注，P2 可延后 |

---

## 6. 进度表（各会话完成后更新）

| 阶段 | 状态 | 负责人 | 完成日期 | 备注 |
|---|---|---|---|---|
| S0 基础重构 | ⬜ 未开始 | Agent-0 | — | 含 Rust 工作区骨架 |
| S1 Rust 音频核心 | ⬜ 未开始 | Agent-A | — | ★关键路径，含 WASM 目标 |
| S1.5 Web 接入与降级 | ⬜ 未开始 | Agent-B | — | 警告条 + 主动停用重型渲染 |
| S2 参数/自动化 | ⬜ 未开始 | Agent-C | — | |
| S3 混音器 | ⬜ 未开始 | Agent-D | — | |
| S4 渲染/导出/PDC | ⬜ 未开始 | Agent-A | — | |
| S5 效果器套件 | ⬜ 未开始 | Agent-C | — | |
| S6 编曲/卷帘/MIDI | ⬜ 未开始 | Agent-E | — | |
| S7 插件宿主 | ⬜ 未开始 | Agent-D | — | 桌面限定 |
| S8 音频编辑 | ⬜ 未开始 | Agent-F | — | |
| S9 收尾 | ⬜ 未开始 | Agent-F | — | |

**图例**：⬜ 未开始 · 🟡 进行中 · ✅ 已完成 · ⛔ 阻塞

---

## 7. 第一个动作（给下一个会话）

**只有 Agent-0 可以先动手**，执行 S0：

1. 读 `lib/services/synth_engine.dart`、`lib/providers/project_provider.dart`、`lib/models/instrument.dart`
2. 按 3.S0 的清单拆分文件（**纯移动代码，不改逻辑**）
3. 新增 `models/musical_time.dart` 与 tick 换算
4. 在 `models/project.dart` / `models/track.dart` 加可选扩展字段
5. **建立 Rust 工作区骨架**（本次新增，是 S1 的基座）：
   - `native/zenith_core/Cargo.toml`，`crate-type = ["staticlib", "cdylib", "rlib"]`
   - `native/zenith_core/src/lib.rs` 放一个最小 `extern "C" fn zenith_version() -> u32`
   - 建立 `hook/build.dart`（native_assets），驱动 `cargo build`
   - **验证链路**：`flutter build windows --debug` 能链接并调用该函数
6. 建立 Dart 侧抽象接口：`lib/engine/engine.dart`、`lib/automation/parameter.dart`、
   `lib/plugins/plugin_host.dart`（只有接口，无实现）。
   **效果器不建 Dart 抽象类**——效果器全部实现在 Rust（S5），
   Dart 侧只通过 FFI 查询参数清单并自动生成 UI。
7. 跑 `cargo clippy` + `cargo test` + `flutter analyze` + `flutter test`，
   全绿后提交并打 tag `s0-baseline`

**S0 完成前，其他会话不要开始写代码**——它们都依赖 S0 的接口与 C ABI 契约。

**工具链现状**（已核实）：`rustc 1.96.0` / `cargo 1.96.0` 已安装；
`build/native_assets/windows/native_assets.json` 存在且为空（`{"native-assets":{}}`），
可直接接入；CI（`.github/workflows/build.yml`）已覆盖 6 平台，需为各 job 补
`rustup target add` 步骤。

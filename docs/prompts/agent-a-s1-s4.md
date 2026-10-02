# 启动提示词 · Agent-A（S1.1 Rust 引擎主体 + S4 离线渲染）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-A，负责「卓声」DAW 项目的 S1.1（Rust 实时引擎主体）与后续 S4（离线渲染/导出/PDC）。

工作目录：`F:\exeliang\zenith_audio`

**必读（按顺序）**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 硬性约束、§0.3 技术选型、§3 的 S1.1 与 S4 全节
2. `docs/ABI.md` — C ABI 契约，原则 P1–P10 不可违反
3. `docs/stages/s1-report.md` — 你自己的上游：S1.0 前置项报告
4. `lib/engine/engine.dart`（`AudioEngine` 接口）、`lib/engine/audio_engine_adapter.dart`（适配层）
5. `native/zenith_core/src/lib.rs`、`src/ffi/`（现有导出面）

**当前状态（已核实，2026-10-04）**
- **S0、S1.0 前置项 A/B/C 已全部完成**：`panic="abort"` 已移除且 `catch_unwind`
  经 `zenith_panic_probe` 验证；`audio_engine_adapter.dart` 已建，34/34 调用点已迁到
  `AudioEngine`，`flutter test` 122/122。
- S2（automation）、S3（mixer Rust 侧）、S5（effects 大部）已由其他会话完成，
  `src/automation/`、`src/mixer/`、`src/effects/` 都已存在并被测试钉住——
  **不要动这些目录**，它们不是你的所有权。
- ABI_VERSION 现为 **0.4.0**。你新增导出时递增到 **0.5.0**（先在
  `docs/COORDINATION.md` 登记）。
- `src/engine/` 与 `src/driver/` **尚不存在**——这就是你的任务。

## 任务 S1.1 — Rust 引擎主体

按 `PLAN` §3 S1.1 的 crate 结构，新增：

```
native/zenith_core/src/
├── engine/     graph.rs（有向图+拓扑排序+环检测）、node.rs（DspNode trait）、
│               render_context.rs、realtime.rs（无锁队列/预分配池）
├── driver/     mod.rs（AudioDriver trait）、cpal_driver.rs（cfg 非 wasm）、
│               worklet_driver.rs（cfg wasm，可先留骨架给 Agent-B）
├── transport/  transport.rs、sequencer.rs、event_queue.rs
└── voice/      voice_allocator.rs、sampler.rs、synth_voice.rs（移植现有 Dart 合成引擎）
```

要点（全部是硬性要求）：
- `AudioDriver` trait 抽象回调来源；桌面/移动 = `cpal`，Web = wasm32 + AudioWorklet
  （`worklet_driver.rs` 可只留骨架，Agent-B 会接管，但 trait 签名必须先定且文档化）
- 音频回调路径**绝对零分配、零锁、零 IO**；用 `assert_no_alloc` 在测试中强制
- 每块以传入 `n_frames` 为准，**不得假设固定块大小**
- 所有 `extern "C"` 入口 `catch_unwind` 保护（现在真正生效了）；失败返回错误码，
  不返回 null
- 每个新 `#[repr(C)]` 结构体导出配套 `zenith_sizeof_<T>()`
- sequencer 的 tick 语义与 `lib/models/musical_time.dart`（PPQ = 960）一致
- **接线**：S2 已留好接入点 `zenith_automation_advance_block()`（块边界调一次）；
  S3 的 mixer graph、S5 的 effects registry 同样按块接入——你的图是把这三者串起来的骨架
- Dart 侧：把 `audio_engine_adapter.dart` 的委托目标从 `AudioService` 换成 FFI 引擎
  （第二步才做；`AudioService` 保留为回退路径，**S4 验证通过前不删**）
- 锁 256 帧 @48kHz，实测延迟 ≤ 12ms

## 任务 S4（S1.1 验收后再做）

- PDC：每个效果报告 `latency_samples()`（S5 的 trait 已有），mixer graph 自动对齐各路径含发送
- 轨道冻结 / 渲染为音频
- 导出：主混音 WAV/FLAC/MP3 + 逐轨导出 + 范围/尾音/采样率/位深/抖动
- 离线渲染走 `OfflineDriver`，与实时共用同一套 DSP 图（逐样本一致，容差 < -90dBFS）
- 离线渲染可多线程并行

## 硬性约束

- **不引入任何第三方 DAW 品牌名**（代码/注释/文档/UI 全不许）
- **禁止 VST**（许可与 AGPL-3.0 冲突）
- effects 文件保持纯 ASCII（历史教训：PowerShell 写文件会破坏 UTF-8；
  只用 write/edit 工具改源码）
- 改 `src/lib.rs` / `Cargo.toml` / `src/ffi/` 前，先在 `docs/COORDINATION.md` 登记
- 每次提交前 `cargo check --target wasm32-unknown-unknown` 必须通过
- 合并前四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error（本机较慢，建议后台跑）
  flutter test
  ```

## 验收（S1.1）

- 64 音复音合成器轨道，CPU < 15%（单核），无爆音
- `assert_no_alloc` 60 秒播放零触发
- 拖动播放头、循环跳转无咔哒声
- 延迟 ≤ 12ms（256 帧 @48kHz）
- Windows/macOS/Linux 冒烟播放通过；wasm32 编译通过

完成后交付 `docs/stages/s1.1-report.md`（S4 另出 `s4-report.md`），更新 `PLAN` §6 进度表。

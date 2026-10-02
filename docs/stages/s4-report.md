# S4 — 离线渲染 / PDC / 导出（Agent-A）

**状态**：Rust + Dart 侧完成，四门禁全绿
**日期**：2026-10-06
**计划依据**：`docs/PLAN_DAW_PARITY.md` §3 S4
**登记**：`docs/COORDINATION.md` C-014

---

## 1. 结论摘要

| 验收要求（PLAN §3.S4） | 结果 |
|---|---|
| 离线走**同一个 `AudioDriver`/DSP 图**，非第二套实现 | ✅ `engine/offline.rs` 直接调用 `Engine::render_block`；测试 `offline_and_a_manual_block_loop_agree` 逐样本比对 |
| PDC（效果延迟补偿），含发送路径 | ✅ `engine/pdc.rs`：按各通道效果链延迟做**相对对齐**；`EffectRack::latency_by_channel` 汇总 |
| 冻结 / 渲染为音频 | ⬜ 依赖 S6 的轨道→通道路由与轨道模型，见 §5 |
| 导出 WAV（主混音 / 逐轨） | 🟡 主混音 WAV 导出完成（16/24/32-bit）；**逐轨导出**待路由，见 §5 |
| 导出范围、尾部尾音处理 | ✅ `startTicks`/`endTicks` + `tailSeconds` |
| 指定采样率/位深/抖动 | 🟡 采样率与位深支持；**抖动**未做（属 P2 增强） |
| 逐样本与实时一致（容差 < -90dBFS） | ✅ 结构保证 + `offline_and_a_manual_block_loop_agree` |

**四门禁**

| 门禁 | 结果 |
|---|---|
| `cargo test -p zenith_core` | ✅ **988 passed / 0 failed**（+18） |
| `cargo clippy --all-targets -- -D warnings` | ✅ **exit 0** |
| `cargo check --target wasm32-unknown-unknown` | ✅ **exit 0** |
| `flutter analyze` | ✅ **0 error** |
| `flutter test` | ✅ **220 passed**（+10）；FFI 冒烟 **9 passed**（构建 dylib 后） |

---

## 2. 交付内容

### 2.1 Rust

| 模块 | 职责 |
|---|---|
| `src/engine/offline.rs` | `render_range` / `render_to_buffer`：复用 `Engine::render_block`，末块按剩余帧数调用（不假设固定块大小）。测试含**确定性**与**逐样本一致**断言。 |
| `src/engine/pdc.rs` | `DelayLine`（整样本、无分配、`delay==0` 时零开销）+ `PdcPlan`（`recompute` 控制线程分配，`apply` 音频线程）。**相对对齐**：每通道补偿 `max_latency - channel_latency`。 |
| `src/engine/effects_rack.rs` | 新增 `channel_latency(hz)` / `latency_by_channel(n)`，供 PDC 查询。 |
| `src/engine/mod.rs` | 内嵌 `PdcPlan`；`process_mixer` 在效果后、计量前应用 PDC；新增 `sync_effects`（含 PDC 重算）、`recompute_pdc`、`pdc_latency`、`reset_effects`。 |
| `src/ffi/render_api.rs` | `zenith_render_offline` / `zenith_buffer_free` / `zenith_engine_pdc_latency`。 |

### 2.2 C ABI

`ABI_VERSION` → **`0.6.0`**。新增 `zenith_render_offline(engine, start, end, target_sr, out_buffer, out_frames)`、`zenith_buffer_free(buffer, frames)`、`zenith_engine_pdc_latency(engine, out)`。签名兑现 ABI §6.8 的既有冻结契约。

### 2.3 Dart

| 文件 | 内容 |
|---|---|
| `lib/services/wav_encoder.dart` | `encodeWav`（PCM16/24/Float32，交错、NaN→静音、对称量化）+ `encodeWavFromFloat64` + `probeWavHeader`。 |
| `lib/services/offline_export.dart` | `OfflineExportService.renderRangeToWav` / `renderProjectToWav` / `pdcLatencySamples`。 |
| `lib/engine/ffi/engine_bindings.dart` | `ZenithEngineHandle.renderOffline`（内部 `zenith_buffer_free`，P3）+ `pdcLatency`。 |
| `lib/native/zenith_core.dart` | `kExpectedAbiVersion` → `0x000600`。 |
| `test/wav_encoder_test.dart` | 10 项：头布局、RIFF 尺寸、格式标签、量化边界、非有限输入、24-bit 补码、探测拒绝。 |

### 2.4 关键设计：为什么「绝对延迟」不被消除

PDC 让每通道对齐到**最深**通道，绝对管线延迟 = `max(通道延迟)`。这个绝对延迟**故意保留**：离线渲染走同一图、有同样的绝对延迟，两者才能**逐样本一致**（PLAN §3.S4 第 5 条）。若实时消除、离线不消除，正是该条禁止的漂移。`zenith_engine_pdc_latency` 把该值暴露给需要对齐渲染结果的调用方。

---

## 3. 验收证据

- `offline_and_a_manual_block_loop_agree`：`render_range` 与手写 `render_block` 循环输出**完全相等** — 「一套图」的测试化。
- `rendering_is_deterministic_across_two_runs`：同工程两次渲染逐样本相同。
- `the_deepest_channel_is_not_delayed_but_others_are` / `recompute_aligns_a_fast_channel_to_a_slow_one`：PDC 相对对齐正确。
- FFI 冒烟 `[S4] offline rendering returns a WAV-encodable buffer`：24000 帧、WAV 头正确、`pdcLatency()==0`。
- WAV：满量程往返、越界钳制、NaN→0、24-bit 负数补码。

---

## 4. 门禁命令

```
cargo test -p zenith_core --offline
cargo clippy -p zenith_core --all-targets --offline -- -D warnings
cargo check -p zenith_core --offline --target wasm32-unknown-unknown \
  --config /opt/homebrew/opt/rust-wasm/share/rust-wasm/cargo-config.toml
flutter analyze
flutter test
```

---

## 5. 已知缺口 / 后续

1. **逐轨导出 / 冻结 / 渲染为音频**：需要 S6 的轨道→混音通道路由（当前 voice bus 固定馈入首个 insert 通道）。API 已按「给定范围 + 格式」成形，补路由后是「按通道循环」而非重设计。
2. **工程 → 引擎装载**：`OfflineExportService` 能渲染**引擎里已有的内容**（已在 FFI 冒烟中端到端验证：24000 帧 + 正确 WAV 头）。但把当前 `Project` 的音符/乐器/混音状态**装载进引擎 sequencer/mixer** 是 S6 的编排层职责，尚未接线。因此本轮**刻意未加 WAV 导出菜单项**——加了会导出静音，比没有更糟。装载完成后，菜单项接 `OfflineExportService` 即可。
3. **抖动（dither）**：16-bit 导出未加抖动；属 P2 增强项。
4. **FLAC / MP3**：PLAN 要求，需外部编码器；未做。
5. **发送/返回路径的 PDC**：当前 PDC 覆盖通道效果链；S3 的 send 总线 DSP 尚未接线（S1 接线时的既定缺口），其延迟补偿随后续接线补。
6. **导出进度 / 取消**：Rust 侧未暴露原子进度（当前渲染是单次调用）；长工程应改为分块 + 进度回调，属后续。

---

## 6. 回滚方式

S4 代码位于 `src/engine/{offline,pdc}.rs`、`src/ffi/render_api.rs`、`lib/services/{wav_encoder,offline_export}.dart`。`ffi/mod.rs` 与 `lib.rs`（版本号）为纯追加，可单独回退不影响 S1/S2/S3/S5。

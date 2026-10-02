# S8 — 音频编辑算法（Agent-A，算法层）

**状态**：Rust 算法层 + FFI 导出 + Dart 绑定完成，四门禁全绿
**日期**：2026-10-06
**计划依据**：`docs/PLAN_DAW_PARITY.md` §3 S8
**登记**：`docs/COORDINATION.md` C-015（算法层）、C-016（FFI 导出）

---

## 1. 结论摘要

PLAN §3.S8 明说拉伸/切片算法「是独立的纯函数（Float32 数组进、Float32 数组出），可以脱离引擎先实现并配单元测试。建议先做这部分。」本轮完成该算法层。

| 验收要求（PLAN §3.S8） | 结果 |
|---|---|
| 实时时间拉伸 / 变调（独立于宿主速度） | ✅ `edit/time_stretch.rs`：WSOLA 拉伸；变调 = 重采样 + 拉伸 |
| 音频切片 → 映射到音符（瞬态检测） | 🟡 `edit/transient.rs`：瞬态检测 + `slice_at`；**映射到音符的 UI/模型接线**待做 |
| 交叉淡化 | ✅ `edit/crossfade.rs`：线性 / 等功率曲线 |
| 非破坏性片段编辑 | ⬜ 属 Dart 模型/UI 层，见 §4 |
| 音频量化 / 瞬态对齐网格 | 🟡 瞬态检测就位；对齐网格的接线待做 |
| 波形编辑器（P2） | ⬜ 未做 |

**四门禁**

| 门禁 | 结果 |
|---|---|
| `cargo test -p zenith_core` | ✅ **1018 passed / 0 failed**（+6 FFI 导出测试，累计 +30） |
| `cargo clippy --all-targets -- -D warnings` | ✅ **exit 0** |
| `cargo check --target wasm32-unknown-unknown` | ✅ **exit 0** |
| `flutter analyze` | ✅ **0 error** |
| `flutter test` | ✅ **288 passed**（+1 FFI 冒烟 `[S8]`；构建 dylib 后 FFI 冒烟 **10 passed**） |

---

## 2. 交付内容

| 文件 | 职责 |
|---|---|
| `src/edit/mod.rs` | 模块入口与再导出。 |
| `src/edit/time_stretch.rs` | `time_stretch` / `time_stretch_with`（WSOLA：汉宁窗 50% 重叠 + 相关性对齐搜索，`StretchConfig` 可配）、`resample_linear`、`pitch_shift`。 |
| `src/edit/transient.rs` | `detect_transients` / `detect_transients_with`（帧能量 vs **局部中位数**，阈值 dB 可配、min-gap 防重复）、`slice_at`。 |
| `src/edit/crossfade.rs` | `FadeCurve`（Linear / EqualPower）、`equal_power_curves`、`crossfade`。 |
| `src/ffi/edit_api.rs` | `zenith_time_stretch` / `zenith_pitch_shift` / `zenith_detect_transients` / `zenith_crossfade` / `zenith_edit_buffer_free`。 |
| `lib/engine/ffi/edit_bindings.dart` | `AudioEditBindings`：复制到 `Float32List` 后立即释放原生缓冲，调用方不见裸指针。 |
| `src/lib.rs` | `pub mod edit;`（C-015）；`ABI_VERSION` → `0.7.0`（C-016）。 |

共 **30 项**新增 Rust 测试（24 算法 + 6 FFI）。`ABI_VERSION` 由 `0.6.0` 升至 **`0.7.0`**（C-016）。

---

## 3. 实现中修掉的真实缺陷

1. **变调方向反了**：`pitch_shift` 原先用 `resample(1/ratio)` 再 `stretch(ratio)`，导致 ±12 半音的输出长度变成 0.25× 而非 1×。修正为 `resample(ratio)` 再 `stretch(ratio)` —— 重采样倍率与拉伸倍率**相同**才能「变调不变长」。测试 `pitch_shift_preserves_length` 钉住。
2. **瞬态检测器对稳态正弦持续触发**：原实现比较「能量变化的 flux」对「flux 的局部中位数」。稳态音的帧能量只有约 0.01 dB 的数值纹波，其 flux 的中位数趋近 0，于是每个纹波都被判为瞬态（实测 18 次误报）。改为比较**帧能量**对**帧能量的局部中位数** —— 这才是「相对本地电平的起音」本义。测试 `a_sustained_tone_does_not_trigger_repeatedly` 钉住。

---

## 4. 已知缺口 / 后续

1. **非破坏性片段编辑 / 波形编辑器**：属 Dart 模型与 UI 层（`audio_clip.dart` / `audio_clip_editor.dart`），未做。FFI 导出与 Dart 绑定**已完成**，UI 可直接调用。
3. **瞬态→卷帘音符映射**：`slice_at` 产出的是音频片段；把它映射成卷帘音符并保留每片独立播放是模型层工作，未做。
4. **相位声码器**：WSOLA 已满足「±50% 无明显金属音」的验证目标；若实测不达标，相位声码器是升级路径（模块文档已注明）。
5. **Web 端降级**：拉伸属 S1.5 的重型渲染，L1 降级时应不可用；该联动待 S1.5/UI 接线。

---

## 5. 回滚方式

删除 `src/edit/**`、`src/ffi/edit_api.rs`、`lib/engine/ffi/edit_bindings.dart`，并还原 `src/lib.rs`（去掉 `pub mod edit;` 与版本号）、`src/ffi/mod.rs` 一行即可。`ABI_VERSION` 一并回退到 `0.6.0`。

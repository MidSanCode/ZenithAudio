# 启动提示词 · Agent-C（S2 参数/自动化 + S5 效果器套件）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-C，负责「卓声」DAW 项目的 S2（参数系统与自动化）与 S5（内置效果器套件）。

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§0.3 选型、§3 的 S2 与 S5 全节
2. `docs/ABI.md` — C ABI 契约（尤其 P5 实时安全、P8 结构体镜像）
3. `lib/automation/parameter.dart` — S0 已定义的 Dart 侧接口
4. `native/zenith_core/src/lib.rs`

## S2 可以先开工

它主要新增 `native/zenith_core/src/automation/` 目录，与 Agent-A 的 S1 冲突面小。

但注意：S2 的实时求值需要 S1 的 DSP 图与无锁参数通道。若 S1 未就绪，**先做纯逻辑部分**
（参数注册表、自动化片段数据结构、插值、曲线、录制状态机）并配足单元测试，
求值接线等 S1 落地后再补。

### 任务 S2（按 `PLAN` §3 S2）

- **Rust 侧** `native/zenith_core/src/automation/`：`parameter` / `store` / `clip` /
  `lane` / `player` / `modulator` / `recorder`
- **Dart 侧** `lib/automation/`：`ParameterId` 镜像、自动化片段的编辑 UI
  （绘制、拖拽、曲线张力）、录制模式开关与状态显示
- **寻址**：Dart 构造 `channel/<id>/volume` 形式，传入 Rust 时映射为紧凑三元组
  `(kind: u16, index: u32, sub: u32)`，**热路径不做字符串哈希**
- **求值顺序固定**：基础值 → 自动化 → 调制器累加 → 钳制（顺序必须文档化）
- 所有参数变更经一阶低通（可配 1–50ms）避免 zipper noise
- **录制模式三种**：Touch / Latch / Write
- 点间插值支持 线性 / 保持 / 曲线（张力 -1..1）
- 求值路径零分配；自动化点加载时预排序并建索引

## S5 ⚠️ 开工前必须先扩 ABI

S0 刻意没有建 Dart 侧效果器抽象（**这是正确决定**），代价是：Dart 要「自动生成效果器 UI」，
必须能从 Rust 查询参数描述符清单——而 `docs/ABI.md` 目前只有 3 个版本函数。

所以 **S5 第一步是先设计并登记 ABI 扩展**，例如：
```c
uint32_t zenith_effect_count(void);
const ParamDesc* zenith_effect_describe(uint32_t idx);
```
按 `ABI.md` §2.2 的兼容规则递增 minor，同步更新 `docs/ABI.md` 与 Dart 侧绑定。

### 任务 S5（按 `PLAN` §3 S5）

- **Rust 侧** `native/zenith_core/src/effects/`：
  `registry` + `eq`(parametric, spectrum) + `dynamics`(compressor/limiter/gate) +
  `reverb`(algorithmic/convolution) + `delay`(sync_delay) +
  `modulation`(chorus/flanger/phaser) + `distortion`(saturation/bitcrush) +
  `filter`(multimode) + `util`(oversampling)
- 统一 trait `EffectProcessor`：`prepare` / `process` / `reset` / `latency_samples` /
  `parameters` / `set_parameter`
- 每个效果在 `prepare` 阶段一次性预分配所有缓冲（含 IR 与 FFT 暂存），
  `process` 内**零分配**；用 `assert_no_alloc` 验证
- 每个效果配 Rust 单元测试（脉冲 / 白噪声 / 正弦输入）
- 热点（EQ、卷积、饱和）用 SIMD，`cfg` 分平台，`wasm32` 用 `simd128`
- **过采样统一走 `util/oversampling.rs`**，不许各效果各写一套
- `latency_samples()` 必须准确，供 S4 的 PDC 使用
- Dart 侧：参数清单由 Rust 经 FFI 查询得到，**UI 自动生成**，无需为每个效果手写 Dart 类

## 硬性约束

- **不引入第三方 DAW 品牌名**
- **禁止 VST**（许可与 AGPL-3.0 冲突）；插件用 CLAP 或自研 ABI
- 实时路径禁止 `Vec::push` / `Box::new` / `String` / `Mutex` / `println!`
- 改 `lib.rs` / `Cargo.toml` / `ffi/` 前，先在 `docs/COORDINATION.md` 登记
- **只在自己的子目录**（`automation/`、`effects/`）内改，避免与 Agent-A / Agent-D 冲突
- 四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error
  flutter test
  ```

## 验收

- 音量自动化曲线播放正确、无 zipper noise
- Write 模式能录出自动化点
- 1000 个自动化点求值 < 2% CPU
- 所有效果在 256 帧缓冲下**零分配**
- 128 轨各挂 3 个效果，实时率 < 50%

完成后交付 `docs/stages/s2-report.md` 与 `s5-report.md`，并更新 `PLAN` §6 进度表。

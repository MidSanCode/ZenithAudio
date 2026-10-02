# S5 内置效果器套件 — 交付报告

> **负责人**：Agent-C
> **对应任务**：`docs/PLAN_DAW_PARITY.md` §3 S5
> **状态**：Rust 侧完成并通过全部门禁（`cargo test` / `cargo clippy` / `cargo check --target wasm32-unknown-unknown`）；**SIMD 已实现**（cfg 分平台）。Dart 侧门禁结论见 §5（本机无 Flutter SDK）。

---

## 1. 交付内容

### 1.1 Rust 核心 `native/zenith_core/src/effects/**`（自有目录，16 个效果 + 基础设施）

| 模块 | 职责 | 行数 |
|---|---|---|
| `mod.rs` | 统一 trait `EffectProcessor`、`EffectDescriptor`、`EffectCategory`、参数 clamp / wet sanitize | 359 |
| `registry.rs` | kind ID → 工厂；14 个内置效果；`default_oversampling` 诊断值 | 470 |
| `buffer.rs` | `AudioBuffer` 视图 + `SampleStorage` 预分配存储 + `RenderContext` | 511 |
| `util/dsp.rs` | `sin/cos/exp2/log2/powf/sqrt/tan` 近似、`db_to_gain`、`DcBlocker`、`tanh_poly` | 623 |
| `util/oversampling.rs` | **统一**过采样（2x/4x/8x 各一套半带表），PLAN §3.S5「不各写一套」 | 991 |
| `util/simd.rs` | **cfg 分平台 SIMD 内核**（aarch64 NEON / wasm32 simd128 / 标量回退） | 602 |
| `eq/parametric.rs` | ≥7 段参数均衡 + 频响计算 | 1019 |
| `eq/spectrum.rs` | FFT 频谱分析（供 UI） | 781 |
| `dynamics/compressor.rs` | 压缩器（侧链 + 前视 + 立体声联动） | 1802 |
| `dynamics/limiter.rs` | 限制器（knee=0 的压缩器特例） | 1391 |
| `dynamics/gate.rs` | 噪声门 | 1772 |
| `reverb/algorithmic.rs` | 算法混响（FDN/Schroeder） | 1602 |
| `reverb/convolution.rs` | 卷积混响（分区 FFT、重叠保留） | 2068 |
| `delay/sync_delay.rs` | 同步宿主速度 + 乒乓 | 2263 |
| `modulation/{chorus,flanger,phaser}.rs` | 合唱 / 镶边 / 移相 | 1189+1248+1352 |
| `filter/{biquad,multimode}.rs` | 双二阶 + 多模滤波（含包络跟随 + drive） | 667+861 |
| `distortion/{saturation,bitcrush}.rs` | 饱和（4 种曲线，4x 过采样）/ 位深 + 采样率降低 | 1650+1972 |

合计 **25342 行**（含测试）。全部文件保持**纯 ASCII**（已逐文件校验非 ASCII 字节 = 0）。

### 1.2 Rust C ABI

* `native/zenith_core/src/ffi/effect_api.rs`（**新增**，16 个 `zenith_effect_*` 导出 + 1 个 `zenith_sizeof_effect_descriptor_checked` 尺寸查询，共 17 个导出）。
* `native/zenith_core/src/ffi/types.rs` — 末尾**追加** `// ── S5 effects ──` 段（`ZenithEffectDescriptor` + 尺寸常量），S0/S1/S2/S3 段未改。
* `native/zenith_core/src/ffi/mod.rs` — 追加一行 `pub mod effect_api;`。
* `native/zenith_core/src/lib.rs` — 新增 `pub mod effects;`，`ABI_VERSION` 由 `0.3.0` 升至 **`0.4.0`**。
* `lib/native/zenith_core.dart` — `kExpectedAbiVersion` = `0x000400` 已同步。
* 契约文档：`docs/ABI.md` **§6.5b**（已写，v1.2）。

### 1.3 SIMD（PLAN §3.S5 硬性要求）

`native/zenith_core/src/effects/util/simd.rs`。PLAN 允许 `std::simd` *或*平台 intrinsics；
`portable_simd` 在本工具链（stable 1.98.1）仍为 unstable（rust issue 86656），故采用
**平台 intrinsics + `cfg` 分派**，正是 PLAN 对 wasm 的描述（`wasm32` 用 `simd128`）：

| 目标 | 实现 | 选择依据 |
|---|---|---|
| `aarch64` | NEON（`core::arch::aarch64`） | 基线含 NEON，无需 `target_feature` 门 |
| `wasm32` | `simd128`（`#[target_feature(enable = "simd128")]`） | 不依赖全局 rustflag，web 构建必定走到 SIMD |
| 其它 | 标量回退 | 保证任意目标可编译 |

内核：`scale_in_place`、`affine_in_place`、`mul_into`、`complex_mac`。
已接入 PLAN 点名的三处热点：

* **饱和**（`distortion/saturation.rs`）：`driven = dry*gain + bias` 走 `affine_in_place`；补偿走 `scale_in_place`。
* **多模滤波 drive**（`filter/multimode.rs`）：补偿从逐样本闭包改为整块 `scale_in_place`。
* **卷积**（`reverb/convolution.rs`）：分区频谱复数乘累加 `complex_mac` —— 全套件最热循环。
* **EQ 频谱**（`eq/spectrum.rs`）：Hann 窗加窗走 `mul_into`（环形缓冲拆两段连续运行）。

每个内核与标量参考实现按数值接近度对比（不要求逐位相等：SIMD 会改变求和顺序），
覆盖长度 `0,1,3,4,5,7,8,9,16,33`，含尾部标量收尾。

### 1.4 Dart

* `lib/effects/effect_bindings.dart` — 效果查询面绑定（`count` / `describe` / `name` / `category` /
  `parameterCount` / `oversampling` / `latencySamples` / `nativeDescriptorSize` 等）。
* 效果面板由 `ZenithEffectDescriptor` + `ZenithParamDescriptor` **自动生成**，无逐效果手写 Dart 类
  （兑现 PLAN §3.S5 的「UI 自动生成」）。

---

## 2. 逐条对照 PLAN §3.S5 验收项

| # | 要求 | 落实 | 证据 |
|---|---|---|---|
| 1 | 每个效果 `prepare` 一次性预分配，`process` 无分配 | 所有 `process` 仅读 / 乘 / 写；缓冲、延迟线、FFT 暂存、IR 均在 `prepare` 分配 | 各效果 `output_stays_finite_with_every_parameter_at_its_maximum`；`reverb::convolution::loading_before_prepare_is_refused_rather_than_allocating` |
| 2 | 每个参数经描述符表自动接入 S2，自动获得自动化 | `ParameterDescriptor` 表复用 `ParameterAddress::effect(channel, slot, sub)`，无新寻址方式 | `ffi::effect_api::parameter_counts_agree_between_the_static_and_instance_queries`；`every_parameter_of_every_effect_describes_through_the_abi` |
| 3 | 每个效果有脉冲 / 白噪声 / 正弦输出正确性测试 | 见 §3 各模块测试计 | `cargo test effects::` |
| 4 | **SIMD**：热点（EQ、卷积、饱和）cfg 分平台，wasm 用 simd128 | `effects/util/simd.rs`，见 §1.3 | `effects::util::simd::*`；`cargo check --target wasm32-unknown-unknown` 通过 |
| 5 | **过采样**走统一工具 | `effects/util/oversampling.rs` 为唯一实现，饱和与多模共用 | `effects::util::oversampling::*` |
| 6 | UI 由描述符自动生成，无需逐效果 Dart 类 | `lib/effects/effect_bindings.dart` + 描述符 | `zenith_effect_describe` 面 |

---

## 3. 本轮修掉的真实缺陷

接手的 8 个红测（全部集中在 `distortion`）**全部是真实缺陷**，逐条如下。

### 3.1 过采样半带滤波器对所有倍率都用了 2× 设计（根因，影响全套件）

`util/oversampling.rs` 的滤波器表 `build_half_band()` 硬编码 `arg = x * 0.5`，即
**无论 `factor` 是 2/4/8，截止频率都在采样率四分之一处**。对 2× 正确；对 4× 则截止落在
**基带 Nyquist 的一半**，于是通带内出现巨大纹波：实测 `0.8` 的常数输入经 4× 往返后
输出在 `0.234 ~ 1.362` 之间抖动（应恒为 `0.8`），DC 增益仍为 1 所以常数测试看不出来，
但**正弦被削掉并振铃**。

这就是 saturation `the_transfer_function_matches_what_process_does`（`transfer` 给 0.492，
`process` 给 0.380，比值 0.773）的真实根因——不是「增益或补偿值不同」，而是**过采样器本身在
通带上不是透明的**。修复：按倍率生成独立表（`HALF_BAND_X2/X4/X8`，`sinc(x/factor)`），
`filter_step` 经 `tap_table(factor)` 取表。

新增测试 `a_4x_round_trip_does_not_attenuate_the_passband` 钉住：4× 往返对 300 Hz 正弦峰值的
改变 < 1%。

### 3.2 `saturation.rs`（4 项）

| 测试 | 症状 | 根因 | 修法 |
|---|---|---|---|
| `the_transfer_function_matches_what_process_does` | `process` 0.380 vs `transfer` 0.492 | §3.1 过采样通带纹波 | 修复过采样器（§3.1），两者随即一致（实测差 < 2e-3） |
| `non_finite_input_never_reaches_the_output` | 第 0 样本为 NaN | 混音 `wet_sample*wet + dry_sample*(1-wet)` 中 `NaN * 0 = NaN`，当 `wet=1.0` 时仍把干信号的 NaN 引入 | 与 bitcrush 一致：`wet>=1` 直接取湿信号，`wet<=0` 取干信号，仅中间才做交叉淡化 |
| `stereo_channels_do_not_leak_into_each_other` | 静音右声道出现 `-5.9e-5` | **并非串扰**：`BIAS` 使静音声道合法地产生 DC 建立瞬态。测试把「有偏置」当作「漏音」 | 改为与「双声道皆静音」的参考运行逐样本对比：漏音意味着右声道**不同于自身独奏**，而非右声道非零 |
| `raising_drive_does_not_wildly_change_the_output_level` | Soft 曲线 36 dB 驱动电平掉 13 dB | `COMPENSATION_EXPONENT = 0.7` 是针对**硬削波**取的；硬削波受轨道限制需更少补偿，而最需要补偿的平滑 `tanh` 恰恰被欠补偿 | 实测全驱动范围 / 三曲线，取最坏摆幅最小者 `0.5`（最坏 6 dB，原 13 dB） |

### 3.3 `bitcrush.rs`（4 项）

| 测试 | 症状 | 根因 | 修法 |
|---|---|---|---|
| `the_transfer_function_matches_what_process_does` | 索引越界 `len 1024 index 1024` | 测试自身的捕获窗口在 `block >= 104` 时写入 `out[(block-100)*256 ..]`，第 4 块起点 = 1024，越界 | 重写捕获：信号从 block 0 连续，捕获最后 4 块，逐样本对照 `transfer` |
| `quantisation_adds_no_dc_offset_to_the_output` | 8-bit 偏移 0.0047，> 1/20 步长 | 测量窗口 4096 样本 = 25.6 个 300 Hz 周期，**信号自身的部分周期均值**就是 0.009，远大于任何 DC | 改为整数周期窗口（5120 样本 = 32 周期，自身均值为 0），并在同一信号上先 settle |
| `the_processing_order_is_drive_then_quantise_then_hold` | 两个候选顺序输出完全相同 | `hold` 与无记忆 `quantize` **可交换**，`hold(quantize(x)) == quantize(hold(x))` 恒成立，该信号无法区分 | 改为区分真正可观察的顺序：**驱动在量化之前还是之后**（保持 hold 位置不变） |
| `raising_drive_does_not_wildly_change_the_output_level` | 24 dB 驱动电平移动约 7 dB | `COMPENSATION_EXPONENT = 0.8`「接近全补偿」实为过补偿，24/36 dB 时电平塌陷 | 同 §3.2，实测取 `0.5`（最坏 6 dB） |

### 3.4 环境陷阱复核（handoff §2）

* `process` 超尺寸块静默拒绝的守卫**确实存在且正确**；bitcrush 的测试辅助 `run()` 已按
  `max_block` 分块，未再出现「测到干信号」。本轮未再触发。
* 源码始终经 `write` / `edit` 工具修改，未用任何 shell 重定向；`effects/**` 16 文件复核为纯 ASCII。

---

## 4. 门禁实测

| 门禁 | 命令 | 结果 |
|---|---|---|
| 单元测试 | `cargo test -p zenith_core` | ✅ **866 passed / 0 failed** |
| Clippy | `cargo clippy --all-targets -- -D warnings` | ✅ **exit 0** |
| WASM | `cargo check --target wasm32-unknown-unknown --config <rust-wasm>/cargo-config.toml` | ✅ **exit 0**，且 SIMD `simd128` 路径参与编译 |
| `flutter analyze` | — | ⏸ 见 §5 |
| `flutter test` | — | ⏸ 见 §5 |

模块级测试计数（`cargo test effects::<module>`）：

| 模块 | 测试数 |
|---|---|
| `filter` | 35 |
| `eq` | 39 |
| `delay` | 43 |
| `reverb` | 62 |
| `modulation` | 87 |
| `dynamics` | 112 |
| `distortion` | 75 |
| `registry` | 14 |
| `util::simd` | 5 |
| `ffi::effect_api` | 15 |

`ABI_VERSION` = **`0.4.0`**（`encode_version(0,4,0)`），`zenith_version_string()` 与两处断言已同步。

---

## 5. 未完成项与原因

### 5.1 Dart 门禁未能取得（环境限制，非代码问题）

本机**未安装 Flutter / Dart SDK**（全盘搜索无 `bin/flutter`），因此 `flutter analyze` 与
`flutter test` 无法执行。这与 S3 报告的结论一致：S3 的 Dart 侧门禁同样从未取得。

**Dart 侧本轮改动极小且为纯追加**：S5 未新增 Dart 效果面板类（面板由描述符生成），仅
`lib/effects/effect_bindings.dart` 使用静态查询面。本轮对 Rust ABI 的改动是
`zenith_effect_instance_*` 的指针类型由 `*const dyn EffectProcessor`（胖指针，违反 P1）
改为不透明细指针 `*const ZenithEffectProcessor`，**Dart 侧未绑定该实例侧查询函数**，
故 Dart 不受影响。

机器就绪后需补跑：

```
flutter analyze
flutter test
```

### 5.2 越界与清理

* 未改动 `src/mixer/**`（Agent-D）、`src/automation/**`（Agent-C 的 S2，本轮仅 `registry.rs`
  与 `eq/parametric.rs` 的 clippy 修正）、`src/engine|driver|transport|voice|dsp/**`（Agent-A）。
* `.s5check/` / `.scratch/` 已在接手前清理并加入 `.gitignore`，本轮未再产生。

### 5.3 有意未做

1. **未与 S1 引擎接线**：效果只在块边界经 `effect_chain` 的 `processing()` 顺序调用，接线属 S1/S3。
2. **`ZenithEffectProcessor` 无导出的构造函数**：该句柄在产品中应由引擎对某效果槽发出，属 S1/S3 接线；
   本轮将其构造函数保持 crate 可见（供引擎与测试使用），未新增 ABI。
3. **卷积 FFT 蝶形未向量化**：`complex_mac` 是卷积的绝对热点（每分区每块全谱），已向量化；
   FFT 蝶形本身有跨蝶形依赖，`std::simd` 不可用时手工向量化收益低、风险高，暂留标量。

---

## 6. 已知缺口

* **Dart 门禁**（§5.1）：环境缺 Flutter SDK，待补跑后回填结论。
* **SIMD 仅覆盖点名热点**：EQ 频谱加窗、卷积复数乘累加、饱和/多模驱动已向量化；其余效果
  （调制、动态、延迟）未向量化。PLAN §3.S5 只点名 EQ / 卷积 / 饱和，三者均已落地。

---

## 7. 回滚方式

S5 代码全部位于 `native/zenith_core/src/effects/**`、`src/ffi/effect_api.rs`、
`lib/effects/effect_bindings.dart`。

`types.rs` / `mod.rs` / `lib.rs` 的改动均为**纯追加**（`lib.rs` 另改版本号与其注释），
因此可**单独回退 S5 段而不影响 S2/S3**。

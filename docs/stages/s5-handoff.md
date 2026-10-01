# S5 内置效果器套件 — 交接说明

> **状态**：Rust 侧功能基本完整，**8 项测试仍为红**（全部集中在 `distortion`）。
> **交接原因**：上下文预算耗尽，且 `distortion` 两个文件是与子代理并发编辑的，
> 继续在其上编辑会产生互相覆盖。详细待办见 `todo_write` 的 15 项。
>
> **接手前必读**：本文 §1（当前状态）、§2（环境的两个坑）、§3（8 个红测的具体诊断）。

---

## 1. 当前状态

### 1.1 已绿（截至最后一次全量运行）

| 模块 | 测试 | 状态 |
|---|---|---|
| `effects::filter`（biquad + multimode） | 35 | ✅ 0 失败 |
| `effects::eq`（parametric 7 段 + spectrum） | 39 | ✅ 0 失败 |
| `effects::delay`（sync_delay、同步到宿主速度、乒乓） | 43 | ✅ 0 失败 |
| `effects::modulation`（chorus / flanger / phaser） | 87 | ✅ 0 失败 |
| `effects::dynamics`（compressor / limiter / gate） | 112 | ✅ 0 失败 |
| `effects::reverb`（algorithmic + convolution） | 62 | ✅ 0 失败 |
| `ffi::effect_api`（S5 查询面，15 个导出） | 15 | ✅ 0 失败 |
| `effects::registry`（跨效果一致性） | ~14 | ✅ 0 失败 |

全量：**849 passed / 11 failed**（其中 3 项为当时的 ABI 版本断言，已修复；现预计
849+3 passed / 8 failed，需重新跑一遍确认）。

### 1.2 仍红（全部在 `distortion`）

`bitcrush.rs` 4 项、`saturation.rs` 4 项。逐条诊断见 §3。

### 1.3 ABI 状态

* `ABI_VERSION` = **0.4.0**（`lib.rs`），`zenith_version_string()` = `"0.4.0"`，
  两处断言已同步更新并通过。
* `lib/native/zenith_core.dart` 的 `kExpectedAbiVersion` = `0x000400` 已同步。
* 新增导出见 `native/zenith_core/src/ffi/effect_api.rs`，契约文档见
  `docs/ABI.md` **§6.5b**（已写）。
* `docs/COORDINATION.md` **C-011** 已登记，状态仍为 🟡 进行中，**全绿后应改 🟢**。

---

## 2. 环境的两个坑（务必先看）

### 2.1 绝不要用 PowerShell 改源码文件

本次 S5 有一个文件被彻底损坏：`Set-Content` / `Out-File` / `>` / `-replace` 后写回，
会把 UTF-8 字节按 Windows ANSI 代码页重新编码，文件变成**非法 UTF-8**，
cargo 直接拒绝读取（`stream did not contain valid UTF-8`），且不可逆（该文件当时
未被 git 跟踪，无副本可恢复）。一个子代理的 `sync_delay.rs`（约 80 KB）因此全部重写。

**规则**：
1. 只用 `write` / `edit` 工具改源码。
2. `native/zenith_core/src/effects/**` 下的文件**保持纯 ASCII**：用 `+/-` 代替 `±`、
   `->` 代替 `→`、`<=` 代替 `≤`、`x` 代替 `×`、普通 `-` 代替长破折号、
   用 "section" 代替 `§`、避免任何 CJK。当前 16 个效果器文件均已验证非 ASCII 字节 = 0。

### 2.2 `process` 会拒绝超尺寸块（且静默）

每个效果的 `process` 都有形如 `if frames > self.max_block { return; }` 的守卫，
**直接返回、不处理、不报错**。这是 ABI P6 要求的正确行为，但它是个陷阱：
测试里把 `prepare(SR, 256, 2)` 的结果拿去跑 2048 帧的块，测到的就是**未经处理的输入**。

这正是 `saturation` 里「soft 和 hard 折叠能量完全相同（0.00016485032 vs 0.00016485032）」
的原因——两个测试都在测干信号。已修：测试的 `run()` 现在会按 `max_block` 切块。
**`bitcrush.rs` 的测试辅助函数很可能有同样的问题，先查这个。**

---

## 3. 8 个红测的具体诊断

> 行号是交接时的快照，修的过程中会漂移；用测试名搜索而不是行号。

### 3.1 `saturation.rs`（4 项）

**`the_transfer_function_matches_what_process_does`（~1593）**
最有诊断价值的一条。`x = 0.4`、6 dB 驱动、Hard 曲线下：
`transfer()` 给 `0.492`，`process()` 给 `0.380`，比值 `0.773`。

手算 `transfer()`：`0.4 × db_to_gain(6) = 0.4 × 2.0 = 0.8` → hard clip = `0.8`
→ `comp = 1 / 2^0.7 = 0.6156` → `0.8 × 0.6156 = 0.4925` ✅ 与 test 一致。

所以 `process()` 用的是另一个增益或另一个补偿值。**让两者共用同一条代码路径**
（例如把 `transfer()` 作为 `process` 的单样本内核调用），而不是让测试去迁就实现。

**`non_finite_input_never_reaches_the_output`（~878）**
第 0 个样本就是 NaN。`Character::shape()` 开头已做 `is_finite` 守卫，所以 NaN 是
**之后**引入的。检查顺序：oversampler 的 FIR 状态、`driven`/`wet_buf` 的复用、
以及混音 `wet_sample * wet + dry_sample * (1.0 - wet)` 中 `wet` 本身是否为 NaN。
要追到 NaN 的**首次出现点**，不要在末尾 clamp —— 末尾 clamp 会把真实 bug 藏起来。

**`stereo_channels_do_not_leak_into_each_other`（~1562）**
右声道静音却在第 4 个样本拾到 `-5.9e-05`。`dc[MAX_CHANNELS]` 与
`oversamplers[MAX_CHANNELS]` 都是**按声道索引**的，所以嫌疑在共享缓冲：
`scratch`、`dry`、`driven`、`wet_buf` 是否在声道循环之间未清零就复用。
注意这个量级很小，像是滤波器状态串扰而不是直接拷贝。

**`raising_drive_does_not_wildly_change_the_output_level`（~1145）**
Soft 曲线 36 dB 驱动：电平从 `-12.04` 掉到 `-25.20 dB`（掉 13 dB）。
`COMPENSATION_EXPONENT = 0.7`。在整个驱动范围内测量实际电平曲线，
再决定是指数不对、还是 mid-tread 量化/饱和本身带来的固有残余。
若是固有残余，就按实测设定容差并在注释里说明原因，不要为了让任意容差通过而调常数。

### 3.2 `bitcrush.rs`（4 项）

**先查 §2.2**：这个文件的测试辅助几乎肯定也在喂超尺寸块。

**`the_transfer_function_matches_what_process_does`（~1917）**
`process` 与 `transfer()` 不一致。很可能是与 saturation 同类的问题：
文档化的传递函数没有包含 DC blocker 或 sample-and-hold 的影响。
先确定「transfer 应该描述哪一段」，再让实现和文档一致。

**`quantisation_adds_no_dc_offset_to_the_output`（~1158）**
输出有 DC。mid-tread 量化器必须把 0 精确映射到 0（`quantize` 里已有守卫，
且 `quantisation_is_mid_tread_so_silence_in_is_silence_out` 已绿）。
所以 DC 来自别处：检查 dither 是否默认开启、以及量化与采样保持的**先后顺序**。

**`the_processing_order_is_drive_then_quantise_then_hold`（~1469）**
实测顺序与文档声明的 `drive -> bits -> rate -> DC block -> mix` 不符。
先确认 `process` 实际做了什么，再决定是改实现还是改文档 —— 文档里那一段
（模块头部第 18-38 行）有相当详细的论证，说明作者是有意选这个顺序的，
所以更可能是实现没跟上。

**`raising_drive_does_not_wildly_change_the_output_level`（~1675）**
24 dB 驱动使电平移动约 7 dB。同 §3.1 的最后一条：先测真实曲线再定容差。

---

## 4. 尚未开始的工作

### 4.1 SIMD（PLAN §3.S5 的硬性要求）

`docs/PLAN_DAW_PARITY.md` §3.S5 要求热点用 SIMD（`std::simd` 或平台 intrinsics，
`cfg` 分支，`wasm32` 用 `simd128`）。**目前完全没做**，所有热循环都是标量。

要么实现，要么在 `s5-report.md` 里**明确记为已知缺口并说明原因**。不要沉默略过。

### 4.2 四个门禁

```
cargo clippy --all-targets -- -D warnings
cargo test
flutter analyze          # 要求 0 error
flutter test
cargo check --target wasm32-unknown-unknown   # ABI P7
```

`cargo clippy` 目前会因**兄弟文件**报错（`registry.rs:91`、`eq/parametric.rs` 等）。
`flutter analyze` 在 S5 期间**两次超时 10 分钟**，需要换更窄的调用或后台跑。
另外 S3 的 Dart 侧门禁从未取得过（C-010 记为「Dart 侧待取得结论」），
所以**不要假定 `flutter analyze` / `flutter test` 的基线是干净的**，要先实测。

### 4.3 收尾文档

* `docs/stages/s5-report.md` —— 按 `docs/stages/s3-report.md` 的格式写
  （交付内容 / 逐条对照验收表 / 门禁证据 / 已知缺口）。
* `docs/PLAN_DAW_PARITY.md` §6 进度表：`| S5 效果器套件 |` 从 ⬜ 未开始 改为实际状态。
* `docs/COORDINATION.md` C-011：🟡 进行中 → 🟢 已生效。
* 删除 `docs/prompts/s5-effect-brief.md`（它自己说明 S5 落地后即可删除）。

---

## 5. S5 期间修掉的真实 bug（供参考，都已被测试钉住）

这些说明「红测往往是真的」，也是接手时不要放松相关测试的理由。

| # | 位置 | 症状 | 原因 |
|---|---|---|---|
| 1 | `filter/biquad.rs` | 所有滤波器/EQ 形状错误：1 kHz 转折点实测 -12 dB 而非 -3 dB，陷波几乎不衰减 | `omega = PI*f/fs` 少了因子 2，应为 `2*PI*f/fs`。所有 `sin`/`cos`/`alpha`/`t` 都算在半角度上 |
| 2 | `eq/spectrum.rs` | 满量程正弦读 -6.6 dB | 归一化用 `2/N`（无窗常数），Hann 窗相干增益 0.5 未计入，应为 `4/N` |
| 3 | `eq/spectrum.rs` | 平滑参数完全不起作用 | 参数单位是「百分比 0..95」，却直接当 0..1 系数用 |
| 4 | `eq/spectrum.rs` | 前几帧从 -120 缓慢淡入 | `prepare`/`reset` 用 -120 初始化，而平滑的哨兵值判据是 `<= -143`，两者不回一致 |
| 5 | `distortion/saturation.rs` | 0 dB 驱动仍有 0.008/样本误差 | DC blocker 无条件运行；~5 Hz 高通串联不是恒等，且会把「常量输入」的 DC 整个滤掉 |
| 6 | `distortion/saturation.rs` | 测试测到的是干信号 | 测试辅助喂超尺寸块，`process` 静默拒绝（见 §2.2） |
| 7 | `distortion/saturation.rs` | hard clip 不比 tanh 脏 | 测试在 18 dB 驱动下测量，`tanh` 已完全饱和，各曲线本就收敛；改到 10 dB |
| 8 | `distortion/saturation.rs` | 2 kHz 谐波测不准 | 粗扫辅助函数以 500 Hz 步进，`1990..2010` 只探一个点，读到的是基波泄漏 |

另有子代理报告的、未被独立复核的两项（delay/chorus/flanger 各一处）：
补一阶高通的「关闭」系数应为 `1.0` 而非 `0.0`（否则整条反馈通路被静音），
以及延迟抽头差一采样（`write - 1` 应为 `write`，导致 20 ms 回声落在 961 而非 960）。

---

## 6. 已登记的越界与清理

* **`.s5check/` 与 `.scratch/`**：并行子代理用过的一次性验证 crate，被一次自动快照
  误提交进 git（40 个文件）。已 `git rm --cached` 并从磁盘删除，同时把
  `.s5check/`、`.s5-check/`、`.scratch/`、`.mycheck/` 加进 `.gitignore`。
  若再见到类似目录，按同样方式处理，不要提交。
* **ABI minor 序列**：这是**所有 agent 共享的单一线性序列**，不是每阶段一个号。
  S2=0.2.0、S3=0.3.0、S5=0.4.0。后续阶段继续递增，不要重新从 0.x 起算。

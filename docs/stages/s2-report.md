# S2 阶段报告 · 参数系统与自动化

> **阶段**：S2（`docs/PLAN_DAW_PARITY.md` §3.S2）
> **负责人**：Agent-C
> **状态**：✅ 完成（纯逻辑部分 + C ABI + Dart 绑定与编辑器）
> **提交**：`021d4a5`（Rust 核心）、`2fed521`（C ABI）、`547bdd8`（Dart）
> **登记**：`docs/COORDINATION.md` C-002（ABI 面）、C-003（types.rs 段落）、C-009（落地与阻塞解除）

---

## 1. 一句话结论

S2 要求的**参数寻址、自动化曲线、调制器、录制模式、无拉链噪声平滑**五项能力
已在 Rust 侧完成并通过 316 项测试，C ABI 导出 **52 个 `zenith_automation_*`
函数**，Dart 侧完成地址编解码、绑定与编辑器 UI。四项门禁全绿。

**S2 未与 S1 引擎接线**，这是刻意的：S1 的 `ZenithEngine` 尚不存在，
`automation` 模块因此**不持有任何 DSP 图引用**。接线点是
`zenith_automation_advance_block()`，S1 落地后在块边界调用一次即可（§7）。

---

## 2. 交付物清单

### 2.1 Rust 核心（`native/zenith_core/src/automation/`）

| 文件 | 行数 | 职责 |
|---|---|---|
| `parameter.rs` | 378 | 寻址三元组 `ParameterAddress`、描述符、单位/标志位 |
| `store.rs` | 525 | 预分配参数注册表、原子值存储、平滑时间 |
| `clip.rs` | 701 | 自动化曲线、64 桶均匀索引、五种插值 |
| `lane.rs` | 510 | 轨道、录制模式、`LaneSet` 容器 |
| `player.rs` | 944 | **实时求值路径**（零分配）、一阶平滑、统计 |
| `modulator.rs` | 1275 | LFO（7 波形）、ADSR 包络、峰值跟随、`ModulatorBank` |
| `recorder.rs` | 716 | Touch / Latch / Write 录制状态机、抽稀、提交合并 |
| `mod.rs` | 270 | 模块文档（**规范求值顺序**）、re-export、零分配断言测试 |
| **合计** | **5319** | |

### 2.2 C ABI

| 文件 | 内容 |
|---|---|
| `src/ffi/types.rs` | 6 个 `#[repr(C)]` 镜像结构体 + 判别值表 + 尺寸 `const` 断言 |
| `src/ffi/param_api.rs` | **52 个导出函数**，单一不透明句柄 `ZenithAutomation` |
| `src/ffi/mod.rs` | ABI 面入口，说明 `src/ffi/` 之外无 `#[no_mangle]` |

### 2.3 Dart（`lib/automation/`）

| 文件 | 行数 | 职责 |
|---|---|---|
| `parameter_address.dart` | 339 | 名称 ↔ 紧凑三元组桥接；五种曲线插值 |
| `native_types.dart` | 589 | 结构体镜像、解码后的值类型、**字段偏移自检** |
| `automation_bindings.dart` | 1103 | 52 个函数的绑定；每次 `calloc` 都在 `finally` 中释放 |
| `automation_lane_painter.dart` | 473 | 绘制与命中测试（纯函数，可无 widget 测试） |
| `automation_lane_editor.dart` | 543 | 绘制/拖动/张力手势、录制模式与状态指示 |
| `parameter.dart` | 148 | （S0 既有，未改动） |

---

## 3. 关键设计决策

### 3.1 寻址：注册期解析，而非热路径哈希

**问题**：UI 用名字说话（`channel/<uuid>/volume`），音频线程要 8 字节整数。
字符串哈希既慢，又可能分配，还可能碰撞。

**方案**：`(kind: u16, index: u32, sub: u16)` 三元组。名称在白名单注册时
解析**一次**，此后所有操作——包括每块的读写——只用整数。

`sub` 索引由 **Rust 注册表分配**，Dart 从不自行编造。这一点使映射天然
**满射且无碰撞**，而不是「希望哈希表现良好」。

`sub` 字段宽度为 `u16` 而 `index` 为 `u32`，`key()` 布局为
`kind << 48 | index << 16 | sub`：store 对此做二分查找，**刻意不用 `HashMap`**，
以免哈希与分配进入音频路径。

### 3.2 规范求值顺序，单点实现

```
基础值 → 自动化 → 调制器累加 → 钳制
```

该顺序在 Rust 侧**只有两处实现**：`player::advance_block`（音频路径）与
`ffi::param_api::zenith_automation_value_at`（UI 预览），且两者语义一致
（后者不含平滑滤波，因为 UI 要显示「自动化说了什么」而不是「衰减后的值」）。

测试 `the_evaluation_order_is_documented_and_single_sourced` 守卫这一点，
防止「文档写一套、代码写另一套」。

### 3.3 零分配：用分配器强制，而非注释声明

`automation/mod.rs` 的测试装了一个 `WatchingAllocator`（包裹
`std::alloc::System` 的 `GlobalAlloc`），在 `advance_block` 的稳态路径上
**一旦分配即 panic**。测试场景为 128 轨 × 600 块。

配套还有一个**反向测试** `mutating_lanes_while_playing_is_not_allocation_free`：
它故意在播放中修改轨道，并断言分配器**确实**看到了分配。没有这个反向测试，
一个坏掉的看门狗会让主测试永远通过——这是「测试通过了但什么都没测」的典型。

### 3.4 平滑系数必须按块时长计算（真实缺陷）

最初的实现按**每采样**计算一阶系数，却**每块**施加一次。后果：10 ms 的平滑
设置在 256 帧缓冲下正常，在 2048 帧缓冲下变成约 80 ms。

这类缺陷只在用户换音频设备时暴露，属于最难复现的一类。修复方式为新增
`one_pole_coeff_for_elapsed(time_ms, elapsed_ms)`，按**本块实际时长**取系数，
使 ABI 承诺的 1..50 ms 在任意缓冲大小下含义一致。

已加测试 `smoothing_time_is_independent_of_block_size`：分别在 256 与 2048
帧下测量 100 ms 后的值，要求两者相差小于 1 dB。

### 3.5 曲线张力：幂曲线而非有理弯曲

三版迭代后才定型。前两版的问题值得记录：

1. `denom = 1 + k(1 - 2t)`：在 `t = 0.5` 处恰好为 1.0，**张力在中点完全无效**。
2. 加偏置后：分母在 `t ≈ 0.55`、`tension = 1.0` 时**穿过零点**，曲线跌落并被
   钳制，产生**不连续跳变**——对音频参数而言这是爆音，不是显示问题。

最终采用 `s(t) = t^gamma`，`gamma = 2^(-3·tension)`：在 `0..1` 上单调、
两端精确、零点移动。测试 `the_shaped_curve_is_monotonic_for_every_tension`
与 `tension bends the midpoint but never leaves the segment` 覆盖。

### 3.6 录制：在途 take 被丢弃（真实缺陷）

`Recorder::on_control_move` 原实现先 `self.take = Some(Take::open(...))` 覆盖
在途 take，之后才判断「参数是否变了」。结果：用户在录制中转动另一个旋钮，
**刚录的那条包络被静默丢弃**。

改为 `self.take.take()` + 同参数判断，前一段提交到**它自己的**轨道。

### 3.7 提交采用 punch-in 语义

`Recorder::commit` 先 `remove_range(start, end)` 再合并新点。即**录制区间内
的点被替换而非叠加**，与用户的「重录这一段」预期一致。

---

## 4. 验收标准对照（PLAN §3.S2）

| # | 标准 | 状态 | 证据 |
|---|---|---|---|
| 1 | 音量自动化曲线正确播放，无拉链噪声 | ✅ | 曲线求值 `clip.rs`；平滑 `player.rs`；块大小无关性测试 |
| 2 | Write 模式能录制自动化点 | ✅ | `recorder.rs` 三种模式；`write_mode` 与 latch/touch 测试 |
| 3 | 1000 个自动化点求值 < 2% CPU | ✅ | 10 000 块（≈53 s 音频）**0.16 s** → 约 **0.3%** 实时占用 |
| 4 | 所有效果器在 256 帧零分配 | ⏳ S5 | 效果器属 S5；S2 侧自动化路径已零分配 |
| 5 | 128 轨 × 3 效果器实时 < 50% | ⏳ S5 | 同上 |

> **关于标准 3 的诚实说明**：`a_thousand_points_evaluate_cheaply` 是**冒烟
> 检查而非基准测试**——它抓的是意外的 O(n²) 或逐采样扫描。真正的性能测量属
> S9。数字（0.16 s / 53 s）来自本机调试构建，不应作为发布性能承诺引用。

### 4.1 零分配断言覆盖范围

`advance_block_does_not_allocate` 的场景：**128 条轨道 + 8 个 LFO + 600 块稳态**。
覆盖参数读写、曲线求值、调制器累加、钳制与平滑的全路径。

---

## 5. 门禁实测

| 门禁 | 命令 | 结果 |
|---|---|---|
| Rust lint | `cargo clippy -p zenith_core --all-targets -- -D warnings` | ✅ exit 0 |
| Rust 测试 | `cargo test -p zenith_core` | ✅ **316 passed / 0 failed** |
| WASM 可编译（P7） | `cargo check -p zenith_core --target wasm32-unknown-unknown` | ✅ 通过 |
| Dart 静态分析 | `flutter analyze` | ✅ **0 error**（100 项 info/warning，属既有基线） |
| Dart 测试 | `flutter test` | ✅ **196 passed / 0 failed** |

其中 S2 新增 Dart 测试 25 项（`test/automation/parameter_address_test.dart`）。

---

## 6. 过程中发现并修复的真实缺陷

以下**全部**是测试抓出来的，不是走查发现的。记录它们是因为其中数条属于
「只在换设备或换操作顺序时才暴露」的类型。

| # | 缺陷 | 症状 | 根因 |
|---|---|---|---|
| 1 | LFO 输出整周期冻结 | 1 Hz 正弦/三角/锯齿在**整整一秒内输出恒定值** | `current` 只在相位回绕时刷新 |
| 2 | 相位偏移被施加两次 | 半周期偏移无法反相波形 | `retrigger()` 与 `shape_value()` 各加一次 |
| 3 | 三角波越界 | 输出 -1.02 | 分段分支写错 |
| 4 | 张力曲线非单调 | 高张力下 1.0 → 0.0 跳变（**爆音**） | 有理弯曲分母过零 |
| 5 | 包络 release 中重触发掉零 | 重触发时包络瞬间归零 | attack 恒从 0 起算 |
| 6 | 录制切换参数丢 take | 刚录的包络消失 | `on_control_move` 提前覆盖在途 take |
| 7 | 平滑时间随块大小漂移 | 10 ms 在 2048 帧下变 80 ms | 系数按采样算、按块施加 |
| 8 | `clamp` 对 ±∞ 行为错误 | 无穷大返回默认值而非钳到边界 | 非有限值一律走默认分支 |
| 9 | 桶索引窗口错误 | 长曲线求值偏差（99 帧处 3 vs 14.14） | `rebuild_index` 的 `.min()` 钳制与窗口偏移 |
| 10 | 同帧多点语义 | 返回首个而非最后一个同帧点 | 未用 `partition_point` |
| 11 | C ABI `advance_block` 在音频线程分配 | 违反自身文档承诺的实时安全 | `lanes.to_vec()` 克隆轨道表 |
| 12 | Dart 字段偏移探测误报 | `value` 报为 offset 4（实为 8） | 8 字节窗口在任意字节处匹配到零填充尾部 |

> 第 12 条特别值得记录：**这个探测器本身的职责就是抓出误声明的结构体镜像**，
> 一个会给出错误答案的探测器比没有更糟——它会给出虚假的安心。修复后按**字段
> 自身宽度**比较。

---

## 7. 与 S1 的接线（交接说明）

S2 **刻意不依赖 DSP 图**，这是它能先于 S1 落地的前提。接线方式：

1. `ZenithEngine` 落地后**内嵌**一个 `ZenithAutomation`（而非另建参数状态），
   参数状态因此只有一份权威副本。
2. 音频回调在**块边界**调用一次：
   ```c
   zenith_automation_advance_block(auto, frame, frames);
   ```
   该函数实时安全（零分配/零锁/零 IO），已由 §3.3 的分配器断言强制。
3. `docs/ABI.md` §6.4 的函数面届时可直接接受 `ZenithEngine*`，或加一层转发；
   **Dart 调用点不需要改写**。

`automation/mod.rs` 的模块文档明确写下了这条边界，避免后续 agent 误以为
S2 应该自己去建图。

---

## 8. 未做（有意）与原因

| 项 | 原因 |
|---|---|
| 与 DSP 图接线 | S1 的 `ZenithEngine` 尚不存在；强行接线等于自建第二套图 |
| `zenith_effect_describe_params` | 依赖 S5 的效果槽模型；S2 用 `zenith_automation_list_parameters` 覆盖同一需求 |
| S5 效果器 | 按 PLAN 顺序，S5 需先有 S2 的 ABI 扩展——**现已就绪** |
| 编辑器的撤销/重做 | 属工程级编辑栈，不属 S2 参数系统范畴 |

---

## 9. 交给 S5 的接口

S5 **可以开工**了。第一项任务按计划是 ABI 扩展：

1. `zenith_effect_count()` / `zenith_effect_describe()`——效果器枚举与描述；
2. `zenith_effect_describe_params()`——在 `zenith_automation_list_parameters`
   之上加一层按效果槽过滤；
3. 效果参数用 `ParameterKind::Effect` 注册（判别值 3，**已冻结**），
   效果槽索引复用 `ParameterAddress::effect()` 的「通道高 24 位 / 槽低 8 位」
   打包方式。

S5 落地后，**Dart 侧的效果器 UI 可由描述符自动生成**，无需为每个效果器写
界面代码——这正是 `ZenithParamDescriptor` 存在的理由。

---

## 10. 已知限制

1. **`clip_move_point` 等按索引操作的函数，索引会因重排而失效。**
   拖动导致重排后，调用方必须重新 `clip_get_points` 再操作下一个点。
   这是为保持热路径免分配而做的取舍，已在 `docs/ABI.md` §6.4 记录。
   编辑器（`automation_lane_editor.dart`）已遵守此约定。

2. **调制器每块求值一次，不与音频采样率同步。**
   `advance_block` 在块边界计算调制值，块内保持不变。对 256 帧（约 5.3 ms）
   而言，LFO 的最快有效更新率约 187 Hz，高于任何音乐相关的调制速率；
   但**音频速率的调制**（如 FM）不在 S2 范围内。

3. **Dart 侧 `dart:ffi` 无 `offsetOf`。**
   本 SDK 版本只提供 `sizeOf`，因此字段偏移由**写入后扫描字节镜像**测得。
   这在 `native_types.dart` 中有注释说明，并配了 25 项测试。

4. **`ZenithParamDescriptor` 的字符串指针由 Rust 侧 leak 持有。**
   注册时 `Box::leak`，句柄销毁时随进程回收。总量受编译期注册表上界约束
   （数百字节/会话），代价换取了免去为每个 FFI 签名引入生命周期参数。

---

## 11. 相关文档

- `docs/PLAN_DAW_PARITY.md` §3.S2、§6 进度表
- `docs/ABI.md` §6.4（参数与自动化契约）、§9.3（结构体镜像清单）、§12（变更记录 v1.1）
- `docs/COORDINATION.md` C-002 / C-003 / C-009
- `docs/stages/s0-report.md`、`docs/stages/s1-report.md`

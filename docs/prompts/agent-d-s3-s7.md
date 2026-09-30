# 启动提示词 · Agent-D（S3 混音器 + S7 插件宿主）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-D，负责「卓声」DAW 项目的 S3（混音器）与 S7（插件宿主）。

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§3 的 S3 与 S7 全节
2. `docs/ABI.md` — C ABI 契约
3. `lib/providers/mixer_provider.dart` — **现状需要被取代**：`trackFx` 只是
   `Map<String, List<String>>`，纯字符串列表、**不挂载任何 DSP**
4. `lib/widgets/mixer/mixer_panel.dart`

## 依赖

S3 依赖 Agent-A 的 S1（DSP 图、`AudioBuffer`、`DspNode` trait）。
开工前确认 `native/zenith_core/src/engine/` 里的图结构与缓冲类型已稳定。

若未就绪，先做不依赖的部分：混音器数据模型、路由与环检测算法、序列化/迁移逻辑、
Dart 侧混音器 UI 骨架，并配单元测试。

## 任务 S3（按 `PLAN` §3 S3）

- **Rust 侧** `native/zenith_core/src/mixer/`：`channel` / `strip` / `bus` / `send` /
  `effect_chain` / `meter` / `pan_law` / `graph`（含环检测）
- **Dart 侧**：混音器 UI（通道条、推子、旋钮、电平表），复用现有
  `lib/widgets/layout/rotary_knob.dart`
- **通道**：默认 64 插入 + 8 返回 + 1 主控；**预分配**，实时路径不扩容
- **路由**：任意通道可输出到任意总线/返回，支持分组嵌套深度 ≥ 4；
  **建图时做环检测**，成环必须被拒绝
- 每通道 **4 个 Send**，各自独立开关 + 电平 + 推子前/后
- 每通道 **10 个效果槽**，支持重排、旁通、湿/干、串联
- 任意通道可作为任意效果的**侧链源**
- 增益以 **dB** 显示（-INF..+12dB），推子实现为 dB 曲线而非线性
- 每通道 + 主控的**峰值与 RMS 双表**，3 秒峰值保持；Rust 原子写入，Dart 无锁读
- 混音器状态写入工程 `spec/project.json`；旧工程按「每轨 = 一个通道」自动迁移，
  **原音量 / 声像 / 静音 / 独奏必须无损迁移**

## 任务 S7 ⚠️ 桌面限定

- 插件 ABI 抽象层 + CLAP 宿主
- 插件参数桥接到参数存储，**自动获得自动化**
- 插件延迟桥接到 PDC
- 插件状态（预设）随工程序列化
- **沙箱化**：插件崩溃不得拖垮宿主（子进程隔离，P1）
- 移动与 Web 端**不加载外部插件**，改用 Agent-C 的 S5 内置效果
- **【禁止 VST】** 许可与项目 AGPL-3.0 冲突，只允许 CLAP 或自研 ABI

## 硬性约束

- **不引入第三方 DAW 品牌名**
- 实时路径禁止 `Vec::push` / `Box::new` / `String` / `Mutex` / `println!`
- 改 `lib.rs` / `Cargo.toml` / `ffi/` 前，先在 `docs/COORDINATION.md` 登记
- **只在自己的子目录**（`mixer/`、`plugins/`）内改
- 四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error
  flutter test
  ```

## 验收

- 建 16 条通道 + 若干发送/返回，播放无爆音、无相位问题
- 环检测正确拒绝成环连接
- 旧工程音量/声像/静音/独奏**无损迁移**
- `assert_no_alloc` 通过
- 加载一个 CLAP 插件后：参数可自动化、延迟被补偿、预设随工程保存
- 插件崩溃时宿主存活，弹出提示并可移除该插件

完成后交付 `docs/stages/s3-report.md` 与 `s7-report.md`，并更新 `PLAN` §6 进度表。

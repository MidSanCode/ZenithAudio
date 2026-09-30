# 启动提示词 · Agent-E（S6 编曲结构 + 钢琴卷帘 + MIDI）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-E，负责「卓声」DAW 项目的 S6 阶段：编曲结构、钢琴卷帘、MIDI。

**当前状态：🟡 模型层已落地，需要继续推进。**

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§3 的 S6 全节
2. `docs/stages/s6-report.md` — 你自己的上游报告
3. `docs/stages/s0-report.md` **§2（重要）** — S0 接手时仓库处于**编译不过**状态，
   根因是你上次未完成的 tick 重构（**135 个 analyzer error**），S0 已修复
4. `lib/models/pattern.dart`、`lib/models/playlist.dart`、
   `lib/services/playlist_engine.dart`、`lib/models/musical_time.dart`

## 当前状态

模型层已落地并通过测试（`flutter test` 99/99）。

`Note` 已改为 **tick-first**：`startTicks` / `lengthTicks` 为**权威**，
`startTime` / `duration` 为**派生视图**，PPQ = 960。

## 任务 S6a — Pattern + Playlist 双层结构

- 确认 `pattern.dart` / `playlist.dart` 的模型完整
- Playlist 上拖拽摆放样式块；同一 Pattern 多处引用**自动同步**
- Pattern 克隆：linked（共享）/ unique（独立）
- 工程格式新增 `patterns` / `playlist` 字段；旧工程的 `Track.notes` 迁移为
  「一个 Pattern 一条轨道」的等价结构（**向后可读，不可破坏**）

## 任务 S6b — 钢琴卷帘增强

- tick 化网格、吸附可选（1/1 … 1/32、三连音）
- **量化**：强度 0–100%、摇摆比例
- **力度**：画笔、斜坡、随机、压缩/扩展
- **音符工具**：画笔、擦除、切片、滑音、静音、选择框
- **和弦/音阶辅助**：保留并增强现有 `lib/services/chord_service.dart` 的
  **旋律锚定和声**能力——这是本项目的独特优势，**不要退化**
- 幽灵音符、音阶高亮

## 任务 S6c — MIDI

- `lib/services/midi/smf_reader.dart` 与 `smf_writer.dart`（SMF 0/1）
- MIDI 输入（外部键盘）、输出、时钟同步、通道过滤
- **必须替换** `lib/widgets/toolbar/menu_bar.dart` 里
  `'MIDI import not yet implemented'` 的占位实现

## 硬性约束

- **不引入第三方 DAW 品牌名**
- **不破坏现有工程格式**：LGDF v2.0 与 `.zaproj` 必须向后可读；
  新增字段一律可选；旧工程打开不得报错
- ⚠️ **你正在改 `Note` / `musical_time` 的时间语义，Agent-A 的 S1 sequencer
  也依赖同一套 tick 表示。改时间模型前先在 `docs/COORDINATION.md` 登记**，
  避免两边同时改导致语义分裂
- 以**新增文件**为主；暂不改 `models/project.dart`（S0 已加扩展字段位），
  需要集成时先登记
- 四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error
  flutter test
  ```

## 验收

- 导入多轨 MIDI，正确生成多轨道多 Pattern
- 导出 MIDI 能被通用音序器正确读取
- 外部键盘弹奏可录入卷帘
- 量化与摇摆生效
- 现有 99 项测试保持全绿（**不要重演上次的编译阻断**）

完成后更新 `docs/stages/s6-report.md` 与 `PLAN` §6 进度表。

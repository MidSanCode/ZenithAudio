# 启动提示词 · Agent-F（S8 音频编辑 + S9 收尾）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-F，负责「卓声」DAW 项目的 S8（音频编辑）与 S9（收尾）。

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§3 的 S8 与 S9 全节
2. `lib/models/audio_clip.dart` — 现有 `AudioClip` / `Selection` 模型
3. `lib/widgets/editor/audio_clip_editor.dart`、`lib/widgets/editor/effect_dialog.dart`

**当前实测状态（2026-10-04）**
- S2/S3(Rust)/S5(大部) 已完成；S1.1 引擎主体未开工（Agent-A 负责中）
- S8 的实时接线依赖 S1.1 与 S3；但**核心算法是纯函数**，现在就能做
- ABI_VERSION = 0.4.0

## 依赖与开工建议

**先做算法层（现在就能做，零依赖）**：

时间拉伸 / 切片 / 瞬态检测都是纯函数（`Float32` 进、`Float32` 出），
在 Rust `native/zenith_core/src/` 下新增独立模块（如 `src/stretch/`、`src/analysis/`），
配足单元测试。**注意 `src/effects/` 等已有目录不是你的所有权，不要动。**

**后做接线层（等 S1.1/S3 落地）**：非破坏性片段播放、交叉淡化与混音器集成。

## 任务 S8（按 `PLAN` §3 S8）

- **算法（Rust，先做）**：
  - 时间拉伸/变调：WSOLA 或相位声码器，独立于宿主速度
  - 瞬态检测 → 音频切片 → 映射到卷帘音符
  - 音频量化：瞬态对齐网格
  - 过采样统一走 `effects/util/oversampling.rs` 已有的工具，不要重写
- **接线（等依赖）**：
  - 非破坏性编辑：片段引用源文件 + 偏移 + 增益包络 + 淡入淡出，**不改源**
  - 交叉淡化：任意两片段重叠处自动/手动淡化曲线
  - 波形编辑器（P2）：破坏性编辑，独立窗口
- ⚠️ **Web 端拉伸属重型渲染**，受 S1.5 降级策略管辖（L1 降级时不可用）——
  实现时预留开关接口供 Agent-B 的降级状态机调用

## 任务 S9 — 收尾（最后做）

- A/B 对比：两份工程状态快速切换
- 性能：128 轨 + 多效果压力测试，目标实时率 < 50%
- 文档：用户手册、快捷键表、架构文档、**Rust 核心开发指南**
- 迁移工具：旧 `.zap` / `.zaproj` 一键迁移向导
- 移除旧依赖：确认无引用后彻底下线 `media_kit`（S4 验证通过后才允许）
- 构建固化：确认 `hook/build.dart` 在六平台 CI 稳定产出原生库
  （CI 各 job 补 `rustup target add`）

## 硬性约束

- **不引入第三方 DAW 品牌名**（**含用户手册等所有文档**）
- **不破坏现有工程格式**
- **只用 write/edit 工具改源码，绝不用 PowerShell 写文件**（UTF-8 会毁）
- Rust 新模块保持纯 ASCII
- 改 `src/lib.rs` / `Cargo.toml` / `src/ffi/` 前先在 `docs/COORDINATION.md` 登记
- 四项全绿：`cargo clippy --all-targets -- -D warnings`、`cargo test`、
  `flutter analyze`（0 error，后台跑）、`flutter test`

## 验收

- 拉伸 ±50% 无明显金属音；切片后可直接用卷帘重排节奏
- 算法模块有完整单元测试且 `assert_no_alloc` 通过
- 六平台 CI 全部产出原生库

完成后交付 `docs/stages/s8-report.md` 与 `s9-report.md`，更新 `PLAN` §6 进度表。

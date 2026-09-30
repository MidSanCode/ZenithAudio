# 启动提示词 · Agent-F（S8 音频编辑 + S9 收尾）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-F，负责「卓声」DAW 项目的 S8（音频编辑）与 S9（收尾）。

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§3 的 S8 与 S9 全节
2. `lib/models/audio_clip.dart` — 现有 `AudioClip` / `Selection` 模型
3. `lib/widgets/editor/audio_clip_editor.dart`、`lib/widgets/editor/effect_dialog.dart`

## 依赖与开工建议

S8 依赖 Agent-D 的 S3（混音器）与 Agent-E 的 S6（编曲结构）。

但**拉伸 / 切片算法是独立的纯函数**（`Float32` 数组进、`Float32` 数组出），
可以**脱离引擎先实现并配单元测试**。建议先做这部分。波形编辑器 UI 需等 S3 / S6 稳定。

## 任务 S8（按 `PLAN` §3 S8，算法在 Rust 中实现）

- **非破坏性编辑**：片段引用源文件 + 偏移 + 增益包络 + 淡入淡出，**不改源**
- **实时时间拉伸 / 变调**：WSOLA 或相位声码器，独立于宿主速度
- **交叉淡化**：任意两片段重叠处自动/手动淡化曲线
- **音频切片**：瞬态检测 → 切片 → 映射到卷帘音符
- **音频量化**：瞬态对齐网格
- **波形编辑器**（P2）：破坏性编辑，独立窗口
- ⚠️ **Web 端拉伸属于重型渲染**，受 S1.5 降级策略管辖（L1 降级时不可用）

## 任务 S9 — 收尾

- **A/B 对比**：两份工程状态快速切换
- **性能**：128 轨 + 多效果压力测试，目标实时率 < 50%
- **文档**：用户手册、快捷键表、架构文档、**Rust 核心开发指南**
- **迁移工具**：旧 `.zap` / `.zaproj` 一键迁移向导
- **移除旧依赖**：确认无引用后彻底下线 `media_kit`
- **构建固化**：确认 `hook/build.dart` 在六平台 CI 上稳定产出原生库
  （CI 各 job 需补 `rustup target add` 步骤）

## 硬性约束

- **不引入第三方 DAW 品牌名**（**含用户手册等所有文档**）
- **不破坏现有工程格式**
- 改 `lib.rs` / `Cargo.toml` / `ffi/` 前，先在 `docs/COORDINATION.md` 登记
- 四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error
  flutter test
  ```

## 验收

- 拉伸 ±50% 无明显金属音
- 切片后可直接用卷帘重排节奏
- 六平台 CI 全部产出原生库

完成后交付 `docs/stages/s8-report.md` 与 `s9-report.md`，并更新 `PLAN` §6 进度表。

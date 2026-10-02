# 会话启动提示词索引

本目录存放各 agent 会话的**自包含启动提示词**。每个文件可直接整段复制给对应会话，
会话无需读完整份计划即可开工。

**主计划**：`docs/PLAN_DAW_PARITY.md`
**C ABI 契约**：`docs/ABI.md`
**S5 接手任务书**：`docs/stages/s5-handoff.md`

> 状态快照：2026-10-04（提示词按此快照编写，开工前请对照 §6 进度表复核）

---

## 当前项目状态（已核实）

| 阶段 | 状态 | 备注 |
|---|---|---|
| S0 基础重构 | ✅ | tag `s0-baseline` |
| S1.0 前置项 A/B/C | ✅ | panic 修复 + 适配层 + 34 调用点迁移，122/122 |
| S1.1 引擎主体 | ⬜ | `src/engine/`、`src/driver/` 不存在——**Agent-A 的任务** |
| S1.5 Web 接入 | ⬜ | 第一段可先行（不依赖 S1.1） |
| S2 参数/自动化 | ✅ | 316 Rust tests，门禁全绿，ABI 0.2.0 |
| S3 混音器 | 🟡 | Rust 侧完成（149 tests，ABI 0.3.0）；**Dart 侧门禁未取得** |
| S4 渲染/导出/PDC | ⬜ | 等 S1.1 |
| S5 效果器套件 | 🟡 | 852/8 红；**8 个 distortion 红测 + clippy 8 项 + SIMD 未决** |
| S6 编曲/卷帘/MIDI | 🟡 | 模型层落地；卷帘增强与 MIDI 未做 |
| S7 插件宿主 | ⬜ | 等 S1.1（CLAP FFI 等独立部分可先行） |
| S8 音频编辑 | ⬜ | **算法层零依赖可先行** |
| S9 收尾 | ⬜ | 最后 |

ABI 版本线性序列：S2 = 0.2.0 → S3 = 0.3.0 → S5 = 0.4.0 →（下一个新增）0.5.0

---

## 文件清单与开工建议

| 会话 | 文件 | 任务 | 可否立即开工 |
|---|---|---|---|
| **Agent-A** | [`agent-a-s1-s4.md`](agent-a-s1-s4.md) | S1.1 引擎主体 + S4 | ✅ **最优先**（所有人的图依赖它） |
| **Agent-C** | [`agent-c-s2-s5.md`](agent-c-s2-s5.md) | S5 收尾（8 红测/clippy/SIMD） | ✅ **可立即**（与 S1.1 零冲突） |
| **Agent-D** | [`agent-d-s3-s7.md`](agent-d-s3-s7.md) | S3 Dart 门禁 + S7 | ✅ S3 收尾可立即；S7 等 S1.1 |
| **Agent-E** | [`agent-e-s6.md`](agent-e-s6.md) | S6 卷帘/MIDI | ✅ 可立即（模型层已就绪） |
| **Agent-B** | [`agent-b-s1.5-web.md`](agent-b-s1.5-web.md) | S1.5 Web | 🟡 第一段可立即；第二段等 S1.1 |
| **Agent-F** | [`agent-f-s8-s9.md`](agent-f-s8-s9.md) | S8 算法 + S9 | 🟡 算法层可立即；接线等 S1.1/S3 |

---

## 启动顺序建议

**不要一次放 6 个会话。** 建议：

| 批次 | 启动 | 理由 |
|---|---|---|
| **第 1 批** | **Agent-A（S1.1）+ Agent-C（S5 收尾）** | 两者目录完全隔离（engine//driver/ vs effects/）；A 是关键路径，C 的 8 个红测不修任何人都不该引用 effects |
| **第 2 批** | Agent-E（S6）+ Agent-D（S3 Dart 收尾） | E 只动卷帘/MIDI 新文件；D 的 Dart 门禁是全项目第一次拿到可靠结论，越早越好 |
| **第 3 批** | Agent-B（S1.5 第一段）+ Agent-F（S8 算法层） | 都是零依赖先行部分 |
| **第 4 批** | 各会话的第二段（S7、worklet、S8 接线） | 等 S1.1 落地 |

**每会话开工前必须确认其提示词里写的前置检查项。**

---

## 所有会话的通用纪律（提示词中已内嵌）

1. 先读 `docs/PLAN_DAW_PARITY.md` §0.2；涉及跨语言加读 `docs/ABI.md`
2. 提交前四项全绿：
   ```
   cargo clippy --all-targets -- -D warnings
   cargo test
   flutter analyze      # 0 error（本机慢，后台跑）
   flutter test
   ```
3. **不许**出现任何第三方 DAW 品牌名（代码/注释/文档/UI）
4. 改 `src/lib.rs` / `Cargo.toml` / `src/ffi/` 前先在 `docs/COORDINATION.md` 登记
5. ABI minor 是**全项目单一线性序列**：下一个新增导出用 0.5.0
6. **只用 write/edit 工具改源码，绝不用 PowerShell 写文件**
   （历史上毁过一个 80KB 文件的 UTF-8，不可逆）
7. `effects/**` 等既有 Rust 文件保持纯 ASCII
8. 完成后写 `docs/stages/sN-report.md` 并更新主计划 §6 进度表
9. **每小步保持仓库可编译可测试**（S0 曾接手过一个 135 error 的烂摊子）

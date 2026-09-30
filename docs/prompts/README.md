# 会话启动提示词索引

本目录存放各 agent 会话的**自包含启动提示词**。每个文件可直接整段复制给对应会话，
会话无需读完整份计划即可开工。

**主计划**：`docs/PLAN_DAW_PARITY.md`
**C ABI 契约**：`docs/ABI.md`

---

## 文件清单

| 会话 | 文件 | 负责阶段 | 当前可开工？ |
|---|---|---|---|
| **Agent-A** | [`agent-a-s1-s4.md`](agent-a-s1-s4.md) | S1 Rust 音频核心 + S4 离线渲染 | ✅ **第一批**（先只做 S1.0 前置项） |
| **Agent-B** | [`agent-b-s1.5-web.md`](agent-b-s1.5-web.md) | S1.5 Web 接入与降级 | 🟡 第三批（或先做不依赖驱动的部分） |
| **Agent-C** | [`agent-c-s2-s5.md`](agent-c-s2-s5.md) | S2 参数/自动化 + S5 效果器 | 🟡 第二批可做 S2 纯逻辑部分 |
| **Agent-D** | [`agent-d-s3-s7.md`](agent-d-s3-s7.md) | S3 混音器 + S7 插件宿主 | ⛔ 第四批（需 S1 图结构稳定） |
| **Agent-E** | [`agent-e-s6.md`](agent-e-s6.md) | S6 编曲/卷帘/MIDI | ✅ **第二批**（模型层已落地，继续推进） |
| **Agent-F** | [`agent-f-s8-s9.md`](agent-f-s8-s9.md) | S8 音频编辑 + S9 收尾 | 🟡 第五批（算法部分可先做） |

**Agent-0（S0）已完成**，无需提示词。

---

## 启动顺序（重要）

**不要一次放 6 个会话。** 建议分批：

| 批次 | 启动 | 理由 |
|---|---|---|
| **第 1 批** | **Agent-A（先只做 S1.0 前置项）** | 前置项 A/B 是所有人的地基；B 的适配层让后续换引擎可回滚。**做完先汇报，不要直接冲 S1.1** |
| **第 2 批** | Agent-E（S6）+ Agent-C（S2 纯逻辑部分） | 二者都主要新增自有目录，与 S1 冲突面小，可与第 1 批并行 |
| **第 3 批** | Agent-A（S1.1 主体）、Agent-B（不依赖驱动的部分） | S1.0 通过后放行 |
| **第 4 批** | Agent-C（S5）、Agent-D（S3/S7） | 需 S1 的图结构稳定；S5 还需先扩 ABI |
| **第 5 批** | Agent-F（S8/S9） | 需 S3 与 S6 稳定 |

每个会话开工前**必须先确认上游依赖已落地**——各提示词里已写明各自的前置检查项。

---

## 当前阻断项（开工前必须解决）

| # | 等级 | 阻断项 | 归属 |
|---|---|---|---|
| 1 | 🔴 | `Cargo.toml` 的 `panic = "abort"` 与 `lib.rs` 的 `catch_unwind` 承诺矛盾 | Agent-A 第一个任务 |
| 2 | 🔴 | `AudioEngine` 接口零实现者、34 个调用点仍绑在 `AudioService` | Agent-A 第二个任务（前置项 B） |
| 3 | 🟡 | S5 开工前需先扩 ABI（参数描述符查询） | Agent-C 第一个任务 |
| 4 | 🟡 | `docs/ABI.md` 顶部"s0 前 native/ 不存在"的说明已过时 | 已在检查中更新 |

---

## 所有会话的通用纪律（提示词中已内嵌）

1. 先读 `docs/PLAN_DAW_PARITY.md` §0.2（硬性约束）；涉及跨语言时加读 `docs/ABI.md`
2. 提交前四项全绿：
   ```
   cargo clippy --all-targets -- -D warnings
   cargo test
   flutter analyze      # 0 error
   flutter test
   ```
3. **不许**在代码 / 注释 / 文档 / UI 文案中出现任何第三方 DAW 品牌名
4. 改 `lib.rs` / `Cargo.toml` / `ffi/` 三个共享文件前，先在 `docs/COORDINATION.md` 登记
5. 完成后写 `docs/stages/sN-report.md` 并更新主计划 §6 进度表
6. Rust 侧**只在自己的子目录内改**（`automation/`、`effects/`、`mixer/`、`driver/` 等）

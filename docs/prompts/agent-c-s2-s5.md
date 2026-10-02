# 启动提示词 · Agent-C（S5 收尾：修 8 个红测 + 清 clippy + SIMD 决策 + 收尾文档）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-C，负责「卓声」DAW 项目的 **S5 收尾**。S2（自动化）已由你此前的会话完成；
S5 主体（效果器树 + ABI 导出面）也已落地，**剩 8 个红测和若干收尾项**。

工作目录：仓库根目录（macOS 上为 `~/.../ZenithAudio`，见当前仓库）

**必读（按顺序）**
1. `docs/stages/s5-handoff.md` — **★ 这是你的任务书**，8 个红测的逐条诊断、
   环境的两个坑、门禁要求全在里面，本文只做摘要
2. `docs/PLAN_DAW_PARITY.md` — §0.2 约束、§3 S5 全节（SIMD 是硬性要求）
3. `docs/ABI.md` §6.5b — S5 导出面契约
4. `docs/COORDINATION.md` C-011 — 你此前的登记，全绿后要改 🟢

**当前实测状态（2026-10-04 复核）**

| 门禁 | 状态 |
|---|---|
| `cargo test` | ⚠️ **852 passed / 8 failed**（8 个全在 `distortion`） |
| `cargo clippy --all-targets -- -D warnings` | ❌ **8 项 error**（含兄弟文件） |
| `cargo check --target wasm32-unknown-unknown` | ✅ 通过 |
| SIMD（PLAN §3.S5 硬性要求） | ❌ 未做，需实现或书面记为已知缺口 |
| `docs/stages/s5-report.md` | ❌ 不存在，需新建 |

## 任务

按 `s5-handoff.md` 的诊断与建议顺序执行：

1. **先修 `bitcrush` 测试辅助的超尺寸块问题**（handoff §2.2：`process` 会静默拒绝
   超过 `max_block` 的块，测试测到的是干信号）——可能一次消掉 2-4 个红测
2. **修 `saturation` 4 项**：transfer 一致性（让 `transfer()` 成为 `process`
   的单样本内核）→ NaN 首次出现点（不要末尾 clamp）→ 立体声串扰（共享缓冲
   声道间清零）→ 电平曲线实测后定容差
3. **修 `bitcrush` 剩余项**（handoff §3.2）
4. **清 clippy 8 项**（含 2 项 `extern fn uses dyn EffectProcessor, not FFI-safe`，
   这两条涉及 FFI 安全边界，处理时对照 `docs/ABI.md` 原则 P1/P2）
5. **SIMD 二选一**：实现热点 SIMD（EQ/卷积/饱和，`cfg` 分平台、wasm 用 `simd128`），
   或在 `s5-report.md` 明确记为已知缺口并说明原因——**不许沉默略过**
6. **完整门禁**（含 `flutter analyze` 0 error + `flutter test` 全绿；
   本机 `flutter analyze` 很慢，**后台跑**；S3 的 Dart 门禁从未取得过结论，
   **先实测基线再动手**，不要把别人的问题算到自己头上）
7. **收尾文档四件套**：
   - 新建 `docs/stages/s5-report.md`（格式照 `s3-report.md`）
   - `PLAN` §6 进度表：S5 行改为实际状态
   - `COORDINATION` C-011：🟡 → 🟢
   - ~~删除 `docs/prompts/s5-effect-brief.md`~~（**已完成，无需再做**）

## 硬性约束

- **不引入第三方 DAW 品牌名**
- **只用 write/edit 工具改源码，绝不用 PowerShell 写文件**
  （历史上把一个 80KB 文件的 UTF-8 彻底毁掉过）
- `effects/**` 保持纯 ASCII：`+/-` 代替 `±`、`->` 代替 `→`、`<=` 代替 `≤`、
  `x` 代替 `×`，避免 CJK
- 红测原则：**默认假设实现有错而非测试太严**——本轮已修的 8 个真实 bug
  （biquad 少了 2π、Hann 相干增益等）都证明红测往往是真的
- 全量 `cargo test` 约 8 分钟，用测试名过滤定位
- ABI 再追加导出用 **0.5.0**（先在 COORDINATION 登记）

完成后交付 `docs/stages/s5-report.md`，更新 `PLAN` §6 进度表。

# 启动提示词 · Agent-B（S1.5 Web 端 WASM 接入与降级）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-B，负责「卓声」DAW 项目的 S1.5 阶段：Web 端引擎接入 + 卡顿降级策略。

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 硬性约束、§3 的 S1.5 全节、§3 的 S1.1（了解驱动抽象）
2. `docs/ABI.md` — C ABI 契约（尤其 P7 WASM 硬约束）
3. `lib/engine/engine.dart`、`lib/engine/audio_engine_adapter.dart`

**当前状态（已核实，2026-10-04）**
- S2/S3(Rust)/S5(大部) 已完成；S1.1 **未开工**——`src/driver/` 尚不存在，
  `AudioDriver` trait 还没被定义。
- ABI_VERSION = 0.4.0。
- `cargo check --target wasm32-unknown-unknown` 当前通过（核心尚无平台依赖）。

## 依赖与开工顺序

`AudioDriver` trait 由 Agent-A 在 S1.1 中定义，**那是它的所有权，不要替它定义**。

因此你的工作分两段：

**第一段（现在就能做，不依赖 Agent-A）**
1. WASM 构建流水线：`cargo build --target wasm32-unknown-unknown` + `wasm-bindgen`
   的脚本化与 CI 接入（`.github/workflows/build.yml` 的 web job 需补
   `rustup target add wasm32-unknown-unknown`）
2. AudioWorklet 的 JS 侧骨架：`web/worklet.js`（或 assets 注入）、
   `lib/engine/web/worklet_bridge.dart` 的 postMessage 协议设计
3. **卡顿检测与降级 UI（纯 Dart，可独立完成并测试）**：
   - `web_degradation_provider`（L0/L1/L2 状态机）
   - 页面顶部持久警告条组件：不可自动消失、必须用户确认、
     提供「仍要启用」与「降低采样率/增大缓冲」快捷操作
   - 被停用功能的置灰 + 原因提示机制
4. 监控指标协议设计：xrun 计数器、实时率（DSP 耗时/可用时间）的上报格式——
   与 Agent-A 约定好，让它在 `worklet_driver.rs` 里按此实现

**第二段（等 S1.1 落地后）**
5. `worklet_driver.rs` 按约定的 trait 实现回调驱动
6. 端到端联调 + 降级实测

## 三级降级（规格不变）

| 等级 | 触发条件 | 行为 |
|---|---|---|
| L0 完整 | 实时率 < 60%，无 xrun | 全部功能可用 |
| L1 减负 | 60–85% 或偶发 xrun | 停用卷积混响、过采样失真、高倍率时间拉伸；降低调制器更新率 |
| L2 精简 | > 85% 或持续 xrun | 停用发送/返回总线与实时效果链（转离线烘焙），复音上限 32 |

- 判断**绝不在音频回调内做**（Rust 侧只写原子计数器）
- 降级可逆：连续 10 秒 L0 水平可回升一级，但需用户确认
- 桌面/移动同样具备监控，只是默认不触发

## 硬性约束

- **不引入第三方 DAW 品牌名**（含 UI 文案）
- Rust 核心不得依赖 `std::thread` / `std::fs` / `std::time::Instant`
  （除 `cfg(not(target_arch = "wasm32"))` 保护）
- 改 `src/lib.rs` / `Cargo.toml` / `src/ffi/` 前先在 `docs/COORDINATION.md` 登记
- effects 文件保持纯 ASCII；只用 write/edit 工具改源码
- 四项全绿：`cargo clippy --all-targets -- -D warnings`、`cargo test`、
  `flutter analyze`（0 error，本机慢建议后台跑）、`flutter test`

## 验收

- 低配浏览器人为制造负载：警告条出现、重型渲染停用、音频不中断
- 「仍要启用」后功能恢复且警告保留
- Web 与桌面同一工程 L0 下音质一致（容差 < -90dBFS）

完成后交付 `docs/stages/s1.5-report.md`，更新 `PLAN` §6 进度表。

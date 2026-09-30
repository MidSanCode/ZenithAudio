# 启动提示词 · Agent-B（S1.5 Web 端 WASM 接入与降级）

把下面整段复制给该会话作为第一条消息。

---

你是 Agent-B，负责「卓声」DAW 项目的 S1.5 阶段：Web 端引擎接入 + 卡顿降级策略。

工作目录：`F:\exeliang\zenith_audio`

**必读**
1. `docs/PLAN_DAW_PARITY.md` — §0.2 硬性约束、§3 的 S1.5 全节、§3 的 S1（了解驱动抽象）
2. `docs/ABI.md` — C ABI 契约
3. `lib/engine/engine.dart`、`lib/services/audio_service_web.dart`

## 依赖与前置

你依赖 Agent-A 的 S1 完成 `AudioDriver` trait 抽象。开工前先确认
`native/zenith_core/src/driver/mod.rs` 里的 `AudioDriver` trait 已存在。

若 S1 尚未落地，你可以先做**不依赖它**的部分：
- WASM 构建流水线（`cargo build --target wasm32-unknown-unknown` + `wasm-bindgen`）
- AudioWorklet 的 JS 侧骨架
- 卡顿检测的 UI 警告条组件（纯 Dart，可独立完成）

但**不要自己改 `AudioDriver` trait**——那是 Agent-A 的所有权。

## 任务

按 `PLAN` §3 S1.5：

1. **Web 复用同一份 Rust 核心**，编译为 WASM，经 AudioWorklet 驱动。
   （`cpal` 在 `wasm32` 上不可用，这正是 Web 必须走独立驱动的原因。）

2. **三级自动降级**
   | 等级 | 触发条件 | 行为 |
   |---|---|---|
   | L0 完整 | 实时率 < 60%，无 xrun | 全部功能可用 |
   | L1 减负 | 实时率 60–85% 或偶发 xrun | 停用卷积混响、过采样失真、高倍率时间拉伸；降低调制器更新率 |
   | L2 精简 | 实时率 > 85% 或持续 xrun | 停用所有发送/返回总线与实时效果链（转离线烘焙），复音上限降至 32 |

3. **检测逻辑**：Rust 侧只写 xrun 计数器与实时率原子量，**判断绝不在音频回调内做**；
   Dart 侧每秒轮询一次（Web 经 Worklet `postMessage` 回传）。

4. **UI**：触发阈值时在页面顶部显示**持久警告条**。要求：
   - 警告**不可自动消失**，必须用户确认
   - 提供「仍要启用」按钮（强制覆盖但保留警告）
   - 提供「降低采样率 / 增大缓冲」快捷操作
   - 被停用的功能在所有 UI 面板上**置灰并附原因提示**
   - 状态经 `web_degradation_provider` 暴露，供所有面板订阅

5. **降级可逆**：性能恢复（连续 10 秒 L0 水平）后可回升一级，但需用户确认。

6. 桌面/移动端**同样具备这套监控**，只是默认不触发降级。

## 硬性约束

- **不引入第三方 DAW 品牌名**（含 UI 文案）
- Rust 核心不得依赖 `std::thread` / `std::fs` / `std::time::Instant`
  （除 `cfg(not(target_arch = "wasm32"))` 保护的部分）
- 改 `driver/worklet_driver.rs` 与 `lib.rs` 前，先在 `docs/COORDINATION.md` 登记
- 四项全绿：
  ```
  cargo clippy --all-targets -- -D warnings
  cargo test
  flutter analyze      # 0 error
  flutter test
  ```

## 验收

- 低配浏览器上人为制造负载，警告条正确出现、重型渲染被停用、**音频不中断**
- 点「仍要启用」后功能恢复且警告保留
- Web 与桌面播放同一工程，在 L0 下音质一致（容差 < -90dBFS）

完成后交付 `docs/stages/s1.5-report.md` 并更新 `PLAN` §6 进度表。

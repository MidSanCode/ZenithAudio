# 卓声 · ZENITH AUDIO

一个作曲 / 编曲工具。界面是 Flutter（Dart），实时音频运算在 Rust，二者经 C ABI
（`dart:ffi`）通信。

> **代号**：Project ZENITH-RT — 把「离线渲染式作曲工具」重构为「实时音频引擎的
> 数字音频工作站」。总计划见 [`docs/PLAN_DAW_PARITY.md`](docs/PLAN_DAW_PARITY.md)。

---

## 它是什么

- **Dart 侧**负责界面、工程模型、文件与云同步；
- **Rust 侧**（`native/zenith_core/`）负责 DSP、调度、混音、效果与离线渲染；
- 一份 Rust 源码覆盖桌面 / 移动 / Web（WASM）。

```
Flutter / Dart  ──  widgets/ 界面 · providers/ 状态 · models/ 工程模型
                    services/ 文件·SMF·WAV·迁移 · engine/ 抽象与 FFI 绑定
                                    │  dart:ffi（C ABI，docs/ABI.md）
Rust core        ──  engine/ 实时引擎 · mixer/ 混音台 · effects/ 内置效果套件
                    automation/ 参数自动化 · transport/ 调度 · edit/ 音频编辑
                    dsp/ 原语 · driver/ 驱动 · ffi/ 唯一导出面
```

细节见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)（全局架构）与
[`docs/RUST_CORE.md`](docs/RUST_CORE.md)（Rust 开发指南）。

---

## 能力概览

| 领域 | 已实现 |
|---|---|
| **实时引擎** | Rust 实时核心、tick 级采样精确调度、内部混音总线、无锁参数/事件通道、无锁状态快照 |
| **混音器** | 64 插入 + 8 返回 + 1 主控，任意路由（建图期环路拒绝）、4 发送、每通道 10 效果槽、峰值+RMS 电平 |
| **效果器** | 内置：参数 EQ、频谱、压缩/限制/门、算法/卷积混响、同步延迟、合唱/镶边/移相、饱和/位粉碎、多模滤波；过采样与分平台 SIMD |
| **自动化** | 参数统一寻址、片段编辑（线性/保持/曲线）、Touch/Latch/Write 录制、LFO/包络调制器 |
| **离线渲染** | 复用同一 DSP 图的离线渲染、效果延迟补偿（PDC）、WAV 导出（16/24/32-bit） |
| **音频编辑** | WSOLA 时间拉伸、变调、瞬态检测、交叉淡化；非破坏性片段模型 |
| **编曲 / 卷帘 / MIDI** | Pattern+Playlist 双层结构、编排视图、量化/摇摆/力度工具、音阶高亮/幽灵音符、SMF 0/1 导入导出 |
| **工程格式** | LGDF v2.0（`.zaproj`），旧的 `.lgdf` / `.zap` 向后可读并就地迁移 |
| **工程体验** | A/B 对比、旧工程迁移向导、无限撤销、云同步 |
| **稳定性** | 音频卡顿自动降级（三级）+ 持久警告条；128 轨压力实测 24.2% 实时率 |

**快捷键**见 [`docs/SHORTCUTS.md`](docs/SHORTCUTS.md)，**用户手册**见
[`docs/USER_MANUAL.md`](docs/USER_MANUAL.md)。

---

## 构建与运行

### 依赖

- **Flutter**（stable）与 Dart SDK
- **Rust** 工具链（`cargo`、`rustc`）
- 桌面构建还需各平台原生工具链（如 macOS/iOS 的 CocoaPods、Linux 的 GTK 开发包）

### 桌面

```bash
flutter pub get
flutter run            # 或 flutter build macos|windows|linux
```

`hook/build.dart`（native-assets）会在 `flutter build` 时自动 `cargo build` 出
Rust 核心并链接。

### Web

```bash
flutter build web --release      # 产出 build/web/
```

> **注意**：Web 端目前**可构建，但实时播放尚未接通**——还缺 `AudioWorklet` 驱动与
> WASM（`wasm-bindgen`）流水线。见下方「已知限制」。

### 直接构建 Rust 核心

```bash
cargo build -p zenith_core
cargo test  -p zenith_core
```

---

## 验证（合并前门禁）

```bash
cargo clippy -p zenith_core --all-targets -- -D warnings   # 零 warning
cargo test   -p zenith_core                                # Rust 全绿
cargo check  -p zenith_core --target wasm32-unknown-unknown # Web 端硬约束（P7）
flutter analyze                                            # 零 error
flutter test                                               # 全绿
flutter build web --release                                # Web 可构建
```

CI 见 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)（拦截回归）与
`build.yml`（六平台出产物）。另有 128 轨压力测试：

```bash
cargo test -p zenith_core --release --test performance -- --ignored --nocapture
```

**当前基线**：Rust 1018 项测试通过；Dart 378 项通过（唯一失败为 Windows 专属
`registry_quoting_test.dart`，在非 Windows 上不适用）。

---

## 目录结构

```
lib/                     Dart 应用
├── models/              工程 / 轨道 / 音符 / Pattern / Playlist / 片段
├── providers/           状态（项目、播放、混音、设置、降级、A/B）
├── services/            文件、LGDF 序列化、SMF、WAV、迁移、云同步
├── widgets/             界面
├── engine/              AudioEngine 抽象与 FFI 绑定
└── automation|mixer|effects|plugins/   与 Rust 对应的 Dart 侧

native/zenith_core/      Rust 核心
├── src/ffi/             唯一 extern "C" 导出面
├── src/engine|driver|transport|voice|dsp|mixer|effects|automation|edit/
└── tests/performance.rs 128 轨压力测试

docs/                    计划、ABI 契约、架构、用户手册、各阶段报告
```

---

## 已知限制

这些是**如实记录**的未完成项，不掩饰：

1. **无内置实时设备驱动**：`cpal` 是可选 feature，默认构建不含。无驱动时
   `zenith_engine_start` 返回 `UNSUPPORTED`（不假装成功）。因此当前应用仍走旧的
   `media_kit` 播放路径。
2. **Web 实时播放未接通**：Web 可构建（`flutter build web` 通过），但实时音频还需
   `AudioWorklet` 驱动 + WASM 流水线。
3. **插件宿主**：槽位状态、序列化、搜索路径已完成；**CLAP FFI 加载器**未做（需
   CLAP SDK，桌面限定）。不使用 VST（许可与 AGPL-3.0 冲突）。
4. **音频编辑 UI**：算法、FFI、非破坏性片段模型已完成；波形编辑器 UI 未做。
5. **编曲/卷帘**：编排视图、量化/摇摆/力度工具、音阶高亮/幽灵音符已完成；力度画笔
   绘制接入、MIDI 外部键盘输入未做。
6. **移除 `media_kit`**：待引擎真正接线（见第 1 条）。

各阶段的详细报告见 `docs/stages/`。

---

## 许可

AGPL-3.0-or-later，见 [`LICENSE`](LICENSE)。

> 本项目**禁止**在代码、注释、文档或 UI 文案中出现任何第三方 DAW 品牌名；CI 有一
> 条 grep 守卫拦截。参照功能一律用中性术语描述。

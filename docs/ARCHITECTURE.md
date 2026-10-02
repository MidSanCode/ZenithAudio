# 卓声 · 架构文档

> 面向新接手者的全局视图。细节契约见 `docs/ABI.md`（跨语言）与
> `docs/PLAN_DAW_PARITY.md`（阶段计划）。

---

## 1. 一句话

卓声是一个 Flutter（Dart）应用，实时音频运算在 **Rust** 里，二者经 **C ABI**
（`dart:ffi`）通信。Dart 负责界面、工程模型、文件与云同步；Rust 负责 DSP、调度、
混音、效果与离线渲染。

```
┌──────────────────────────── Flutter / Dart ────────────────────────────┐
│  widgets/ 界面     providers/ 状态     models/ 工程模型                  │
│  services/ 文件、序列化、SMF、WAV、迁移                                 │
│  engine/engine.dart        抽象接口 AudioEngine                        │
│  engine/audio_engine_adapter.dart   委托实现（当前走旧 media_kit 路径）  │
│  engine/ffi/*               FFI 绑定 + FfiAudioEngine                   │
└───────────────────────────────┬────────────────────────────────────────┘
                                │ dart:ffi  (C ABI, docs/ABI.md)
┌───────────────────────────────▼────────────────────────────────────────┐
│                       Rust core (native/zenith_core)                    │
│  ffi/      唯一 extern "C" 面      engine/   实时引擎 + 效果机架 + PDC   │
│  driver/   AudioDriver / 离线         transport/ 播放头 + 序列器         │
│  mixer/    控制台 + 路由 + 电平        automation/ 参数 + 自动化          │
│  effects/  16 个内置效果               voice/ 合成/采样声部 + 池          │
│  dsp/      biquad/SVF/FFT/重采样       edit/   拉伸/变调/瞬态/交叉淡化    │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 2. 为什么是 Rust

离线 bounce（把 MIDI 渲染成 WAV 再交给播放器）无法支撑实时参数自动化、实时效果链与
零延迟监听。计划因此把运算核心整体下沉到 Rust：无 GC、可 SIMD、可编译到 WASM，六平台
共享**同一份**核心源码。

Rust 核心必须**不依赖** `std::thread` / `std::fs` / `std::time::Instant`（除非
`cfg(not(target_arch = "wasm32"))` 保护），否则 Web 端会失去。这条由
`cargo check --target wasm32-unknown-unknown` 每次 CI 强制。

---

## 3. 实时安全（最重要的一条规则）

**音频回调路径禁止**：堆分配、`Mutex`、引用计数增减、IO、打印、panic。

- 所有缓冲在 `prepare` / 构造时一次性分配；
- 参数写入走原子量，事件走无锁 SPSC 队列；
- 状态回传走**无锁快照轮询**，不做回调（回调会引入 GC 与调度不确定性）；
- 边界 `extern "C"` 一律经 `guard()`（`catch_unwind`），panic 转错误码。

见 `docs/RUST_CORE.md` 的具体检查清单。

---

## 4. 目录职责速查

| 目录 | 负责 | 不负责 |
|---|---|---|
| `lib/models/` | 工程/轨道/音符/Pattern/Playlist 的纯数据 + JSON | 无 UI、无 IO |
| `lib/providers/` | Riverpod 状态：项目、播放、混音、设置、降级、A/B | 不直接碰 FFI |
| `lib/services/` | 文件、LGDF 序列化、SMF、WAV、迁移、云同步 | 不持有 UI 状态 |
| `lib/widgets/` | 界面 | 不含 DSP |
| `lib/engine/ffi/` | 唯一了解 C ABI 形状的 Dart 代码 | 不含业务逻辑 |
| `native/.../ffi/` | 唯一 `#[no_mangle]` 面 | 不含 DSP 算法本体 |
| `native/.../engine|mixer|effects|automation|voice|dsp|edit/` | 运算 | 不碰文件系统 |

---

## 5. 时间与寻址

- **时间**：音乐位置用 **tick**（`PPQ = 960`）权威存储；秒是派生视图。实时传输用**帧**。
  tick↔帧换算只在一处实现（Rust `transport`），Dart 侧不得自行插值。
- **参数寻址**：`ParameterAddress` 紧凑三元组 `(kind, index, sub)`，热路径不做字符串
  哈希。Dart 侧字符串键 ↔ Rust 侧的映射在 `lib/automation/parameter_address.dart`。
- **效果槽**：`effect(channel, slot, sub)`，slot 折进 `sub` 高 8 位。

---

## 6. 跨语言契约（不可违反）

完整清单见 `docs/ABI.md` §1 的 P1–P10。要点：

1. 所有导出在 `src/ffi/`，签名只用 C 兼容类型；
2. Dart 只持不透明指针，绝不解析 Rust 内部内存；
3. 谁分配谁释放（Rust 分配、Rust 提供 `*_free`）；
4. 每个导出 `catch_unwind` 保护；
5. `#[repr(C)]` 结构体两侧字段顺序/宽度严格一致，并有 `size_of` 双向断言；
6. `ABI_VERSION` 运行时校验，不匹配立即报错。

`ABI_VERSION` 的 minor 是**全项目单一线性序列**（S2=0.2, S3=0.3, S5=0.4, S1=0.5,
S4=0.6），每次纯新增导出/追加字段才 +1。

---

## 7. 工程文件

- 当前格式 **LGDF v2.0**，容器扩展名 `.zaproj`；
- 旧 `.lgdf` 与 `.zap` **向后可读**，读取时在内存中迁移；源文件不动；
- 检测按**内容**而非扩展名（`lib/services/project_migration.dart`）。

---

## 8. 构建

- Rust：`crate-type = ["staticlib", "cdylib", "rlib"]`，由 `hook/build.dart`
  （native-assets）在 `flutter build` 时驱动 `cargo build`；
- 默认构建**不含设备驱动**（`cpal` 为可选 feature）；无驱动时
  `zenith_engine_start` 返回 `UNSUPPORTED`，不假装成功；
- WASM：`cargo build --target wasm32-unknown-unknown`。

---

## 9. 验证

四道门禁（CI 见 `.github/workflows/ci.yml`）：

```
cargo clippy -p zenith_core --all-targets --locked -- -D warnings
cargo test -p zenith_core --locked
cargo check -p zenith_core --locked --target wasm32-unknown-unknown
flutter analyze   &&   flutter test
```

外加一条 grep 守卫：禁止出现第三方 DAW 品牌名（`docs/PLAN_DAW_PARITY.md` §0.2）。

# 卓声 · Rust 核心开发指南

> `native/zenith_core/` 是实时音频核心。本文件面向要改它的开发者，重点是那些
> **弄错就会出 UB 或爆音**的规则。跨语言契约见 `docs/ABI.md`。

---

## 1. 布局

```
native/zenith_core/src/
├── lib.rs          ABI 版本、Status、guard()、模块声明
├── ffi/            唯一的 #[no_mangle] extern "C" 面
│   ├── types.rs        #[repr(C)] 结构体（分 S0/S2/S3/S5 段落）
│   ├── engine_api.rs   引擎与传输
│   ├── render_api.rs   离线渲染与 PDC
│   ├── param_api.rs    参数与自动化
│   ├── mixer_api.rs    混音器
│   └── effect_api.rs   效果查询面
├── engine/         实时引擎、DSP 图、效果机架、PDC、离线渲染
├── driver/         AudioDriver trait、OfflineDriver、cpal 骨架
├── transport/      播放头、序列器、无锁事件队列
├── mixer/          控制台、通道条、发送、效果槽、电平、路由
├── automation/     参数存储、自动化片段、调制器、录制
├── effects/        16 个内置效果 + 注册表 + 过采样 + SIMD
├── voice/          合成/采样声部 + 固定池
├── dsp/            biquad / SVF / FFT / 重采样
└── edit/           拉伸 / 变调 / 瞬态 / 交叉淡化（离线）
```

**所有权**：`ffi/types.rs`、`lib.rs`、`Cargo.toml`、`ffi/**` 是跨模块共享文件；
改前先在 `docs/COORDINATION.md` 登记。各运算子目录归各自 agent。

---

## 2. 实时安全清单（逐条都是硬要求）

音频回调路径（`process` / `render_block` / `advance_block`）**绝对禁止**：

| 禁止 | 原因 |
|---|---|
| `Vec::push` / `Box::new` / `String` | 分配 → 可能 GC/系统调用 → 爆音 |
| `Mutex` / `RwLock` / 阻塞 | 加锁 → 优先级反转 → 掉帧 |
| `println!` / 日志 / 文件 | IO → 不可预测的延迟 |
| `panic!` / `unwrap` / `expect` / 越界 | 跨 FFI unwind 是 UB |
| `std::thread::spawn` | 实时线程不得创建线程 |

替代做法：

- 缓冲在 `prepare` 一次性分配；
- 控制→音频：原子量（参数）或无锁 SPSC 队列（事件）；
- 音频→UI：原子快照，UI 轮询（`EngineSnapshot`）；
- 结构变更（加通道/改路由）**非实时安全**，只在控制线程调用。

编译期兜底：`#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]`。
运行期兜底：`automation` 模块的 watching-allocator 测试在稳态求值时若有分配即失败。

---

## 3. FFI 边界规则

1. **唯一导出面**：所有 `#[no_mangle] extern "C"` 只在 `src/ffi/`。
2. **不透明指针**：Dart 只持 `*mut ZenithEngine` 等，绝不解析内部布局。
3. **panic 防火墙**：每个入口经 `guard()`（`lib.rs`），panic 转 `Status::Panicked`。
   这就是 `[profile.release]` **不能**设 `panic = "abort"` 的原因——abort 下
   `catch_unwind` 捕获不到任何东西。
4. **错误码而非异常**：可失败函数返回 `Status`，出参用 `*mut T` 写出；**不返回 null 表示失败**。
5. **谁分配谁释放**：Rust 分配的内存由 Rust 提供 `*_free`（如 `zenith_buffer_free`）。
6. **结构体镜像**：`#[repr(C)]` 字段从大到小排列消除 padding；每个结构体有
   `size_of` 的 Rust 断言 + `zenith_sizeof_*` 导出，Dart 侧运行时比对。

---

## 4. 加一个导出函数的步骤

1. 在 `src/ffi/` 对应文件写 `#[no_mangle] pub extern "C" fn zenith_xxx(...) -> i32`，
   函数体包在 `guard(|| { ... })` 里；
2. 需要跨语言的结构体加到 `src/ffi/types.rs` 的对应段落（纯追加，不动别人的段）；
3. 加 `zenith_sizeof_xxx()` 并在 Rust 测试里断言硬编码字节数；
4. `ABI_VERSION` minor +1（**全项目单一线性序列**）；
5. 同步 `lib/native/zenith_core.dart` 的 `kExpectedAbiVersion`；
6. 更新 `docs/ABI.md`；
7. 在 `docs/COORDINATION.md` 登记。

---

## 5. 加一个内置效果

1. 在 `src/effects/<类目>/` 建文件，实现 `EffectProcessor`：
   - `prepare` 一次性分配所有缓冲（延迟线、IR、FFT 暂存）；
   - `process` 零分配；
   - `latency_samples` 准确（供 PDC）；
   - `parameters()` 返回该实例的参数表（地址含 channel/slot）；
2. 在 `registry.rs` 注册 kind（内置区 `0x0000_0000..=0x0000_FFFF`）；
3. 加脉冲/白噪声/正弦测试；
4. **不用改 Dart**——UI 由描述符自动生成。这是本项目的一个核心收益。

---

## 6. 效果处理器契约

```rust
pub trait EffectProcessor: Send {
    fn descriptor(&self) -> &'static EffectDescriptor;
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext);
    fn reset(&mut self);
    fn latency_samples(&self) -> usize;
    fn parameters(&self) -> &[ParameterDescriptor];
    fn set_parameter(&mut self, sub: u16, value: f32);
    fn get_parameter(&self, sub: u16) -> Option<f32>;
    fn set_bypassed(&mut self, bypassed: bool);
    fn set_wet(&mut self, wet: f32);
    fn tail_seconds(&self) -> f32;
}
```

`process` 对**超尺寸块静默拒绝**（`frames > max_block` 直接返回），测试喂块必须按
`max_block` 分块，否则测到的是干信号。

---

## 7. 跨平台

核心必须能编到 `wasm32-unknown-unknown`：

- 不用 `std::thread` / `std::fs` / `std::time::Instant`（除非 `cfg(not(target_arch = "wasm32"))` 保护）；
- 数学函数用 `effects::util::dsp` 里的近似（`sin_poly`/`cos_poly`/`exp2`/`sqrt`），
  不要用平台 `libm`；
- 音频回调来源是**可注入的驱动 trait**：桌面/移动 `cpal`，Web `AudioWorklet`，离线
  `OfflineDriver`。核心本身不知道谁在推它。

提交前必跑：`cargo check --target wasm32-unknown-unknown`。

---

## 8. 提交前四门禁

```
cargo clippy -p zenith_core --all-targets --locked -- -D warnings
cargo test   -p zenith_core --locked
cargo check  -p zenith_core --locked --target wasm32-unknown-unknown
# 若改了 Dart：
flutter analyze && flutter test
```

CI 见 `.github/workflows/ci.yml`（与 `build.yml` 分离：前者拦回归，后者出产物）。

# 卓声 · C ABI 契约设计（`docs/ABI.md`）

> **代号**：Project ZENITH-RT
> **地位**：本文件是 Dart 侧与 Rust 核心（`native/zenith_core/`）之间**唯一的跨语言契约**。
> 属于 `docs/PLAN_DAW_PARITY.md` §4.2 第 1、2 条所称的「接口契约」，
> 任何 agent 变更 ABI 必须同步改本文件，并按 §4.2 第 1 条先登记 `docs/COORDINATION.md`。
> **文档版本**：v1.0（对应 S0 契约冻结）；v1.0.1 起补充 S0 实际落地状态
>
> **当前实现状态（S0 已完成，2026-09-30 更新）**：
> `native/zenith_core/`、`hook/build.dart`、`lib/engine/`、`lib/automation/`、
> `lib/plugins/` **均已存在**并进入版本控制。S0 已导出并验证的符号为：
> `zenith_version()` / `zenith_version_match(u32)` / `zenith_version_string()`，
> 共 3 个，`ABI_VERSION = 0x000100`（0.1.0）。
> 端到端已验证：`flutter build windows --debug` 会链接 Rust 静态库并把
> `zenith_core.dll` 落在 exe 同级目录。
>
> 因此：本文件中标注 **[S0 落地]** 的条目**大部分仍是目标契约而非已实现事实**
> ——S0 只交付了版本握手这一最小链路。DSP 图、sequencer、mixer、effects
> 的 ABI 均待 S1–S5 逐步落地，落地时必须同步更新本文件。
>
> ⚠️ 已知不一致（S1 必须修复）：`Cargo.toml` 的 `[profile.release]` 当前写着
> `panic = "abort"`，与本文原则 **P4**（panic 不得跨 FFI 边界，须 `catch_unwind`
> 包裹）**直接矛盾**——`abort` 下 `catch_unwind` 永远无法捕获。见
> `docs/PLAN_DAW_PARITY.md` §3 S1.0 前置项 A。
>
> ⚠️ 本文件描述的是**我们自己**的 ABI。禁止在本仓库引入任何第三方 DAW 品牌名（§0.2）。

---

## 1. 设计原则（不可协商）

| # | 原则 | 说明 | 违反后果 |
|---|---|---|---|
| P1 | **单一 `extern "C"` 面** | 所有跨语言入口在 `src/ffi/` 内，签名一律 `extern "C" fn`，参数只用 C 兼容类型 | 无法保证 six-platform 一致 |
| P2 | **不透明指针** | Dart 只持有 `*mut ZenithEngine` 等不透明 handle，**绝不**解析 Rust 结构体内部内存 | 内存越界 UB |
| P3 | **Dart 绝不分配 Rust 内存、也绝不 free Rust 内存** | 所有缓冲由 Rust 侧分配与释放，提供配套 `*_free` 函数 | 分配器不匹配崩溃 |
| P4 | **panic 不得跨 FFI 边界** | 每个导出函数用 `catch_unwind` 包裹，panic → 错误码 | UB（跨 FFI unwind 是未定义行为） |
| P5 | **实时路径零分配、零锁、零 IO** | 音频回调内禁止 `Vec::push`/`Box::new`/`String`/`Mutex`/`println!` | 爆音（PLAN §0.3、§4.2-7） |
| P6 | **块大小不恒定假设** | 核心不得假设固定块大小；每次 `render` 以传入 `n_frames` 为准 | 部分驱动下崩溃 |
| P7 | **WASM 可编译是硬约束** | 核心不得直接依赖 `std::thread`/`std::fs`/`std::time::Instant`（除 `cfg` 保护） | 放弃 Web 端（PLAN §0.3） |
| P8 | **结构体镜像单向权威** | `#[repr(C)]` 结构体为权威，Dart 侧手工镜像；两侧字段顺序、类型宽度必须严格一致 | 镜像漂移 → 静默内存误读 |
| P9 | **ABI 版本可运行时校验** | 每次加载必须校验 ABI 版本，不匹配立即报错，不做「尽力而为」 | 版本错配下的静默崩溃 |
| P10 | **错误码而非异常** | 所有可失败函数返回状态码，出参通过指针写出；**不返回 null 表示失败** | Dart 侧无法区分错误与空值 |

---

## 2. 版本与兼容性策略

### 2.1 版本号构成

Rust 侧导出单一版本查询函数（**[S0 落地]**，即 PLAN §3.S0 要求的「最小可跑通 FFI 链路」）：

```c
/* 返回 ABI 版本，编码为 (major << 16) | (minor << 8) | patch */
uint32_t zenith_abi_version(void);

/* 返回编译期字符串，形如 "0.1.0+rustc1.96.0"，静态生命周期，无需释放 */
const char* zenith_abi_version_string(void);
```

### 2.2 兼容规则

| 变更类型 | 是否破坏 ABI | 处理 |
|---|---|---|
| 新增导出函数 | 否（向后兼容） | minor +1 |
| 新增结构体字段（**追加在末尾**，且 Dart 侧已按 `size_of` 校验） | 否 | minor +1，两侧同步 |
| 修改既有字段类型/顺序、删除字段 | **是** | major +1，必须提供迁移或拒绝加载 |
| 修改函数签名 | **是** | major +1 |
| 修改枚举已有成员的值 | **是** | major +1（枚举值一经发布不可变） |
| 新增枚举成员 | 否 | minor +1，Dart 侧 `default` 分支必须能处理未知值 |

### 2.3 运行时校验（强制）

Dart 侧 `library_loader.dart` 加载后**第一步**必须核对：

1. `zenith_abi_version() >> 16 == EXPECTED_MAJOR`，否则**抛出并拒绝启动引擎**；
2. 对每个 `#[repr(C)]` 结构体，比对 Rust 侧导出的 `zenith_sizeof_<T>()` 与 Dart `sizeOf<T>()`，不一致 → 抛出（PLAN §4.2 第 2 条的 `size_of` 双向断言）。

**[S0 落地]** 每个 `#[repr(C)]` 结构体必须配套导出 `size_t zenith_sizeof_<type>(void)`。

---

## 3. 类型系统

### 3.1 基础标量映射

| Rust | C ABI | Dart（`dart:ffi`） | 备注 |
|---|---|---|---|
| `f32` | `float` | `Float` | **音频样本统一 `f32`**（PLAN §0.3） |
| `f64` | `double` | `Double` | 仅用于时间/速度等非音频量 |
| `u8/i8` | `uint8_t/int8_t` | `Uint8/Int8` | MIDI 字节 |
| `u16/i16` | `uint16_t/int16_t` | `Uint16/Int16` | tick 低 16 位片段等 |
| `u32/i32` | `uint32_t/int32_t` | `Uint32/Int32` | 计数、ID 索引 |
| `u64/i64` | `uint64_t/int64_t` | `Uint64/Int64` | tick 绝对位置 |
| `bool` | **`uint8_t`（0/1）** | `Uint8` | ⚠️ 不用 C `bool`，避免 `_Bool` 宽度差异 |
| `usize` | `size_t` | `Size`（`int`） | 长度、偏移 |
| `()` 失败/成功 | `ZenithStatus` | `Int32` | 见 §4 |
| `&mut T`（不透明） | `*mut T` | `Pointer<T>` | |

> `usize` 在 wasm32 上是 32 位、在桌面是 64 位：**任何跨 FFI 的长度/偏移都必须用 `usize`**，
> 且 Dart 侧用 `Size`（随平台宽度）而非 `Uint64`。

### 3.2 枚举

一律以 `int32_t` 传输，`#[repr(C)]` + 显式赋值。**每个枚举必须显式写出所有判别值**：

```c
typedef enum ZenithStatusCode {
  ZENITH_OK                    = 0,
  ZENITH_ERR_INVALID_ARG       = 1,
  ZENITH_ERR_NULL_POINTER      = 2,
  ZENITH_ERR_ABI_MISMATCH      = 3,
  ZENITH_ERR_ALREADY_EXISTS    = 4,
  ZENITH_ERR_NOT_FOUND         = 5,
  ZENITH_ERR_OUT_OF_RANGE      = 6,
  ZENITH_ERR_WOULD_CYCLE       = 7,   /* DSP 图成环 */
  ZENITH_ERR_CAPACITY          = 8,   /* 预分配池已满，实时路径不扩容 */
  ZENITH_ERR_NOT_PREPARED      = 9,
  ZENITH_ERR_DEVICE            = 10,  /* 音频设备失败 */
  ZENITH_ERR_IO                = 11,
  ZENITH_ERR_UNSUPPORTED       = 12,  /* 该平台/该降级等级不支持 */
  ZENITH_ERR_PANICKED          = 13,  /* Rust 侧 panic 被 catch_unwind 捕获 */
  ZENITH_ERR_BUSY              = 14,
  ZENITH_ERR_INTERNAL          = 15,
} ZenithStatusCode;
```

**规则**：`ZENITH_ERR_PANICKED` 专门对应 P4——panic 被捕获后返回此码，且**该引擎实例进入不可继续使用的
毒化状态**，Dart 侧应销毁并重建，不得继续调用（避免半初始化状态下的 UB）。

### 3.3 字符串与字节串（所有权铁律）

| 方向 | 约定 |
|---|---|
| **Dart → Rust** | Dart 传 `const char*`（**UTF-8，NUL 结尾**）+ 显式 `usize len`。**Rust 不持有、不释放**，仅在调用期内读取 |
| **Rust → Dart（借用）** | 返回指向 Rust 内部静态/实例内存的 `const char*`，**Dart 只读，不得释放**。仅在 handle 存活期内有效 |
| **Rust → Dart（移交）** | 返回 Rust 分配的缓冲 + 配套 `zenith_string_free(char*)`，Dart 用完必须调用，否则泄漏 |

**规则**：禁止让 Rust 保存 Dart 传来的指针跨越调用边界（Dart 的 `Pointer` 在 GC/`finalizer` 后可能失效）。
所有需要持久化的字符串（轨道名、文件路径、插件名）必须在 Rust 侧**拷贝到自有存储**。

### 3.4 结构体镜像规范（P8 落地）

每个 `#[repr(C)]` 结构体：

1. 字段从大到小排列（`u64/f64` → `u32/f32` → `u16` → `u8`），**消除隐式 padding**，两侧都不得插手工 padding；
2. 在 `src/ffi/types.rs` 与 `lib/engine/ffi/native_types.dart` 中各有一份，**字段顺序逐字对应**；
3. 每份都有注释指明「对应另一侧的文件与字段名」；
4. **[S0 落地]** Rust 侧 `#[test]` 断言 `size_of::<T>()` 等于硬编码期望值；Dart 侧测试断言同一数值。

### 3.5 音乐时间类型（对齐 PLAN §3.S0 第 2 条）

tick 化后，**所有音乐时间在 ABI 上一律用 tick**（`int64`），不使用秒，避免 BPM 变化下的精度漂移：

```c
typedef struct ZenithMusicalTime {
  int64_t  ticks;        /* 绝对位置，PPQ = 960（PLAN §3.S0） */
  uint32_t ppq;          /* 冗余携带，便于 Rust 侧独立校验 */
  uint32_t _reserved;    /* 显式占位，保持 8 字节对齐 */
} ZenithMusicalTime;
```

**换算责任**：`ticks ↔ seconds` 的换算**由 Rust 侧权威实现**（依赖 `Project.bpm`），
Dart 侧仅在 UI 显示时使用自身换算。两侧换算公式必须一致：

```
seconds = ticks * 60.0 / (bpm * ppq)
ticks   = round(seconds * bpm * ppq / 60.0)
```

> 迁移期兼容：旧工程 `Note.startTime`（秒）按当前 BPM 转 tick，
> 写出时同时保留 `startTime` 影子字段一个版本周期（PLAN §3.S0 第 2 条）。

---

## 4. 错误处理契约

### 4.1 统一形态

```c
ZenithStatusCode zenith_engine_play(ZenithEngine* engine);
```

- **返回值**：`ZenithStatusCode`，`ZENITH_OK == 0`；
- **出参**：需要返回数据时通过 `*out_xxx` 指针写出，且**要求调用方传入非空指针**（否则返回 `ZENITH_ERR_NULL_POINTER`）；
- **不变量**：函数返回非 `OK` 时，**出参内容未被修改**（要么不写，要么写前已保存）——便于 Dart 侧安全忽略部分失败。

### 4.2 panic 隔离模板（P4 落地，强制）

```rust
#[no_mangle]
pub extern "C" fn zenith_engine_play(engine: *mut ZenithEngine) -> ZenithStatusCode {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let engine = unsafe { as_ref_mut(engine)? };
        engine.play()
    })) {
        Ok(status) => status,
        Err(_)    => ZenithStatusCode::err_panicked(),
    }
}
```

**禁止**：任何 `extern "C"` 函数体内出现 `unwrap()`/`expect()`/数组索引越界/`panic!`。
用 `#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]`（PLAN §4.2-7）在编译期兜底。

### 4.3 错误信息查询（可选但推荐）

```c
/* 将最近一次错误的人类可读描述写入 buf（UTF-8），返回实际写入字节数（不含 NUL）。
   buf 不足则截断并返回所需长度。线程安全：按线程存储。 */
size_t zenith_last_error(char* buf, size_t buf_len);
```

---

## 5. 对象模型与所有权

### 5.1 Handle 一览

| Handle | 创建 | 销毁 | 线程约束 |
|---|---|---|---|
| `ZenithEngine*` | `zenith_engine_create` | `zenith_engine_destroy` | 控制线程独占；音频线程由内部驱动进入 |
| `ZenithEngineConfig` | **栈上值类型**，非指针 | — | — |
| 节点/通道/效果 | 引擎内部对象，**以 `u32` 索引寻址，不导出指针** | `*_remove` | 控制线程 |

> **为什么内部对象用 `u32` 索引而不是指针**：预分配池（PLAN §3.S3 第 1 条）在重建/压缩时会
> 使指针失效，而索引可稳定寻址，且能天然表达「句柄失效」。Rust 侧必须校验索引有效性，
> 失效索引返回 `ZENITH_ERR_NOT_FOUND` 而不是 UB。

### 5.2 引擎创建（**[S0 落地]** 最小、**[S1+]** 扩展）

```c
typedef struct ZenithEngineConfig {
  uint32_t sample_rate;       /* 目标采样率，如 48000 */
  uint32_t block_size;        /* 默认 256（PLAN §3.S1 第 4 条），允许 64..2048 */
  uint32_t max_channels;      /* 预分配混音通道数，默认 64（PLAN §3.S3） */
  uint32_t max_tracks;        /* 预分配轨道数，目标 128 */
  uint32_t driver_kind;       /* 见 ZenithDriverKind */
  uint32_t flags;             /* 位标志，见下 */
  uint32_t abi_major;         /* 调用方期望的 major，用于校验 */
  uint32_t _reserved;
} ZenithEngineConfig;

/* out_engine 在成功时写入新引擎指针 */
ZenithStatusCode zenith_engine_create(const ZenithEngineConfig* config,
                                      ZenithEngine** out_engine);

/* 幂等：NULL 安全；返回后指针立即失效，不得再使用 */
void zenith_engine_destroy(ZenithEngine* engine);
```

**`driver_kind`**：

```c
typedef enum ZenithDriverKind {
  ZENITH_DRIVER_AUTO     = 0,  /* 平台最佳：cpal 或 worklet */
  ZENITH_DRIVER_CPAL     = 1,  /* 桌面/移动；wasm32 上返回 ZENITH_ERR_UNSUPPORTED */
  ZENITH_DRIVER_WORKLET  = 2,  /* 仅 wasm32；由 JS AudioWorklet 主动驱动 */
  ZENITH_DRIVER_OFFLINE  = 3,  /* 离线渲染，不使用设备（PLAN §3.S4 第 5 条） */
} ZenithDriverKind;
```

> **驱动抽象是 Web 的关键**（PLAN §3.S1）：`AudioDriver` trait 让「谁的时钟在推进 DSP 图」
> 成为可注入依赖。桌面是 `cpal` 推，Web 是 `AudioWorkletProcessor` 拉。

**`flags` 位定义**（一经发布不可复用位）：

| 位 | 名称 | 含义 |
|---|---|---|
| `0x01` | `ZENITH_FLAG_STRICT_REALTIME` | 实时安全断言开启（仅 debug/测试构建有效） |
| `0x02` | `ZENITH_FLAG_WEB_DEGRADE` | 允许 Web 降级策略介入（PLAN §3.S1.5） |
| `0x04` | `ZENITH_FLAG_OFFLINE_FAST` | 离线渲染走大缓冲高速路径 |

### 5.3 生命周期规则

1. **创建/销毁串行**：同一引擎的 `create`/`destroy` 必须在同一线程串行调用；
2. **销毁前静默**：`destroy` 内部必须先停止传输、停止驱动、join 所有非实时线程，再释放内存；
3. **销毁后不可用**：指针立即失效，Dart 侧 `EngineHandle`（RAII）负责保证不重复销毁、不悬垂调用；
4. **不可重入**：任何导出函数都不得回调 Dart（无回调 ABI）；状态回传一律走**轮询快照**。

> **为什么没有回调**：实时线程回调 Dart（`NativeCallable`）会引入 GC 与调度不确定性，
> 直接违反 P5。因此状态（播放位置、电平、xrun、降级等级）统一由 Rust 写入**原子快照**，
> Dart 侧按固定频率（如每秒 60 次 / Web 端每秒 1 次）无锁读取。

---

## 6. 函数面清单

> **[S0 落地]** 只要求实现 §6.1 的版本三件套（`zenith_abi_version`、
> `zenith_abi_version_string`、`zenith_sizeof_*`）+ `zenith_engine_create/destroy` 骨架（可返回
> `ZENITH_ERR_UNSUPPORTED`）。其余为 **[S1+]** 契约，分期实现但**签名先冻结**。

### 6.1 版本与自检（**[S0 落地]**）

```c
uint32_t    zenith_abi_version(void);
const char* zenith_abi_version_string(void);
size_t      zenith_sizeof_engine_config(void);
size_t      zenith_sizeof_engine(void);      /* 不透明，仅调试用，可为 0 */
```

### 6.2 引擎与传输（**[S1]**）

```c
ZenithStatusCode zenith_engine_create(const ZenithEngineConfig*, ZenithEngine**);
void             zenith_engine_destroy(ZenithEngine*);

ZenithStatusCode zenith_engine_start(ZenithEngine*);   /* 启动音频设备/驱动 */
ZenithStatusCode zenith_engine_stop(ZenithEngine*);    /* 停止设备，保留图与状态 */
ZenithStatusCode zenith_engine_prepare(ZenithEngine*); /* 重分配所有实时缓冲（非实时线程） */

ZenithStatusCode zenith_transport_play(ZenithEngine*);
ZenithStatusCode zenith_transport_pause(ZenithEngine*);
ZenithStatusCode zenith_transport_stop(ZenithEngine*);
ZenithStatusCode zenith_transport_seek(ZenithEngine*, ZenithMusicalTime pos);
ZenithStatusCode zenith_transport_set_loop(ZenithEngine*, ZenithMusicalTime start,
                                           ZenithMusicalTime end, uint8_t enabled);
ZenithStatusCode zenith_transport_set_tempo(ZenithEngine*, double bpm);
ZenithStatusCode zenith_transport_set_time_signature(ZenithEngine*, uint32_t num,
                                                     uint32_t den);
```

### 6.3 状态快照（**[S1]**，无锁读取）

```c
typedef struct ZenithEngineStatus {
  int64_t  playhead_ticks;
  double   bpm;
  float    cpu_load;            /* 实时率 0..1：DSP 耗时 / 可用时间 */
  uint32_t xrun_count;          /* 缓冲欠载累计次数（PLAN §3.S1.5） */
  uint32_t active_voices;
  uint32_t max_voices;
  uint32_t degrade_level;       /* 0=L0, 1=L1, 2=L2 */
  uint32_t state;               /* 0=stopped 1=playing 2=paused */
  uint32_t driver_kind;
  uint32_t sample_rate;
  uint32_t block_size;
  uint32_t _reserved;
} ZenithEngineStatus;

/* 无锁读取当前状态；out_status 必须有效 */
ZenithStatusCode zenith_engine_status(const ZenithEngine* engine,
                                      ZenithEngineStatus* out_status);
```

> **写入方是音频线程、读取方是 UI 线程**：Rust 侧用 `AtomicU64`/`AtomicU32`
> 打包写入（playhead 用 `AtomicI64`），保证读到的快照是**单一时刻的一致视图**，不撕裂。

### 6.4 参数与自动化（**[S2]**，PLAN §3.S2）

```c
/* 参数紧凑寻址：热路径不做字符串哈希（PLAN §3.S2 第 1 条） */
typedef struct ZenithParamId {
  uint16_t kind;    /* 参数类别，见 ZenithParamKind */
  uint16_t sub;     /* 子索引（如 EQ 频段号） */
  uint32_t index;   /* 目标对象索引（通道号/轨道号/效果槽号） */
} ZenithParamId;

ZenithStatusCode zenith_param_set(ZenithEngine*, ZenithParamId, float value);
ZenithStatusCode zenith_param_get(const ZenithEngine*, ZenithParamId, float* out_value);

/* 平滑时间（防止 zipper noise，PLAN §3.S2 第 3 条），单位毫秒，1..50 */
ZenithStatusCode zenith_param_set_smoothing(ZenithEngine*, ZenithParamId, float ms);
```

**求值顺序固定且文档化**（PLAN §3.S2 第 2 条）：
`基础值 → 自动化 → 调制器累加 → 钳制`。

```c
/* 参数描述符：Dart 侧据此自动生成效果器 UI（PLAN §3.S5「UI 自动生成」） */
typedef struct ZenithParamDescriptor {
  ZenithParamId id;
  float    min_value;
  float    max_value;
  float    default_value;
  uint32_t unit;          /* 0=线性 1=dB 2=Hz 3=秒 4=百分比 5=枚举 */
  uint32_t flags;         /* 1=可自动化 2=离散 4=对数显示 */
  const char* name_utf8;  /* 静态字符串，Dart 只读，不释放 */
  const char* label_utf8;
} ZenithParamDescriptor;

ZenithStatusCode zenith_param_describe(const ZenithEngine*, ZenithParamId,
                                       ZenithParamDescriptor* out_desc);
ZenithStatusCode zenith_effect_describe_params(const ZenithEngine*, uint32_t effect_id,
                                               ZenithParamDescriptor* out_descs,
                                               size_t capacity, size_t* out_count);
```

### 6.5 DSP 图与效果槽（**[S3]/[S5]**）

```c
ZenithStatusCode zenith_channel_add(ZenithEngine*, uint32_t* out_channel_index);
ZenithStatusCode zenith_channel_remove(ZenithEngine*, uint32_t channel_index);

ZenithStatusCode zenith_effect_insert(ZenithEngine*, uint32_t channel_index,
                                      uint32_t slot, uint32_t effect_kind);
ZenithStatusCode zenith_effect_remove(ZenithEngine*, uint32_t channel_index,
                                      uint32_t slot);

/* 建图时必须做环检测（PLAN §3.S3 第 2 条），成环返回 ZENITH_ERR_WOULD_CYCLE */
ZenithStatusCode zenith_route_connect(ZenithEngine*, uint32_t src_channel,
                                      uint32_t dst_channel);
ZenithStatusCode zenith_route_disconnect(ZenithEngine*, uint32_t src_channel,
                                         uint32_t dst_channel);

/* PDC：效果延迟报告（PLAN §3.S4 第 1 条） */
ZenithStatusCode zenith_effect_latency(const ZenithEngine*, uint32_t channel_index,
                                       uint32_t slot, uint32_t* out_samples);
```

### 6.6 MIDI / 事件注入（**[S6]**）

```c
/* 无锁 SPSC 队列写入；满则返回 ZENITH_ERR_CAPACITY，绝不阻塞音频线程 */
ZenithStatusCode zenith_midi_push(ZenithEngine*, uint32_t track_index,
                                  ZenithMusicalTime at, uint8_t status,
                                  uint8_t data1, uint8_t data2);
```

**规则**：MIDI 事件队列写入必须是**非阻塞**的。控制线程（UI 弹奏、MIDI 输入）
在队列满时丢弃并上报，而不是等待——等待会阻塞 UI，且若从音频线程写入会造成死锁。

### 6.7 电平表（**[S3]**）

```c
typedef struct ZenithMeterSnapshot {
  float peak_l;
  float peak_r;
  float rms_l;
  float rms_r;
  float peak_hold_l;   /* 3 秒峰值保持（PLAN §3.S3 第 7 条） */
  float peak_hold_r;
} ZenithMeterSnapshot;

ZenithStatusCode zenith_meter_read(const ZenithEngine*, uint32_t channel_index,
                                   ZenithMeterSnapshot* out_snapshot);
```

### 6.8 离线渲染与导出（**[S4]**）

```c
ZenithStatusCode zenith_render_offline(ZenithEngine*, ZenithMusicalTime start,
                                       ZenithMusicalTime end, uint32_t target_sample_rate,
                                       float** out_buffer, size_t* out_frames);
/* 必须与 out_buffer 配套调用，否则泄漏 */
void zenith_buffer_free(float* buffer, size_t frames);
```

> **离线走同一个 `AudioDriver`（`OfflineDriver`）**，不是第二套 DSP 实现——
> 这是「实时与离线逐样本一致」的结构保证（PLAN §3.S4 第 5 条）。
> `zenith_buffer_free` 的存在是 P3 的直接体现。

---

## 7. 线程模型与实时安全

### 7.1 线程角色

| 线程 | 归属 | 职责 | 约束 |
|---|---|---|---|
| **音频回调线程** | 平台驱动（`cpal`/`AudioWorklet`） | 推进 DSP 图、消费事件队列、写原子快照 | P5：零分配、零锁、零 IO、零 panic |
| **控制线程** | Dart（UI） | 调用导出函数改图/改参数/控制传输 | 不得高频轮询破坏帧率；不得阻塞 |
| **渲染线程池** | Rust | 离线渲染、卷积 IR 预处理、时间拉伸 | `cfg(not(wasm32))`，Web 端不可用 |
| **Dart UI 线程** | Flutter | 每帧无锁读取快照 | 只读，不写引擎状态 |

### 7.2 跨线程通信

1. **Dart → 音频线程**：参数写入用**原子变量**；事件（MIDI/音符）用**无锁 SPSC 环形缓冲**（PLAN §0.3）；
2. **音频线程 → Dart**：只写原子快照（§6.3、§6.7），Dart 只读；
3. **禁止**：音频线程上加锁、分配、等条件变量、写文件、打印日志——任何一项都会造成爆音或死锁。

### 7.3 图变更的安全协议（**[S3]**）

修改 DSP 图（增删节点/改路由）**不得**在音频线程直接进行。协议：

1. 控制线程构建**新的图描述**；
2. 通过无锁命令队列投递给音频线程；
3. 音频线程在**块边界**原子切换（交换指针），旧图延后释放（**RCU 式延迟回收**，避免音频线程释放内存）。

**[S0 契约]**：任何「引擎结构变更型」API 调用都必须在文档中说明「是否实时安全」。
本文件约定：**所有图结构变更 API 一律非实时安全**，只能从控制线程调用。

---

## 8. 构建、产物与加载

### 8.1 构建集成（**[S0 落地]**，PLAN §3.S0 第 5 条）

- `native/zenith_core/Cargo.toml`，`crate-type = ["staticlib", "cdylib", "rlib"]`
- `hook/build.dart`（native_assets hook）驱动 `cargo build`
- 现有 `build/native_assets/windows/native_assets.json` 内容为
  `{"format-version":[1,0,0],"native-assets":{}}`——**空 map，正好由 hook 接管填充**

### 8.2 各平台产物命名

| 平台 | target triple | 产物 |
|---|---|---|
| Windows | `x86_64-pc-windows-msvc` | `zenith_core.dll` |
| macOS | `aarch64-apple-darwin` / `x86_64-apple-darwin` | `libzenith_core.dylib` |
| Linux | `x86_64-unknown-linux-gnu` | `libzenith_core.so` |
| Android | `aarch64-linux-android` 等 | `libzenith_core.so` |
| iOS | `aarch64-apple-ios` | `libzenith_core.a`（静态链接） |
| **Web** | **`wasm32-unknown-unknown`** | **`zenith_core.wasm`** |

### 8.3 CI 必需的守卫（PLAN §4.2 第 8 条）

CI（`.github/workflows/build.yml`，当前 6 job 均未装 Rust）每个 job 在 `flutter build` 前必须：

```
rustup target add <triple>
cargo check --target <triple> --locked
cargo clippy --all-targets -- -D warnings
cargo test
```

**其中 `cargo check --target wasm32-unknown-unknown` 是硬性门禁**——它拦截
`std::thread`/`std::fs`/`Instant` 的误用，是 Web 端能否存活的唯一自动化保障。

### 8.4 Dart 侧加载

`lib/engine/ffi/library_loader.dart` 按平台定位动态库（Web 走 WASM 实例化路径，不走 `DynamicLibrary.open`），
加载后**先做 §2.3 的版本与 size 校验**，再构造 `ZenithEngine`。

---

## 9. 契约同步与验证（PLAN §4.2 第 2 条）

### 9.1 变更流程（强制）

1. 在 `docs/COORDINATION.md` 提案，标注影响范围与涉及 agent；
2. 修改 `src/ffi/types.rs` **与** `lib/engine/ffi/native_types.dart`（**同一提交内**）；
3. 更新本文件对应章节 + §9.3 结构体清单表；
4. 两侧 `size_of` 断言测试同步更新；
5. `cargo test` + `flutter test` 双绿。

### 9.2 自动化验证项（缺一不可）

| 验证 | 手段 | 拦截的问题 |
|---|---|---|
| 结构体大小一致 | Rust `size_of` 断言 + Dart `sizeOf` 断言 + 运行时 `zenith_sizeof_*` 比对 | 镜像漂移（P8） |
| 无 panic 逃逸 | `clippy::unwrap_used` deny + FFI 边界 `catch_unwind` | UB（P4） |
| 零分配 | `assert_no_alloc` 在 60 秒播放测试中零触发 | 实时爆音（P5） |
| WASM 可编译 | `cargo check --target wasm32-unknown-unknown` | 平台依赖泄漏（P7） |
| ABI 版本匹配 | 加载时 major 校验 | 版本错配（P9） |
| 无第三方品牌名 | CI grep 规则（PLAN §4.2 第 6 条） | §0.2 约束 |

### 9.3 结构体镜像清单（改一侧必须改另一侧）

| Rust（`src/ffi/types.rs`） | Dart（`lib/engine/ffi/native_types.dart`） | 状态 |
|---|---|---|
| `ZenithEngineConfig` | `ZenithEngineConfig` | **[S0 落地]** |
| `ZenithEngineStatus` | `ZenithEngineStatus` | [S1] |
| `ZenithMusicalTime` | `ZenithMusicalTime` | **[S0 落地]**（tick 化基座） |
| `ZenithParamId` | `ZenithParamId` | [S2] |
| `ZenithParamDescriptor` | `ZenithParamDescriptor` | [S2] |
| `ZenithMeterSnapshot` | `ZenithMeterSnapshot` | [S3] |

---

## 10. 显式非目标（Out of Scope）

以下**不在本 ABI 范围内**，避免后续 agent 越界：

1. **不做回调 ABI**（§5.3）——状态一律轮询快照；
2. **不导出 Rust 结构体内部指针**——只用不透明 handle 与 `u32` 索引；
3. **不做 Dart 侧效果器实现**——效果器全在 Rust（S5），Dart 只查询描述符生成 UI（PLAN §3.S0 第 4 条）；
4. **不在 Web 端加载外部插件**——CLAP 宿主仅桌面端（PLAN §3.S7 跨平台边界）；
5. **不使用 VST SDK**——许可与 AGPL-3.0 冲突（PLAN §0.3）；
6. **不假设块大小恒定**（P6）；
7. **不保证跨 major 版本的 ABI 兼容**——major 变更要求同步升级 Dart 侧。

---

## 11. 待决问题（S0 期间必须回答）

| # | 问题 | 影响 | 建议 |
|---|---|---|---|
| Q1 | `ZenithEngineConfig.block_size` 在 Web 端是否强制为 128（`AudioWorklet` 常见渲染量子）？ | 影响 P6 与 Web 降级阈值 | 允许 64..2048，Web 端由驱动实际值回写 |
| Q2 | 效果器 `effect_kind` 的 ID 空间如何分配（内置 vs 插件）？ | 影响 S5/S7 寻址 | 内置 `0x0000_0000..0x0000_FFFF`，插件 `0x0001_0000+` |
| Q3 | 轨道/通道索引复用策略（删除后是否复用索引）？ | 影响 Dart 侧句柄有效性 | **不复用**，配合 generation 计数防 ABA |
| Q4 | `ZenithMusicalTime.ppq` 是全局还是每工程？ | 影响换算权威 | 每工程，默认 960，写入 config |
| Q5 | Android 多 ABI（armv7/arm64/x86_64）是否全支持？ | 影响 APK 体积与构建时间 | S1 先 arm64，其余按需 |

---

## 12. 变更记录

| 版本 | 日期 | 变更 | 作者 |
|---|---|---|---|
| v1.0 | — | 首次冻结：确立 10 条设计原则、版本策略、类型映射、错误码、所有权模型、函数面清单、线程模型、构建产物、同步验证机制 | S0 契约基线 |

---

**相关文档**
- `docs/PLAN_DAW_PARITY.md` —— 总计划（本文件的父文档，§0.2/§0.3/§4.2 为硬性约束来源）
- `docs/COORDINATION.md` —— ABI 变更提案登记（**尚未创建，S0 需建立**）

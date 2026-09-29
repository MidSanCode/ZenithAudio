# S0 — 基线与构建修复报告

**状态**：✅ 完成
**标签**：`s0-baseline`
**计划依据**：`docs/PLAN_DAW_PARITY.md` v2.0 §7 S0

---

## 1. 结论摘要

S0 的三项目标全部达成，并可复现：

| 验收项 | 结果 | 证据 |
|---|---|---|
| `flutter analyze` 零 error | ✅ **0 error**（97 项 info/warning，全部为既有告警） | `dart analyze lib test hook` |
| `flutter test` 全绿 | ✅ **99 / 99 通过**（基线 50 → 99，新增 49 项） | `flutter test` |
| `cargo clippy -D warnings` | ✅ 零输出、退出码 0 | `cargo clippy --all-targets -- -D warnings` |
| `cargo test` | ✅ **4 / 4 通过** | `cargo test` |
| `flutter build windows --debug` 链接 Rust 静态库 | ✅ 构建成功，`zenith_core.dll` 落在 exe 同级 | 见 §4 |
| WASM 可编译（§0.2 硬约束） | ✅ `cargo build --target wasm32-unknown-unknown` 成功 | 见 §5 |

---

## 2. 接手时发现的阻断（已修复）

S0 实际开工时仓库**处于编译不通过状态**（135 个 analyzer error），来自
上一个会话（Agent-E，commit `038d1b1`）未完成的 tick 重构。这属于计划 §7 S0
第 1 项"先修好构建"，故优先处理：

1. **`lib/services/playlist_engine.dart` 的 import 缺前缀**
   —— 5 个模型 import 写成 `musical_time.dart` 而非 `../models/musical_time.dart`。
   修复后 error 135 → 39。

2. **`Note` 构造函数新增必填 `startTicks` / `lengthTicks`** 导致 39 处
   `missing_required_argument`。全部按"秒 → tick"换算语义迁移到
   `Note.fromSeconds(...)`，并补上 `bpm` 参数。

3. **`lgdf_project_codec.dart` 的 `List<Pattern>?` 类型不匹配** —— 补 `?? const []`。

修复后 `flutter analyze` 归零、`flutter test` 50/50 通过，与重构前行为一致。

> 说明：本次没有回退 Agent-E 的 tick 设计。其"tick 为权威、秒为派生视图"的
> 方向与计划 §2 一致，且已通过 50 项既有测试，回退反而会引入风险。

---

## 3. 文件拆分（计划 §7 S0 第 3 项：>800 行必须拆）

| 原文件 | 行数 | 拆分结果 |
|---|---|---|
| `lib/services/synth_engine.dart` | 615 | → `dsp/synth_common.dart`(42) + `dsp/svf_filter.dart`(43) + `dsp/compressor.dart`(69) + `dsp/wavetable.dart`(72) + `synth_voice.dart`(248) + `synth_render_job.dart`(154)；原文件变为 26 行 barrel |
| `lib/providers/project_provider.dart` | 881 | → `project_provider.dart`(492) + `project_io.dart`(364) + `project_undo.dart`(67) |
| `lib/models/instrument.dart` | 698 | → `instrument.dart`(324) + `instrument_dsp_params.dart`(113) + `instrument_presets_data.dart`(298) |

**拆分手法与理由**：

- **`synth_engine.dart` / `instrument.dart` 用 barrel + extension/export**：
  原文件保留 `export` 或 `export` 声明，**所有既有 import 路径零改动**。
  这两个文件的公开 API（`SynthVoice`、`InstrumentPreset.synthSample` 等）
  在调用点看来完全没变。
- **`project_provider.dart` 用 `part` / `part of` + mixin**：
  该 Notifier 的撤销栈、`_currentFilePath`、`_isDirty` 都是私有的，跨文件拆分
  必须共享私有作用域。最初尝试用 `extension`，但 Riverpod 的 `state`
  是 `protected`（`invalid_use_of_protected_member`），extension 无法访问，
  因此改为 `mixin _ProjectHistoryMixin on Notifier<Project>`——mixin 在类体内
  展开，可以正常读写 `state` 和私有字段。

拆分后每个文件均 < 800 行（最大 492）。

---

## 4. Rust 核心骨架与 native_assets 接线

### 4.1 新增文件

```
Cargo.toml                              # workspace 根 + release profile
.cargo/config.toml                      # target-dir / alias
native/zenith_core/Cargo.toml           # crate-type = ["staticlib","cdylib","rlib"]
native/zenith_core/src/lib.rs           # ABI + 4 项单元测试
hook/build.dart                         # native_assets 构建驱动
lib/native/zenith_core.dart             # dart:ffi 绑定
```

### 4.2 ABI 表面（S0 仅最小握手）

```rust
pub const ABI_VERSION: u32 = encode_version(0, 1, 0);   // = 256

extern "C" fn zenith_version() -> u32;                  // 链接探针
extern "C" fn zenith_version_match(expected: u32) -> u32;
extern "C" fn zenith_version_string() -> *const c_char; // 'static "0.1.0"
```

`zenith_version_match` 返回状态码而不是 panic：Rust panic 越过 FFI 边界进入
Dart VM 是 UB，版本不匹配必须由 Dart 侧决定如何提示用户。

### 4.3 关键：hook 真实生效

`hook/build.dart` **不是占位符**——它在 `flutter build` 期间真的调用了
`cargo build`，产物被 Flutter 打包：

```
build/windows/x64/runner/Debug/
├── zenith_audio.exe
└── zenith_core.dll          ← 104,960 bytes，由 hook 产出
```

DLL 内可检索到全部三个导出符号（`zenith_version` / `zenith_version_match` /
`zenith_version_string`）。构建后启动 exe，进程正常运行、stderr 无输出。

**API 版本注意**：当前 `hooks` 2.0.2 **没有** `BuildType` / `input.config.buildType`。
release/debug 改为读 user-define（`input.userDefines['release']`，默认 release）。
计划文本里写的 `native_assets_cli` 在当前 Dart 3.12.2 生态中已更名为
`hooks` + `code_assets`，本实现按新包名接线。

### 4.4 Dart 侧 FFI 绑定

`lib/native/zenith_core.dart` 是**全仓库唯一**了解 C ABI 形状的地方；
延迟加载，import 本身不加载动态库。

---

## 5. WASM 硬约束验证

计划 §0.2 要求核心"不依赖 `std::thread`、文件系统、系统时钟"，否则失去 Web 端。
S0 通过**实际编译**验证而非口头声明：

```
cargo build -p zenith_core --target wasm32-unknown-unknown   → 成功
```

（`wasm32-unknown-unknown` target 开发机已安装。）

---

## 6. 接口预留

按计划 §7 S0 第 4 项，**只定义接口，不定义 Dart 侧效果器抽象**：

| 文件 | 内容 |
|---|---|
| `lib/engine/engine.dart` | `AudioEngine`（S1 用 FFI 实现）、`SampleFormat`、`PlaybackRequest`、`TransportState`、`TransportPosition`、`LevelSnapshot` |
| `lib/automation/parameter.dart` | `ParameterId`、`ParameterDescriptor`、`ParameterKind`、`AutomationPoint`、`ParameterStore` |
| `lib/plugins/plugin_host.dart` | `PluginHost`、`PluginInstance`、`PluginDescriptor`、`PluginFormat` |

**刻意不提供** Dart 侧 `Effect` / 效果器接口——效果器为 Rust 实现（S5），
Dart 只通过 FFI 查询参数描述符生成 UI。若新增 Dart 侧效果器抽象，会立刻产生
第二套竞争的实现面。

设计要点：
- `ParameterId` 用 `(ownerId, parameterKey)` 字符串对而非 Dart 对象——同一个
  身份要穿过 C ABI、工程文件和插件宿主，用对象就得在每个边界再造一套编码。
- `AutomationPoint` 用**帧**而非秒，与 tick-first 模型一致：变速时自动化不漂移。
- `TransportPosition` 用帧，秒是派生视图（`asDuration`）。

---

## 7. 新增测试（49 项）

| 文件 | 项数 | 覆盖 |
|---|---|---|
| `test/musical_time_test.dart` | 27 | PPQ=960 常量、拍/秒/小节换算、各类时值（含三连音/五连音/附点）落在整数 tick、`snap`/`swingOffset`/`quantize`/`safe` 边界 |
| `test/legacy_project_compat_test.dart` | 17 | 旧工程（秒制、无 tick 字段）反序列化、缺字段回退、tick 优先于秒、`withTempo` 保持音乐位置、`copyWith` 秒→tick 换算；`PlaylistEngine.fromTracks` 迁移（整小节对齐、空轨/音频轨跳过、lane 连续、id 稳定、回滚 `flatten` 后逐音符位置一致）、迁移结果 JSON 往返 |
| `test/native_ffi_smoke_test.dart` | 5 | 加载原生库、`zenith_version()` 非零且 ==256、Dart/Rust ABI 一致、非法版本被拒、版本字符串可读 |

FFI 测试在原生库缺失时会 `markTestSkipped` 而非硬失败——开发机上不阻塞，
但 CI 必须构建后运行才算通过。

---

## 8. 计划执行中的偏离与说明

1. **未拆分计划未点名的 800+ 行文件**（`audio_clip_editor.dart` 1377、
   `piano_roll_editor.dart` 1125、`track_tile.dart` 819 等）。
   依据用户先前决定："只拆计划点名的文件"。S0 只拆了 `synth_engine.dart`、
   `project_provider.dart`、`instrument.dart` 三个。

2. **计划 §0.2 的行数与实际不符**：`instrument.dart` 记为 653、实际 698。
   已在按实际值处理。

3. **`pubspec.yaml` 新增 3 个直接依赖**：`code_assets ^1.2.1`、`hooks ^2.0.2`、
   `ffi ^2.1.3`（后者原本只是传递依赖，但绑定文件直接 import 它，
   按惯例应声明为直接依赖）。

4. **`.gitignore` 新增 Rust 产物忽略**。此前 `auto_snapshot` 把 126 个
   `target/` 构建产物纳入了版本控制，已 `git rm --cached` 移除。
   `Cargo.lock` **不忽略**——本仓库分发二进制，lockfile 属于构建契约。

---

## 9. 风险与遗留（供 S1 参考）

| 风险 | 等级 | 说明 |
|---|---|---|
| `audio_service.dart` 是**纯转发 barrel**，全仓库无 import 点 | **高** | 其内容仅为 `export 'audio_service_io.dart' if (dart.library.html) 'audio_service_web.dart';`。S1 把 `Provider<AudioService>` 换成 Rust 引擎时，**不会有编译错误、也不需要改任何调用点**——静默重绑定。建议 S1 第一步先给它加一个显式的 `AudioEngine` 适配层与冒烟测试。 |
| native_assets hook 仅在 Windows 验证 | 中 | 计划已预期（§7 S0 风险表）。`hook/build.dart` 的 triple 映射已覆盖 5 个平台，但 Linux/macOS 未实机验证。 |
| hook 在 hooks 2.0.2 无 `buildType` | 低 | 改用 user-define 控制 release/debug，默认 release。若上游后续提供 `buildType`，应切回官方字段。 |
| `flutter analyze` 仍有 97 项 info/warning | 低 | 全部为既有告警（未用 import 等），非本次引入。S0 验收只要求"零 error"。 |

---

## 10. 复现步骤

```powershell
# Dart 侧
flutter pub get
dart analyze lib test hook          # 期望 0 error
flutter test                        # 期望 99 passed

# Rust 侧
cargo clippy --all-targets -- -D warnings   # 期望无输出，退出码 0
cargo test                                  # 期望 4 passed

# 端到端（native_assets 接线）
flutter build windows --debug
# 期望 build\windows\x64\runner\Debug\zenith_core.dll 存在

# WASM 硬约束
cargo build -p zenith_core --target wasm32-unknown-unknown
```

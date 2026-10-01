# S5 effect authoring contract (internal task brief)

This file is a **working brief** for parallel S5 authoring sessions. It is not a
project document and can be deleted once S5 lands.

## Read these first

- `native/zenith_core/src/effects/mod.rs` — the `EffectProcessor` trait. Read the
  whole file.
- `native/zenith_core/src/effects/filter/multimode.rs` — the **reference
  implementation**. Copy its structure exactly.
- `native/zenith_core/src/effects/util/mod.rs` — shared math and oversampling.
- `native/zenith_core/src/effects/registry.rs` — the kind id table and the tests
  that will run against your effect.
- `native/zenith_core/src/automation/parameter.rs` — `ParameterDescriptor`,
  `ParameterUnit`, `parameter_flags`, `ParameterAddress`.
- `docs/PLAN_DAW_PARITY.md` §3.S5 and `docs/ABI.md` §6.4.
- `docs/COORDINATION.md` C-011 (the ABI registration for this work).

## Hard rules — a violation is a bug, not a style choice

1. **No third-party DAW brand names** anywhere: code, comments, docs, UI strings.
   This is PLAN §0.2 and is checked by CI grep. If you cannot think of a neutral
   way to describe an algorithm, describe it by its DSP properties instead.
2. **`process` must not allocate.** No `Vec::push`, `Box::new`, `String`,
   `to_vec()`, `vec![]`, `collect()`, or `format!` inside `process`. Every buffer
   is allocated in `prepare` and stored on the struct. The registry test
   `every_registered_effect_processes_a_block_without_panicking` and the
   zero-allocation guard will catch violations.
3. **`process` must not lock or do IO.** No `Mutex`, `println!`, file access.
4. **Only edit files inside your assigned directory.** Do not touch `lib.rs`,
   `ffi/`, `registry.rs`, `mod.rs` of a parent directory, or another effect's
   files. Report what you need wired instead.
5. **WASM must keep compiling.** Do not use `std::thread`, `std::fs`,
   `std::time::Instant`, or `f32::sin`/`cos`/`exp`/`powf`/`sqrt`/`tan`.
   Use `effects::util::dsp` (`sin_poly`, `cos_poly`, `exp2`, `log2`, `log10`,
   `powf`, `sqrt`, `tan_poly`, `db_to_gain`, `gain_to_db`, `one_pole_coeff`,
   `clamp_frequency`, `tanh_poly`) or `core::f32::consts`.
6. **`cargo clippy --all-targets -- -D warnings` must be clean.** Clippy rejects
   an approximate literal for a known constant — write `core::f32::consts::PI`,
   not `3.14159`.
7. **`cargo fmt` must be clean.**
8. **Do not add dependencies.** The crate has none and must keep none.

## Required structure of each effect file

Follow `filter/multimode.rs` exactly:

```rust
//! Module docs: what it does, why the algorithm was chosen, real-time notes.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::{clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor};
use crate::automation::parameter::{parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit};
// plus whatever shared math you need from super::super::util::dsp

/// Parameter ordinals, as `pub const PARAM_X: u16 = n;` covering 0..PARAM_COUNT.
/// One doc comment each.
pub const PARAM_COUNT: u16 = N;

/// Static description. `kind` comes from `super::super::registry::KIND_*`.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor { ... };

/// `pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize]`
/// Build with `ParameterAddress::effect(address.index, address.effect_slot(), sub)`.
/// Every `min_value <= default_value <= max_value`. Every key is non-empty,
/// lowercase, `[a-z_]+`, and unique within this effect.

/// The processor struct. All scratch is `alloc::vec::Vec<f32>` sized in `prepare`.
pub struct YourEffect { ... }

impl EffectProcessor for YourEffect {
    fn descriptor(&self) -> &'static EffectDescriptor { &DESCRIPTOR }
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) { /* ALL allocs here */ }
    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext) { /* zero alloc */ }
    fn reset(&mut self) { ... }
    fn latency_samples(&self) -> usize { ... }   // MUST be accurate; see below
    fn parameters(&self) -> &[ParameterDescriptor] { &self.table }
    fn set_parameter(&mut self, sub: u16, value: f32) { ... }
    fn get_parameter(&self, sub: u16) -> Option<f32> { ... }
    fn is_bypassed(&self) -> bool { self.bypassed }
    fn set_bypassed(&mut self, bypassed: bool) { self.bypassed = bypassed; }
    fn wet(&self) -> f32 { self.wet }
    fn set_wet(&mut self, wet: f32) { self.wet = sanitize_wet(wet); ... }
    // fn tail_seconds(&self) -> f32  — override when the effect has a tail
}
```

## Non-negotiable behaviours every effect must implement

These are what the registry-wide tests and the acceptance criteria rely on.
Write a focused test for each.

| Behaviour | Requirement |
|---|---|
| **Bypass** | `set_bypassed(true)` → `process` returns the buffer **byte-identical**. Test with `assert_eq!` on the whole block. |
| **Zero mix** | `set_wet(0.0)` → output equals input within `1e-6`. |
| **Clamping** | Out-of-range parameter values clamp to the descriptor bounds; `NaN` falls back to `default_value`. Never reject, never panic. |
| **Unknown ordinal** | `set_parameter(999, x)` is a no-op; `get_parameter(999) == None`. |
| **Oversized block** | A block larger than `prepare` sized for must return the buffer untouched rather than index out of bounds. Test with a 512-frame block after `prepare(_, 256, _)`. |
| **Non-finite input** | `NaN`/`±inf` input must not leave non-finite values in the output or in filter state. |
| **Reset** | Clears delay lines / filter history / envelopes. |
| **Finite output under extremes** | Every parameter at its maximum simultaneously must not produce `inf`/`NaN`. Assert `is_finite()` over several blocks. |
| **Stereo independence** | Filter/delay state is per channel; a loud left must not leak into a silent right. |
| **`parameters().len() == PARAM_COUNT`** | The registry test asserts this. |
| **Descriptor consistency** | `descriptor().param_count == PARAM_COUNT`; `param_range()` starts at 0 and covers the table. |

## Latency — get this right, it is a correctness requirement

`latency_samples()` feeds PDC (S4 §3.S4 requirement 1). A wrong value misaligns
**every other track in the project**, not just this one.

- Zero-latency (no delay line in the audio path, no look-ahead, no oversampling):
  return `0`. A biquad cascade, a waveshaper, the analyser.
- Oversampling: `oversampler.latency_samples()` — it is already expressed at the
  base rate. Do **not** multiply by the factor.
- Look-ahead (a limiter, a look-ahead compressor): the look-ahead window length
  in samples.
- A reverb/delay whose *wet* path is delayed but which reports its delay as a
  musical parameter: report `0` only if the dry path is undelayed and the wet
  path's delay is the effect's audible content. When in doubt, report the delay
  and say why in a comment.

Test it: assert the exact expected number, and assert it is `0` when the
latency-producing feature is off (e.g. drive at 0 dB).

## Real-time safety pattern to copy

Do **not** copy the dry signal with `to_vec()`. Do this:

```rust
fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
    self.dry = alloc::vec![0.0; max_block];
    self.wet_buf = alloc::vec![0.0; max_block];
    self.max_block = max_block;
    // ...size oversamplers, delay lines, FFT tables...
}

fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext) {
    let frames = buffer.frames();
    if frames == 0 || frames > self.max_block || frames > self.dry.len() { return; }
    for channel in 0..buffer.channel_count().min(MAX_CHANNELS) {
        { let Some(src) = buffer.channel(channel) else { continue };
          self.dry[..frames].copy_from_slice(src); }
        // ...work in self.wet_buf[..frames] against self.dry[..frames]...
        if let Some(dst) = buffer.channel_mut(channel) {
            for (i, out) in dst.iter_mut().enumerate() {
                let wet = self.wet_buf.get(i).copied().unwrap_or(0.0);
                let dry = self.dry.get(i).copied().unwrap_or(0.0);
                *out = wet * self.wet + dry * (1.0 - self.wet);
            }
        }
    }
}
```

## Test style

- Tests live in a `#[cfg(test)] mod tests` at the bottom of your file.
- Use `alloc::vec!` / `alloc::vec::Vec` (the crate is `no_std`-shaped).
- Build test helpers that process a real block through the real `process` — do
  not test only the internals.
- Measure responses by driving a sine and reading the settled peak, so the test
  is independent of any analytic formula you also wrote (if both come from the
  same wrong assumption, they agree and the test passes vacuously).
- Name tests as assertions about behaviour:
  `a_low_pass_attenuates_high_frequencies`, not `test_filter_1`.

## Reporting back

Reply with: file path(s) written, the exact `cargo test` filter you ran, the
pass count, and anything you could not do. Do not edit `registry.rs` — if the
kind id or descriptor there disagrees with your file, say so in your report.

//! Distortion: saturation and bit reduction.
//!
//! Two ways to damage a signal on purpose, which is why they share a module
//! and a category but almost no code.
//!
//! * [`saturation`] is an **analogue-shaped** nonlinearity: a memoryless
//!   transfer curve (a smooth one, a hard clip or a fold) applied to the
//!   waveform, oversampled so the harmonics it generates do not fold back as
//!   aliasing. It is continuous — pushing it harder moves the signal smoothly
//!   toward saturation.
//! * [`bitcrush`] is a **digital** degradation: quantisation to a small number
//!   of amplitude steps and sample-and-hold at a low rate. It is deliberately
//!   discontinuous; the steps and the stair-stepping *are* the effect.
//!
//! # Why only the saturator is oversampled
//!
//! The saturator's aliasing is an accident of sampling a continuous curve: the
//! harmonics above Nyquist were never meant to exist and fold back as
//! inharmonic tones. Oversampling puts them where the decimation filter can
//! remove them, so it is on by default.
//!
//! The bit crusher's artefacts are the *intent*, and neither half of it gains
//! from oversampling. Quantisation to `2^N` levels is a memoryless map, and the
//! decimation filter of an oversampled round trip would smear the steps it
//! produces back together — undoing the effect. Sample-and-hold is likewise
//! defined at the base rate: the hold length is derived from the target rate
//! against the block's own sample rate, so there is nothing above the base
//! Nyquist to clean up. `registry::default_oversampling` nevertheless reports
//! 4x for this kind, because that is the quality badge the *plan* assigns the
//! distortion family; the value is diagnostics, not a promise about this
//! processor's internals.
//!
//! # Real-time safety
//!
//! Both allocate every buffer in `prepare`; `process` only reads, multiplies
//! and writes. DC blocking is mandatory on both — a nonlinearity with any
//! asymmetry generates an offset, and quantisation generates a bias whenever
//! the signal does not sit exactly on a step boundary.

pub mod bitcrush;
pub mod saturation;

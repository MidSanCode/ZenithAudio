//! Shared DSP primitives for the real-time engine (Agent-A, S1).
//!
//! These are the small, allocation-free building blocks the engine and the
//! voice layer use: filters, a radix-2 FFT and a linear resampler. They are
//! deliberately independent of the effect suite (`crate::effects`), which has
//! its own, richer processor implementations.
//!
//! # Real-time discipline
//!
//! Nothing in this module allocates or locks once `prepare` has run. The
//! transcendental helpers are imported from [`crate::effects::util::dsp`] so
//! that the core keeps a single implementation and stays free of the platform
//! maths library (`wasm32` has none, ABI principle P7).

pub mod biquad;
pub mod fft;
pub mod resampler;
pub mod svf;

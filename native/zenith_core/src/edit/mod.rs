//! Audio editing algorithms (PLAN §3.S8): time stretch, pitch shift, transient
//! detection and crossfading.
//!
//! # Scope
//!
//! These are **offline, buffer-in/buffer-out** transforms. They allocate their
//! result and are not on the audio path, which is what lets them be unit-tested
//! as pure functions of `&[f32]`. The real-time engine never calls them; the
//! audio editor does.
//!
//! # Platform
//!
//! Everything here is `f32` arithmetic with no platform dependency, so the
//! module compiles for `wasm32` like the rest of the core (ABI principle P7).
//! Web builds may choose to skip the heaviest transforms at runtime (the S1.5
//! degradation policy), but the code still compiles.

pub mod crossfade;
pub mod time_stretch;
pub mod transient;

pub use crossfade::{crossfade, equal_power_curves, FadeCurve};
pub use time_stretch::{pitch_shift, resample_linear, time_stretch, StretchConfig};
pub use transient::{detect_transients, slice_at, TransientConfig};

//! Filters and signal-shaping effects.
//!
//! [`biquad`] is the shared two-pole building block; [`multimode`] wraps it as
//! an insert effect with resonance, drive and an envelope follower.

pub mod biquad;
pub mod multimode;

pub use biquad::{Biquad, FilterMode};
pub use multimode::MultimodeFilter;

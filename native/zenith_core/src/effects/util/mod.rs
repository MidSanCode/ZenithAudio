//! Shared utilities for the effect suite.
//!
//! * [`dsp`] — the math the core cannot take from `std`, plus shared filter
//!   design helpers and a DC blocker.
//! * [`oversampling`] — the **single** oversampling implementation. PLAN
//!   §3.S5 forbids each effect rolling its own, because two half-band filters
//!   that disagree produce two different amounts of aliasing for the same
//!   nominal "4x" setting.

pub mod dsp;
pub mod oversampling;

pub use dsp::{
    clamp_frequency, cos_poly, db_to_gain, exp2, gain_to_db, log10, log2, one_pole_coeff, powf,
    sin_poly, sqrt, tan_poly, wrap_pi, DcBlocker,
};
pub use oversampling::{
    Oversampler, OversamplerBank, OversamplingFactor, HALF_BAND_TAPS, MAX_OVERSAMPLED_CHANNELS,
};

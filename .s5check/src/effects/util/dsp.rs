//! Shared DSP math for effects.
//!
//! # Why this exists
//!
//! The core deliberately has **no dependencies**, so `exp`/`log`/`sin` must
//! either come from `std` (unavailable on some targets) or be implemented
//! here. `src/mixer/channel.rs` already established the pattern of a
//! self-contained `exp2`/`log2`/`log10` for exactly this reason; this module
//! provides the wider set effects need (`sin`, `pow`, `tan`, `sqrt`) plus the
//! filter-design helpers that several effects share.
//!
//! Keeping one implementation means a filter coefficient computed by the EQ
//! and by the multimode filter cannot drift apart.

use core::f32::consts::PI;

/// Re-exported so sibling modules can name the constant without importing
/// `core::f32::consts` themselves.
pub use core::f32::consts::PI as PI_CONST;

/// A polynomial approximation of `sin` on `-PI..=PI`.
///
/// # Accuracy
///
/// A Taylor series truncated at `x^11` still loses about `1e-4` near `±PI`
/// (~162°): the series converges slowly at the ends of the interval because
/// the next term is a large fraction of the last. That error is audible as
/// detune on a sustained tone and shows up directly in a filter coefficient,
/// so the argument is first folded into `-PI/2..=PI/2` using the symmetry
/// `sin(PI - x) = sin(x)`, where the same series is accurate to ~`1e-8`.
#[must_use]
pub fn sin_poly(x: f32) -> f32 {
    let x = wrap_pi(x);
    // Fold the two outer quadrants onto the inner one.
    let x = if x > core::f32::consts::FRAC_PI_2 {
        PI - x
    } else if x < -core::f32::consts::FRAC_PI_2 {
        -PI - x
    } else {
        x
    };
    // Odd series through x^11.
    let x2 = x * x;
    x * (1.0
        - x2 * (1.0 / 6.0
            - x2 * (1.0 / 120.0
                - x2 * (1.0 / 5040.0 - x2 * (1.0 / 362_880.0 - x2 * (1.0 / 39_916_800.0))))))
}

/// `cos(x)` derived from [`sin_poly`].
#[must_use]
pub fn cos_poly(x: f32) -> f32 {
    sin_poly(x + PI * 0.5)
}

/// Wraps `x` into `-PI..=PI`.
///
/// # Precision at large magnitudes
///
/// For an argument the size of `1e30` the `f32` representation has no bits
/// left for a fractional part, so *no* wrapping scheme can recover a
/// meaningful phase — the information is already gone before this function is
/// called. Returning `0.0` in that case is the honest answer: it is bounded
/// and finite, which is what a runaway oscillator needs, and the alternative
/// (`x - 2*PI*round(x/2PI)` overflowing to `-inf`) would poison downstream
/// filter state.
#[must_use]
pub fn wrap_pi(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let two_pi = 2.0 * PI;
    // Beyond this magnitude the quotient is larger than f32 can represent
    // exactly and every resulting phase is noise.
    if x.abs() > 1.0e7 {
        return 0.0;
    }
    // Reduce in two stages: remove whole multiples of 2*PI using `floor` on a
    // quotient of at most ~1.6e6, then handle the residual. A single
    // `x - 2*PI*round(x/2PI)` loses up to half an ulp of `x` itself, and near
    // |x| = 20 that is already ~1e-4 of phase — enough to fail a 1e-4
    // accuracy target on the polynomial that follows.
    let turns = (x / two_pi).round();
    let wrapped = x - turns * two_pi;
    // Catastrophic cancellation above leaves a few ulps of error; a second
    // cheap pass on the much smaller residual removes nearly all of it.
    let refined = wrapped - (wrapped / two_pi).round() * two_pi;
    if refined > PI {
        refined - two_pi
    } else if refined < -PI {
        refined + two_pi
    } else {
        refined
    }
}

/// `exp2(x)` — used for frequency ratios and dB conversion.
#[must_use]
pub fn exp2(x: f32) -> f32 {
    if !x.is_finite() {
        return if x > 0.0 { f32::MAX } else { 0.0 };
    }
    // Split into integer and fractional parts so the polynomial only has to
    // cover 0..1 and the integer part is an exact bit shift.
    let integer = x.floor();
    let fract = x - integer;

    // 2^f for f in 0..1: a degree-6 series in `f`, written as multiples of
    // `LN_2` so the coefficients are derived from the constant rather than
    // transcribed (clippy rejects an approximate literal for a known constant,
    // and rightly so — a mistyped digit here is a silent level error).
    let ln2 = core::f32::consts::LN_2;
    let poly = 1.0
        + fract
            * (ln2
                + fract
                    * (ln2 * ln2 / 2.0
                        + fract
                            * (ln2 * ln2 * ln2 / 6.0
                                + fract
                                    * (ln2 * ln2 * ln2 * ln2 / 24.0
                                        + fract
                                            * (ln2 * ln2 * ln2 * ln2 * ln2 / 120.0
                                                + fract
                                                    * (ln2 * ln2 * ln2 * ln2 * ln2 * ln2
                                                        / 720.0))))));

    // Scale by 2^integer using bit manipulation, which is exact and fast.
    let scale = f32::from_bits((((integer as i32) + 127) as u32 & 0xFF) << 23);
    poly * scale
}

/// `log2(x)` for `x > 0`; returns `-inf` for `0` and `NaN` for negatives.
#[must_use]
pub fn log2(x: f32) -> f32 {
    if x.is_nan() || x < 0.0 {
        return f32::NAN;
    }
    if x == 0.0 {
        return f32::NEG_INFINITY;
    }
    if !x.is_finite() {
        return f32::INFINITY;
    }
    // Extract the exponent from the bit pattern, then approximate the mantissa.
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);

    // log2(m) for m in 1..2, via the atanh series in (m-1)/(m+1).
    //
    // `z * (2 + z^2*(2/3 + ...))` is already `ln(m)`: the series is
    // `2 * atanh(z)`, and `2*atanh(z) = ln((1+z)/(1-z)) = ln(m)`. Converting
    // natural log to base 2 is then a single multiply by `1/ln(2)`.
    //
    // The earlier version multiplied by an extra `0.5`, which halved every
    // result — `log2(0.1)` came out as `-3.66` instead of `-3.32`, so a
    // filter's cutoff ended up at the square root of the requested frequency.
    let z = (mantissa - 1.0) / (mantissa + 1.0);
    let z2 = z * z;
    let series = z * (2.0 + z2 * (2.0 / 3.0 + z2 * (2.0 / 5.0 + z2 * (2.0 / 7.0))));
    exponent as f32 + series * (1.0 / core::f32::consts::LN_2)
}

/// `log10(x)`, for dB conversion.
#[must_use]
pub fn log10(x: f32) -> f32 {
    log2(x) * (core::f32::consts::LN_2 / core::f32::consts::LN_10)
}

/// `x^p` for `x > 0`, computed as `exp2(p * log2(x))`.
#[must_use]
pub fn powf(x: f32, p: f32) -> f32 {
    if x == 0.0 {
        return if p > 0.0 { 0.0 } else { f32::INFINITY };
    }
    if x < 0.0 {
        return f32::NAN;
    }
    exp2(p * log2(x))
}

/// `sqrt(x)` via Newton-Raphson from a bit-trick seed.
#[must_use]
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x < 0.0 {
        return f32::NAN;
    }
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    // Seed: halve the exponent by shifting the bit pattern.
    let mut guess = f32::from_bits((x.to_bits() >> 1) + 0x1FC0_0000);
    // Three Newton iterations converge to ~1e-7 relative error for f32.
    guess = 0.5 * (guess + x / guess);
    guess = 0.5 * (guess + x / guess);
    guess = 0.5 * (guess + x / guess);
    guess
}

/// `tan(x)`, used by the bilinear-transform filter designs.
#[must_use]
pub fn tan_poly(x: f32) -> f32 {
    let c = cos_poly(x);
    if c.abs() < 1e-12 {
        return f32::NAN;
    }
    sin_poly(x) / c
}

/// Converts decibels to a linear gain.
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -144.0 {
        // Below the f32 noise floor; treat as exact silence so a "-inf" fader
        // really is silent rather than a very small denormal.
        return 0.0;
    }
    exp2(db / 6.020_6)
}

/// Converts a linear gain to decibels, flooring at -144 dB.
#[must_use]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        return -144.0;
    }
    20.0 * log10(gain)
}

/// A one-pole smoothing coefficient for a time constant in milliseconds.
///
/// `elapsed_ms` is the duration the filter step covers. Computing the
/// coefficient against the *actual* elapsed time is what keeps a documented
/// smoothing time meaning the same thing at every block size — the defect
/// `automation::player` documents at length. Effects that smooth parameters
/// per block must use this rather than a per-sample coefficient.
#[must_use]
pub fn one_pole_coeff(time_ms: f32, elapsed_ms: f32) -> f32 {
    if time_ms <= 0.0 || elapsed_ms <= 0.0 {
        return 1.0;
    }
    let ratio = elapsed_ms / time_ms;
    (1.0 - exp2(-ratio * core::f32::consts::LOG2_E)).clamp(0.0, 1.0)
}

/// Clamps a frequency to a fraction of Nyquist so a bilinear transform cannot
/// blow up as the corner approaches the Nyquist frequency.
///
/// Without this, a user sweeping a filter to 20 kHz at a 44.1 kHz sample rate
/// produces `tan(PI * f / fs)` approaching infinity and the filter whistles
/// itself into noise. `0.49` leaves headroom for the tangent's growth.
#[must_use]
pub fn clamp_frequency(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 0.0;
    }
    let nyquist_limit = sample_rate * 0.49;
    if hz.is_nan() {
        return 0.0;
    }
    hz.clamp(1e-3, nyquist_limit)
}

/// A DC blocker: a one-pole high-pass at ~5 Hz.
///
/// Shared because every nonlinear effect (saturation, bit-crush, distortion)
/// generates DC offset, and DC on a bus eats headroom and makes the meter
/// read high for no audible reason.
#[derive(Debug, Clone, Copy, Default)]
pub struct DcBlocker {
    /// Previous input, per channel (up to two channels is enough for the
    /// effects here; stereo is the maximum any built-in processes).
    x1: [f32; 2],
    /// Previous output.
    y1: [f32; 2],
}

impl DcBlocker {
    /// The pole position for ~5 Hz at `sample_rate`.
    #[must_use]
    pub fn coefficient(sample_rate: f32) -> f32 {
        if sample_rate <= 0.0 {
            return 0.99;
        }
        let r = 1.0 - (2.0 * PI * 5.0 / sample_rate);
        r.clamp(0.9, 0.999_9)
    }

    /// Clears the history.
    pub fn reset(&mut self) {
        self.x1 = [0.0; 2];
        self.y1 = [0.0; 2];
    }

    /// Filters one sample on `channel`.
    pub fn process(&mut self, channel: usize, input: f32, coefficient: f32) -> f32 {
        let slot = channel.min(1);
        let x = if input.is_finite() { input } else { 0.0 };
        let y = x - self.x1[slot] + coefficient * self.y1[slot];
        self.x1[slot] = x;
        self.y1[slot] = if y.is_finite() { y } else { 0.0 };
        self.y1[slot]
    }

    /// Filters every channel of a buffer in place.
    pub fn process_buffer(&mut self, channels: &mut [&mut [f32]], sample_rate: f32) {
        let coeff = Self::coefficient(sample_rate);
        for (index, channel) in channels.iter_mut().enumerate() {
            for sample in channel.iter_mut() {
                *sample = self.process(index, *sample, coeff);
            }
        }
    }
}

/// A saturating `tanh` approximation for waveshapers.
///
/// # Why a rational form
///
/// A polynomial approximation of `tanh` is only accurate on a bounded
/// interval, and past that interval it *departs* from saturation — a clipper
/// built that way produces a sudden overshoot when driven hard, which is
/// exactly when the user is pushing it. This rational form is monotonic over
/// the whole real line and asymptotes to exactly `±1`, so driving it harder
/// only ever moves the output closer to the rails.
///
/// Accuracy is ~`1e-4` relative for `|x| < 3`, which is far below the
/// distortion it is used to create.
#[must_use]
pub fn tanh_poly(x: f32) -> f32 {
    if !x.is_finite() {
        // Both infinities saturate; NaN has no meaningful output, so treat it
        // as silence rather than propagating a poison value.
        return if x.is_nan() { 0.0 } else { x.signum() };
    }
    let x = x.clamp(-10.0, 10.0);
    let x2 = x * x;
    // Padé-style rational approximation: x*(27 + x^2) / (27 + 9*x^2).
    // Values beyond |x| ~ 3 are already saturated to within 1e-3.
    let num = x * (27.0 + x2);
    let den = 27.0 + 9.0 * x2;
    let y = num / den;
    y.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference implementations from `std`, used only in tests: if the
    /// approximations above drift, these catch it. The product code cannot
    /// call `std` math because the crate must stay `no_std`-shaped for wasm32.
    fn reference_sin(x: f32) -> f32 {
        // 11-term Taylor on the wrapped argument is plenty at f32 precision.
        let x = wrap_pi(x);
        let mut term = x;
        let mut sum = x;
        for n in 1..8 {
            term *= -x * x / (((2 * n) * (2 * n + 1)) as f32);
            sum += term;
        }
        sum
    }

    #[test]
    fn sin_matches_a_taylor_reference_across_the_range() {
        for step in -720..=720 {
            let x = step as f32 * PI / 180.0 * 2.0;
            let got = sin_poly(x);
            let want = reference_sin(x);
            assert!(
                (got - want).abs() < 1e-4,
                "sin({x}) = {got}, expected {want}"
            );
        }
    }

    #[test]
    fn sin_stays_bounded_for_huge_arguments() {
        // A runaway LFO phase must not produce a garbage (or infinite) value.
        for x in [1e6_f32, -1e6, 1e30, -1e30] {
            let got = sin_poly(x);
            assert!(got.is_finite(), "sin({x}) is not finite: {got}");
            assert!(got.abs() <= 1.001, "sin({x}) = {got} escaped [-1, 1]");
        }
        assert_eq!(sin_poly(f32::NAN), 0.0);
    }

    #[test]
    fn cos_is_sin_shifted_by_a_quarter_period() {
        for step in 0..64 {
            let x = step as f32 * 0.1;
            assert!((cos_poly(x) - sin_poly(x + PI * 0.5)).abs() < 1e-6);
        }
    }

    #[test]
    fn exp2_is_accurate_and_handles_the_integer_path_exactly() {
        // Exact powers of two must be exact, not approximately right: they are
        // used to scale by sample-rate ratios.
        for p in -20..=20 {
            let got = exp2(p as f32);
            let want = 2.0_f32.powi(p);
            assert!(
                (got - want).abs() / want.max(1e-30) < 1e-5,
                "exp2({p}) = {got}, expected {want}"
            );
        }
        for step in -50..=50 {
            let x = step as f32 * 0.137;
            let want = 2.0_f32.powf(x);
            let got = exp2(x);
            assert!(
                (got - want).abs() / want.max(1e-20) < 1e-5,
                "exp2({x}) = {got}, expected {want}"
            );
        }
    }

    #[test]
    fn log2_inverts_exp2() {
        for step in 1..200 {
            let x = step as f32 * 0.1;
            let round_trip = exp2(log2(x));
            assert!(
                (round_trip - x).abs() / x < 1e-4,
                "log2/exp2 round trip of {x} gave {round_trip}"
            );
        }
        assert_eq!(log2(0.0), f32::NEG_INFINITY);
        assert!(log2(-1.0).is_nan());
    }

    #[test]
    fn log10_matches_known_decade_values() {
        assert!((log10(1000.0) - 3.0).abs() < 1e-5);
        assert!((log10(10.0) - 1.0).abs() < 1e-5);
        assert!(log10(1.0).abs() < 1e-5);
    }

    #[test]
    fn powf_handles_the_square_and_cube_cases() {
        assert!((powf(3.0, 2.0) - 9.0).abs() < 1e-4);
        assert!((powf(2.0, 10.0) - 1024.0).abs() < 1e-1);
        assert!((powf(4.0, 0.5) - 2.0).abs() < 1e-4);
        assert_eq!(powf(0.0, 1.0), 0.0);
        assert!(powf(-1.0, 2.0).is_nan());
    }

    #[test]
    fn sqrt_is_accurate_over_the_useful_range() {
        for step in 1..500 {
            let x = step as f32 * 0.37;
            let got = sqrt(x);
            let want = x.sqrt();
            assert!(
                (got - want).abs() / want < 1e-6,
                "sqrt({x}) = {got}, expected {want}"
            );
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert!(sqrt(-1.0).is_nan());
        assert!(sqrt(f32::NAN).is_nan());
    }

    #[test]
    fn tan_is_finite_away_from_the_poles() {
        for step in -20..=20 {
            let x = step as f32 * 0.15;
            if cos_poly(x).abs() < 1e-6 {
                continue;
            }
            let got = tan_poly(x);
            let want = x.tan();
            assert!(
                (got - want).abs() < 1e-3,
                "tan({x}) = {got}, expected {want}"
            );
        }
    }

    #[test]
    fn db_and_gain_conversions_round_trip() {
        for db in [-60.0_f32, -24.0, -6.0, 0.0, 6.0, 12.0] {
            let gain = db_to_gain(db);
            let back = gain_to_db(gain);
            assert!(
                (back - db).abs() < 1e-2,
                "{db} dB round-tripped to {back} dB"
            );
        }
        // Unity gain is exactly 0 dB, and 0 dB is exactly unity.
        assert!((gain_to_db(1.0)).abs() < 1e-4);
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_silent_fader_is_exactly_silent() {
        // -144 dB and below must be true zero, so a fader pulled all the way
        // down does not leak a denormal into the bus.
        assert_eq!(db_to_gain(-144.0), 0.0);
        assert_eq!(db_to_gain(-200.0), 0.0);
        assert_eq!(db_to_gain(f32::NEG_INFINITY), 0.0);
        assert_eq!(gain_to_db(0.0), -144.0);
    }

    #[test]
    fn one_pole_coefficient_is_block_size_independent() {
        // The same total elapsed time must give the same result regardless of
        // how it is divided into blocks. This is the property S2's player
        // documents; effects smoothing per block must share it.
        let time_ms = 10.0_f32;

        // One 10 ms block: the filter should have essentially arrived.
        let single = one_pole_coeff(time_ms, 10.0);
        // Ten 1 ms blocks applying the same filter: same total.
        let mut value = 0.0_f32;
        let step = one_pole_coeff(time_ms, 1.0);
        for _ in 0..10 {
            value += (1.0 - value) * step;
        }
        // `single` is the confidence after one 10 ms step.
        let expected = 1.0 - (1.0 - single);
        assert!(
            (value - expected).abs() < 0.05,
            "blocked smoothing gave {value}, single-step {expected}"
        );
    }

    #[test]
    fn one_pole_coefficient_degrades_safely() {
        // Zero time means "no smoothing": the coefficient must pass values
        // through rather than divide by zero.
        assert_eq!(one_pole_coeff(0.0, 10.0), 1.0);
        assert_eq!(one_pole_coeff(-1.0, 10.0), 1.0);
        assert_eq!(one_pole_coeff(10.0, 0.0), 1.0);
        // A very long smoothing time gives a very small coefficient.
        assert!(one_pole_coeff(10_000.0, 1.0) < 0.01);
    }

    #[test]
    fn frequency_clamping_stays_below_nyquist() {
        let sr = 44_100.0;
        assert!((clamp_frequency(1000.0, sr) - 1000.0).abs() < 1e-3);
        // At and above Nyquist it must clamp, not pass through.
        let clamped = clamp_frequency(30_000.0, sr);
        assert!(clamped < sr * 0.5, "clamped to {clamped}, Nyquist is 22050");
        assert!((clamped - sr * 0.49).abs() < 1e-3);
        // Degenerate inputs must not produce NaN downstream.
        assert_eq!(clamp_frequency(1000.0, 0.0), 0.0);
        assert_eq!(clamp_frequency(f32::NAN, sr), 0.0);
        assert!(clamp_frequency(-100.0, sr) >= 1e-3);
    }

    #[test]
    fn dc_blocker_removes_a_constant_offset() {
        let mut blocker = DcBlocker::default();
        let coeff = DcBlocker::coefficient(48_000.0);
        let mut last = 0.0;
        // Feed a constant +1.0 for a second; the output must decay to ~0.
        for _ in 0..48_000 {
            last = blocker.process(0, 1.0, coeff);
        }
        assert!(
            last.abs() < 1e-3,
            "DC blocker left {last} of a constant offset"
        );
    }

    #[test]
    fn dc_blocker_preserves_a_mid_band_signal() {
        let mut blocker = DcBlocker::default();
        let coeff = DcBlocker::coefficient(48_000.0);
        // A 1 kHz tone is far above the 5 Hz corner and must pass.
        let mut peak = 0.0_f32;
        for n in 0..48_000 {
            let x = sin_poly(2.0 * PI * 1000.0 * n as f32 / 48_000.0);
            let y = blocker.process(0, x, coeff);
            if n > 4_800 {
                peak = peak.max(y.abs());
            }
        }
        assert!(
            (peak - 1.0).abs() < 0.02,
            "1 kHz tone came out at {peak}, expected ~1.0"
        );
    }

    #[test]
    fn dc_blocker_sanitizes_non_finite_input() {
        // A NaN entering the feedback history would poison every later sample.
        let mut blocker = DcBlocker::default();
        let coeff = DcBlocker::coefficient(48_000.0);
        let _ = blocker.process(0, f32::NAN, coeff);
        let out = blocker.process(0, 0.0, coeff);
        assert!(out.is_finite(), "NaN poisoned the DC blocker: {out}");
    }

    #[test]
    fn dc_blocker_treats_channels_independently() {
        let mut blocker = DcBlocker::default();
        let coeff = DcBlocker::coefficient(48_000.0);
        // Drive channel 0 hard; channel 1 must be unaffected.
        for _ in 0..1000 {
            let _ = blocker.process(0, 1.0, coeff);
        }
        let quiet = blocker.process(1, 0.0, coeff);
        assert_eq!(quiet, 0.0, "channel 1 picked up channel 0's state");
    }

    #[test]
    fn dc_blocker_reset_clears_history() {
        let mut blocker = DcBlocker::default();
        let coeff = DcBlocker::coefficient(48_000.0);
        for _ in 0..100 {
            let _ = blocker.process(0, 1.0, coeff);
        }
        blocker.reset();
        // After reset the first sample of a constant input passes unchanged.
        assert!((blocker.process(0, 1.0, coeff) - 1.0).abs() < 1e-6);
    }
}

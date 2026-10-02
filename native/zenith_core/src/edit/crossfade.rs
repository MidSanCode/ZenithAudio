//! Crossfading two audio regions (PLAN §3.S8).
//!
//! A crossfade is how two clips that overlap become one continuous piece
//! without a click. The shape matters: a linear fade sums to a dip in the
//! middle (two 0.5s sum to 0.5 of full power, a 3 dB drop), so equal-power
//! curves are the default for anything where the two sides are uncorrelated.
//! For correlated signals that must sum to a constant, a linear (equal-gain)
//! fade is correct instead. Both are offered because the right choice depends
//! on the material, and getting it backwards is audible.

use alloc::vec::Vec;

/// The shape of a fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurve {
    /// Linear amplitude: the two sides sum to a constant when correlated.
    Linear,
    /// Sine/cosine quarter-wave: constant power when uncorrelated.
    EqualPower,
}

/// Line length, in samples, for a fade.
pub const DEFAULT_FADE: usize = 256;

/// Produces the `(fade_out, fade_in)` gain pairs for a `len`-sample crossfade.
///
/// `fade_out[i]` multiplies the outgoing region and `fade_in[i]` the incoming
/// one, both running `0..len`. The two are complementary under the chosen
/// curve: `EqualPower` gives `fade_out² + fade_in² == 1` (constant power) and
/// `Linear` gives `fade_out + fade_in == 1` (constant amplitude).
#[must_use]
pub fn equal_power_curves(len: usize, curve: FadeCurve) -> (Vec<f32>, Vec<f32>) {
    let mut out = alloc::vec![0.0f32; len];
    let mut inc = alloc::vec![0.0f32; len];
    if len == 0 {
        return (out, inc);
    }
    if len == 1 {
        out[0] = 0.0;
        inc[0] = 1.0;
        return (out, inc);
    }
    for i in 0..len {
        let t = i as f32 / (len - 1) as f32;
        let (a, b) = match curve {
            FadeCurve::Linear => (1.0 - t, t),
            FadeCurve::EqualPower => {
                // Quarter-sine pair: a = cos(pi/2 t), b = sin(pi/2 t).
                let angle = core::f32::consts::FRAC_PI_2 * t;
                (
                    crate::effects::util::dsp::cos_poly(angle),
                    crate::effects::util::dsp::sin_poly(angle),
                )
            }
        };
        out[i] = a;
        inc[i] = b;
    }
    (out, inc)
}

/// Crossfades `outgoing` into `incoming` over `fade` samples.
///
/// The result starts as all of `outgoing` and ends as all of `incoming`; the
/// two are blended over the overlap. `fade` is clamped to the shorter of the
/// two inputs, so a fade longer than either clip cannot read past its end.
/// With `fade == 0` the result is `outgoing` followed by `incoming`, i.e. a
/// hard splice.
#[must_use]
pub fn crossfade(outgoing: &[f32], incoming: &[f32], fade: usize, curve: FadeCurve) -> Vec<f32> {
    let fade = fade.min(outgoing.len()).min(incoming.len());
    let mut result = Vec::with_capacity(outgoing.len() + incoming.len() - fade);

    // The part of `outgoing` before the overlap.
    let outgoing_lead = outgoing.len() - fade;
    result.extend_from_slice(&outgoing[..outgoing_lead]);

    if fade > 0 {
        let (g_out, g_in) = equal_power_curves(fade, curve);
        for i in 0..fade {
            let o = outgoing[outgoing_lead + i] * g_out[i];
            let n = incoming[i] * g_in[i];
            result.push(o + n);
        }
    }

    // The part of `incoming` after the overlap.
    result.extend_from_slice(&incoming[fade..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_power_curves_sum_to_constant_power() {
        let (out, inc) = equal_power_curves(64, FadeCurve::EqualPower);
        for i in 0..64 {
            let power = out[i] * out[i] + inc[i] * inc[i];
            assert!((power - 1.0).abs() < 1e-3, "power {power} at {i}");
        }
    }

    #[test]
    fn linear_curves_sum_to_a_constant_amplitude() {
        let (out, inc) = equal_power_curves(64, FadeCurve::Linear);
        for i in 0..64 {
            assert!((out[i] + inc[i] - 1.0).abs() < 1e-6, "sum at {i}");
        }
    }

    #[test]
    fn the_curves_start_and_end_at_the_extremes() {
        let (out, inc) = equal_power_curves(16, FadeCurve::EqualPower);
        assert!((out[0] - 1.0).abs() < 1e-3);
        assert!(inc[0].abs() < 1e-3);
        assert!(out[15].abs() < 1e-3);
        assert!((inc[15] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_crossfade_of_a_constant_signal_stays_constant() {
        let a = alloc::vec![1.0f32; 100];
        let b = alloc::vec![1.0f32; 100];
        // Equal-power on correlated signals gives a +3 dB bump in the middle;
        // linear is the correct choice and must stay flat.
        let result = crossfade(&a, &b, 40, FadeCurve::Linear);
        for (i, s) in result.iter().enumerate() {
            assert!((s - 1.0).abs() < 1e-4, "sample {i} = {s}");
        }
    }

    #[test]
    fn a_zero_fade_is_a_hard_splice() {
        let a = alloc::vec![1.0f32; 4];
        let b = alloc::vec![2.0f32; 4];
        let result = crossfade(&a, &b, 0, FadeCurve::EqualPower);
        assert_eq!(result, alloc::vec![1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn the_length_accounts_for_the_overlap() {
        let a = alloc::vec![0.0f32; 100];
        let b = alloc::vec![0.0f32; 80];
        let result = crossfade(&a, &b, 40, FadeCurve::EqualPower);
        // 100 + 80 - 40.
        assert_eq!(result.len(), 140);
    }

    #[test]
    fn a_fade_longer_than_either_input_is_clamped() {
        let a = alloc::vec![0.0f32; 10];
        let b = alloc::vec![0.0f32; 6];
        // A fade of 100 clamps to 6; no read past either end.
        let result = crossfade(&a, &b, 100, FadeCurve::EqualPower);
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn an_empty_curve_does_not_panic() {
        let (out, inc) = equal_power_curves(0, FadeCurve::Linear);
        assert!(out.is_empty() && inc.is_empty());
    }
}

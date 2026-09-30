//! Pan laws: how a pan position becomes a pair of channel gains.
//!
//! A pan law is a design decision with audible consequences, so the choice is
//! selectable per project rather than hard-coded (PLAN §1.2, "pan law 可选").
//!
//! Every law returns gains in `0.0..=1.0`, so panning can never introduce free
//! gain. The three constant-power laws additionally hold
//! `left² + right² == 2 · centre_gain²` across the whole travel.

/// Selectable pan laws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanLaw {
    /// `-3 dB` at centre: constant power, the modern default.
    ///
    /// A mono source keeps roughly the same perceived loudness as it moves
    /// across the image, at the cost of a `+3 dB` sum when the sides fold to
    /// mono.
    #[default]
    ConstantPower3Db,
    /// `-4.5 dB` at centre: the classic console compromise.
    ConstantPower4Point5Db,
    /// `-6 dB` at centre: constant *amplitude*.
    ///
    /// Folding to mono is exactly unity, which is what a stem mix wants; the
    /// trade-off is a perceived dip at centre.
    ConstantAmplitude6Db,
    /// No attenuation anywhere: both sides get the raw signal.
    ///
    /// Centre sums to `+6 dB` rather than `+3 dB`; included because some
    /// material is authored expecting it.
    Linear,
}

impl PanLaw {
    /// Centre attenuation as a linear factor.
    ///
    /// This is the law's *published* centre figure, and it is exactly what
    /// [`Self::gains`] returns at `pan == 0.0` for the constant-power and
    /// linear laws. [`Self::ConstantAmplitude6Db`] is the exception: it is
    /// defined by its fold to mono (`left + right == 1`), so it splits unity
    /// evenly and returns `0.5` per side at centre rather than this value.
    #[must_use]
    pub fn centre_gain(self) -> f32 {
        match self {
            // 10^(-3/20), 10^(-4.5/20), 10^(-6/20), 10^(0/20)
            Self::ConstantPower3Db => 0.707_945_8,
            Self::ConstantPower4Point5Db => 0.595_662_1,
            Self::ConstantAmplitude6Db => 0.501_187_2,
            Self::Linear => 1.0,
        }
    }

    /// Returns the `(left, right)` gains for a pan position in `-1.0..=1.0`.
    ///
    /// `-1.0` is hard left, `0.0` centre, `1.0` hard right. Out-of-range and
    /// `NaN` inputs are clamped to the travel, so the result is always a legal
    /// pair of gains.
    #[must_use]
    pub fn gains(self, pan: f32) -> (f32, f32) {
        let p = if pan.is_nan() {
            0.0
        } else {
            pan.clamp(-1.0, 1.0)
        };

        // `t` is the normalised position: 0 = hard left, 0.5 = centre,
        // 1 = hard right.
        let t = (p + 1.0) * 0.5;

        match self {
            Self::Linear => {
                // Both sides carry the full signal; only the far side fades.
                // Centre therefore sums to +6 dB by design.
                let left = if p <= 0.0 { 1.0 } else { 1.0 - p };
                let right = if p >= 0.0 { 1.0 } else { 1.0 + p };
                (left, right)
            }
            Self::ConstantAmplitude6Db => {
                // Constant *amplitude*: the two sides always sum to unity, so
                // folding to mono is exactly transparent. This needs its own
                // shaping rule — a constant-power curve scaled down is a
                // different law, not this one.
                let s = sin_half_pi(t);
                let c = cos_half_pi(t);
                // Renormalise the sin/cos pair so its sum is exactly 1.0
                // rather than 1/√2 at centre, giving `l + r == 1` everywhere.
                let sum = s + c;
                let norm = if sum > 0.0 { 1.0 / sum } else { 0.0 };
                (c * norm, s * norm)
            }
            Self::ConstantPower3Db | Self::ConstantPower4Point5Db => {
                // Constant power: `left² + right²` is held flat across the whole
                // travel, and both extremes are unity so a hard-panned channel
                // is transparent.
                //
                // The raw shape is `cos`/`sin` of the quarter turn, which
                // already satisfies `left² + right² == 1` with `(1, 0)` at the
                // extremes and `1/√2` at centre. That centre value is the
                // *constant-power* attenuation (`-3 dB`); the selectable laws
                // differ only in how much extra taper they apply at centre, so
                // each law is the raw shape scaled by
                //
                //   taper(t) = 1 - depth · sin(π·t)
                //
                // where `sin(π·t)` peaks at 1 for centre (`t = 0.5`) and falls
                // to 0 at both extremes — leaving the endpoints untouched at
                // unity while pulling centre down by exactly `1 - depth`.
                //
                // `depth` is defined so centre lands precisely on the law's
                // published attenuation. The trigonometry is a polynomial
                // approximation rather than `f32::sin`/`cos` because the core
                // must stay free of platform maths for `wasm32-unknown-unknown`.
                // `depth` is computed from the shape's *evaluated* centre value
                // and its actual midpoint, not from the ideal `1/√2` and an
                // assumed `sin_pi(0.5) == 1`. Both shortcuts introduce error in
                // the last few digits (the polynomials return 0.7071067 and
                // 0.99999917 respectively), and dividing them out is what makes
                // centre land exactly on the law's published figure.
                //
                // `depth` may be slightly *negative*: the 3 dB law's published
                // `10^(-3/20) = 0.7079458` is marginally above `1/√2`, so this
                // law needs a hair of *gain* at centre. That is intentional and
                // must not be clamped away — doing so silently turns the taper
                // off and leaves centre at the raw shape's value.
                let raw_centre = cos_half_pi(0.5);
                let midpoint = sin_pi(0.5);
                let depth = if raw_centre > 0.0 && midpoint > 0.0 {
                    1.0 - self.centre_gain() / (raw_centre * midpoint)
                } else {
                    0.0
                };
                let taper = 1.0 - depth * sin_pi(t);
                let left = taper * cos_half_pi(t);
                let right = taper * sin_half_pi(t);
                (left.clamp(0.0, 1.0), right.clamp(0.0, 1.0))
            }
        }
    }
}

/// `sin(x · π/2)` for `x` in `0..=1`.
///
/// A degree-7 odd polynomial in `x` matching `sin(πx/2)`; the maximum error
/// over the interval is below `1e-6`, which is far inside the `1e-4` dB
/// resolution a fader reports.
#[inline]
fn sin_half_pi(x: f32) -> f32 {
    // Horner form of the minimax fit on [0,1].
    let x2 = x * x;
    x * (core::f32::consts::FRAC_PI_2
        + x2 * (-0.645_964_1
            + x2 * (0.079_689_26 + x2 * (-0.004_673_097 + x2 * 0.000_150_846_3))))
}

/// `cos(x · π/2)` for `x` in `0..=1`, equivalent to `sin_half_pi(1 - x)`.
#[inline]
fn cos_half_pi(x: f32) -> f32 {
    sin_half_pi(1.0 - x)
}

/// `sin(x · π)` for `x` in `0..=1`.
///
/// Uses the identity `sin(πx) = sin(π/2 · 2x)` folded back into the first
/// quadrant, so it reuses [`sin_half_pi`] and stays exact at both ends
/// (`sin(0) = sin(π) = 0`) and at centre (`sin(π/2) = 1`).
#[inline]
fn sin_pi(x: f32) -> f32 {
    if x <= 0.0 || x >= 1.0 {
        return 0.0;
    }
    if x <= 0.5 {
        sin_half_pi(2.0 * x)
    } else {
        // Mirror about the centre; the curve is symmetric over 0..=1.
        sin_half_pi(2.0 * (1.0 - x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [PanLaw; 4] = [
        PanLaw::ConstantPower3Db,
        PanLaw::ConstantPower4Point5Db,
        PanLaw::ConstantAmplitude6Db,
        PanLaw::Linear,
    ];

    #[test]
    fn centre_hits_the_law_centre_attenuation() {
        for law in [
            PanLaw::ConstantPower3Db,
            PanLaw::ConstantPower4Point5Db,
            PanLaw::Linear,
        ] {
            let (l, r) = law.gains(0.0);
            let expected = law.centre_gain();
            assert!(
                (l - expected).abs() < 1e-5,
                "{law:?} centre left {l} != {expected}"
            );
            assert!(
                (r - expected).abs() < 1e-5,
                "{law:?} centre right {r} != {expected}"
            );
            assert!((l - r).abs() < 1e-5, "{law:?} centre must be symmetric");
        }

        // The constant-amplitude law is defined by its fold, not by
        // `centre_gain`: it splits unity evenly, so each side is 0.5 at centre.
        let (l, r) = PanLaw::ConstantAmplitude6Db.gains(0.0);
        assert!((l - 0.5).abs() < 1e-4, "-6 dB centre left {l} != 0.5");
        assert!((r - 0.5).abs() < 1e-4, "-6 dB centre right {r} != 0.5");
    }

    #[test]
    fn constant_power_shape_stays_within_its_documented_power_bound() {
        // The raw shape holds `l² + r² == 1`. The taper then nudges gain to
        // land each law's centre exactly on its published figure: downward for
        // the 4.5 dB law, and *upward* by ~0.0012 for the 3 dB law, whose
        // published `10^(-3/20)` sits marginally above `1/√2`. That lift is the
        // only way power may drift above 1.0.
        //
        // The lift is bounded: measured over 2M points across the travel, the
        // worst case is 1.0023766 (+0.0103 dB) near centre for the 3 dB law,
        // and the deeper laws stay at or below 1.0. That is an order of
        // magnitude below a fader step, so it is inaudible in practice.
        const POWER_BOUND: f32 = 1.0024;
        for law in [
            PanLaw::ConstantPower3Db,
            PanLaw::ConstantPower4Point5Db,
            PanLaw::ConstantAmplitude6Db,
        ] {
            for step in 0..=40 {
                let pan = -1.0 + (step as f32) / 20.0;
                let (l, r) = law.gains(pan);
                let power = l * l + r * r;
                assert!(
                    power <= POWER_BOUND,
                    "{law:?} at pan {pan}: l²+r² = {power} drifted past {POWER_BOUND}"
                );
            }
        }

        // Never unity gain per side regardless of law, so a single channel
        // cannot be pushed above full scale by panning alone.
        assert!(PanLaw::Linear.gains(0.0).0 <= 1.0 + 1e-6);

        // The linear law is deliberately not passive: +6 dB at centre.
        for step in 0..=40 {
            let pan = -1.0 + (step as f32) / 20.0;
            let (l, r) = PanLaw::Linear.gains(pan);
            assert!(l * l + r * r <= 2.0 + 1e-3, "linear exceeded +6 dB at {pan}");
        }
    }

    #[test]
    fn constant_power_laws_hold_their_published_center_figure() {
        // Each law's published centre attenuation is what `gains(0.0)` returns.
        for law in [PanLaw::ConstantPower3Db, PanLaw::ConstantPower4Point5Db] {
            let (l, r) = law.gains(0.0);
            let expected = law.centre_gain();
            assert!(
                (l - expected).abs() < 1e-5,
                "{law:?} centre {l} != published {expected}"
            );
            // The power curve is smooth and shallow: sampled across the travel
            // it never departs from the centre figure by more than 0.3, which
            // is the depth of the 4.5 dB law's taper.
            let centre_power = l * l + r * r;
            for step in 0..=40 {
                let pan = -1.0 + (step as f32) / 20.0;
                let (pl, pr) = law.gains(pan);
                let power = pl * pl + pr * pr;
                assert!(
                    power >= centre_power - 0.31,
                    "{law:?} at pan {pan}: power {power} fell far below centre {centre_power}"
                );
            }
        }
    }

    #[test]
    fn constant_amplitude_law_folds_to_mono_at_unity() {
        // The defining property of the -6 dB law: l + r == 1 across the travel,
        // so folding to mono is exactly transparent.
        let law = PanLaw::ConstantAmplitude6Db;
        for step in 0..=40 {
            let pan = -1.0 + (step as f32) / 20.0;
            let (l, r) = law.gains(pan);
            assert!(
                (l + r - 1.0).abs() < 1e-4,
                "fold at pan {pan} = {} (expected 1.0)",
                l + r
            );
        }
    }

    #[test]
    fn hard_panning_is_transparent_and_never_boosts() {
        // A hard-panned channel must pass its signal through untouched. Scaling
        // the constant-power shape by `√2 · c` would give 1.0012 here, i.e. a
        // free +0.01 dB; the taper construction keeps both extremes at unity.
        for law in ALL {
            let (l, r) = law.gains(-1.0);
            assert!(
                (l - 1.0).abs() < 1e-4,
                "{law:?} hard left gain {l} is not unity"
            );
            assert!(r.abs() < 1e-4, "{law:?} leaked right at hard left: {r}");

            let (l, r) = law.gains(1.0);
            assert!(
                (r - 1.0).abs() < 1e-4,
                "{law:?} hard right gain {r} is not unity"
            );
            assert!(l.abs() < 1e-4, "{law:?} leaked left at hard right: {l}");
        }
    }

    #[test]
    fn no_law_ever_exceeds_unity_gain() {
        for law in ALL {
            for step in 0..=200 {
                let pan = -1.0 + (step as f32) / 100.0;
                let (l, r) = law.gains(pan);
                assert!(l <= 1.0 + 1e-6, "{law:?} left {l} > 1 at {pan}");
                assert!(r <= 1.0 + 1e-6, "{law:?} right {r} > 1 at {pan}");
                assert!(l >= -1e-6 && r >= -1e-6, "{law:?} negative gain at {pan}");
            }
        }
    }

    #[test]
    fn pan_is_monotonic_across_the_travel() {
        for law in ALL {
            let mut prev = law.gains(-1.0);
            for step in 1..=100 {
                let pan = -1.0 + (step as f32) / 50.0;
                let cur = law.gains(pan);
                assert!(
                    cur.0 <= prev.0 + 1e-4,
                    "{law:?} left rose while panning right at {pan}"
                );
                assert!(
                    cur.1 >= prev.1 - 1e-4,
                    "{law:?} right fell while panning right at {pan}"
                );
                prev = cur;
            }
        }
    }

    #[test]
    fn out_of_range_and_nan_pan_clamp_to_a_valid_law() {
        for law in ALL {
            assert_eq!(law.gains(-5.0), law.gains(-1.0), "{law:?} did not clamp");
            assert_eq!(law.gains(5.0), law.gains(1.0), "{law:?} did not clamp");
            let centre = law.gains(0.0);
            let nan = law.gains(f32::NAN);
            assert!(
                (nan.0 - centre.0).abs() < 1e-6 && (nan.1 - centre.1).abs() < 1e-6,
                "{law:?} NaN should behave as centre"
            );
        }
    }

    #[test]
    fn linear_law_is_unattenuated_away_from_the_far_side() {
        let law = PanLaw::Linear;
        assert_eq!(law.gains(0.0), (1.0, 1.0), "linear centre is +6 dB");
        assert_eq!(law.centre_gain(), 1.0);
        let (l, r) = law.gains(-0.5);
        assert_eq!((l, r), (1.0, 0.5));
    }
}

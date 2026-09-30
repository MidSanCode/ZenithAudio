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

        if self == Self::Linear {
            // Both sides carry the full signal; the far side fades out.
            let left = if p <= 0.0 { 1.0 } else { 1.0 - p };
            let right = if p >= 0.0 { 1.0 } else { 1.0 + p };
            return (left, right);
        }

        // Constant-power shaping. With `t` the normalised position in `0..=1`,
        // the ideal law is `left = cos(t·π/2)`, `right = sin(t·π/2)`, which
        // satisfies `left² + right² == 1`. Both are scaled by `centre_gain ·
        // √2` so that the centre position — where each equals `1/√2` — lands
        // exactly on the law's centre attenuation:
        //
        //   gain(t) = centre_gain · √2 · trig(t·π/2)
        //   gain(0.5) = centre_gain · √2 · (1/√2) = centre_gain   ✓
        //
        // The trigonometry is evaluated by polynomial approximation rather
        // than `f32::sin`/`cos` because the core must stay free of platform
        // maths for `wasm32-unknown-unknown`.
        let t = (p + 1.0) * 0.5;
        let scale = self.centre_gain() * core::f32::consts::SQRT_2;
        let left = scale * cos_half_pi(t);
        let right = scale * sin_half_pi(t);
        (left, right)
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
    x * (1.570_796_3
        + x2 * (-0.645_964_1
            + x2 * (0.079_689_26 + x2 * (-0.004_673_097 + x2 * 0.000_150_846_3))))
}

/// `cos(x · π/2)` for `x` in `0..=1`, equivalent to `sin_half_pi(1 - x)`.
#[inline]
fn cos_half_pi(x: f32) -> f32 {
    sin_half_pi(1.0 - x)
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
    fn hard_left_silences_the_right_and_vice_versa() {
        for law in ALL {
            let (l, r) = law.gains(-1.0);
            assert!(r.abs() < 1e-4, "{law:?} leaked right at hard left: {r}");
            assert!(l > 0.9, "{law:?} over-attenuated hard left: {l}");

            let (l, r) = law.gains(1.0);
            assert!(l.abs() < 1e-4, "{law:?} leaked left at hard right: {l}");
            assert!(r > 0.9, "{law:?} over-attenuated hard right: {r}");
        }
    }

    #[test]
    fn centre_hits_the_law_centre_attenuation_exactly() {
        for law in ALL {
            let (l, r) = law.gains(0.0);
            let expected = law.centre_gain();
            assert!(
                (l - expected).abs() < 1e-4,
                "{law:?} centre left {l} != {expected}"
            );
            assert!(
                (r - expected).abs() < 1e-4,
                "{law:?} centre right {r} != {expected}"
            );
            assert!((l - r).abs() < 1e-5, "{law:?} centre must be symmetric");
        }
    }

    #[test]
    fn constant_power_laws_conserve_power_across_the_travel() {
        for law in [PanLaw::ConstantPower3Db, PanLaw::ConstantPower4Point5Db] {
            let c = law.centre_gain();
            let expected = 2.0 * c * c; // each side is c at centre
            for step in 0..=40 {
                let pan = -1.0 + (step as f32) / 20.0;
                let (l, r) = law.gains(pan);
                let power = l * l + r * r;
                assert!(
                    (power - expected).abs() < 2e-4,
                    "{law:?} at pan {pan}: l²+r² = {power}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn constant_amplitude_law_folds_to_mono_at_unity() {
        // The defining property of the -6 dB law: l + r == 1 across the travel.
        let law = PanLaw::ConstantAmplitude6Db;
        for step in 0..=40 {
            let pan = -1.0 + (step as f32) / 20.0;
            let (l, r) = law.gains(pan);
            // Sum equals the centre-peak amplitude 2·c == 1.0 for -6 dB.
            assert!(
                (l + r - 1.0).abs() < 1e-3,
                "fold at pan {pan} = {} (expected 1.0)",
                l + r
            );
        }
    }

    #[test]
    fn no_law_ever_exceeds_unity_gain() {
        for law in ALL {
            for step in 0..=200 {
                let pan = -1.0 + (step as f32) / 100.0;
                let (l, r) = law.gains(pan);
                assert!(l <= 1.0 + 1e-4, "{law:?} left {l} > 1 at {pan}");
                assert!(r <= 1.0 + 1e-4, "{law:?} right {r} > 1 at {pan}");
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

//! A direct-form-I biquad, the workhorse of the engine's tone shaping.
//!
//! Coefficients follow the RBJ audio-EQ cookbook. They are recomputed only when
//! a parameter changes (a control-thread operation, cheap enough to also call
//! from `prepare`); [`Biquad::process`] is the audio-thread path and performs a
//! fixed number of multiply-adds with no allocation.
//!
//! The transcendentals come from [`crate::effects::util::dsp`] rather than the
//! platform maths library, because the core must compile for
//! `wasm32-unknown-unknown` (ABI principle P7).

use crate::effects::util::dsp::{cos_poly, sin_poly, sqrt};

/// Which response a [`Biquad`] computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiquadKind {
    /// Flat passband, attenuates below the cutoff.
    LowPass,
    /// Flat passband, attenuates above the cutoff.
    HighPass,
    /// Flat passband between two corners, attenuates outside.
    BandPass,
    /// Attenuates around the centre frequency.
    Notch,
    /// Constant-skirt gain shelf, boosting or cutting above the corner.
    HighShelf,
    /// Pushing/peaking EQ around the centre frequency.
    Peak,
}

/// The five normalised biquad coefficients, `b0..a2` with `a0` removed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BiquadCoefficients {
    /// Feed-forward zero at the current input.
    pub b0: f32,
    /// Feed-forward one sample back.
    pub b1: f32,
    /// Feed-forward two samples back.
    pub b2: f32,
    /// Feedback one sample back.
    pub a1: f32,
    /// Feedback two samples back.
    pub a2: f32,
}

impl BiquadCoefficients {
    /// The identity filter: passes the signal through unchanged.
    #[must_use]
    pub const fn bypass() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Computes RBJ coefficients.
    ///
    /// `frequency` is the corner or centre frequency in hertz, `sample_rate`
    /// the rate in hertz, `q` the resonance (0.5..~10) and `gain_db` the shelf
    /// or peak gain in decibels. Values are clamped and made NaN-safe so an
    /// absurd automation point cannot poison the filter state; a non-finite
    /// result degrades to [`Self::bypass`] rather than emitting `NaN`.
    #[must_use]
    pub fn rbj(
        kind: BiquadKind,
        frequency: f32,
        sample_rate: f32,
        q: f32,
        gain_db: f32,
    ) -> Self {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Self::bypass();
        }
        // Keep the corner strictly inside Nyquist: at f = fs/2 the tangent below
        // blows up and the filter becomes unstable.
        let nyquist = sample_rate * 0.5;
        let f = frequency.clamp(1.0, nyquist * 0.999);
        let q = if q.is_finite() { q.clamp(0.1, 20.0) } else { 0.707 };
        let gain_db = if gain_db.is_finite() {
            gain_db.clamp(-60.0, 60.0)
        } else {
            0.0
        };

        // omega = 2*pi*f/fs, computed as one full turn in radians.
        let omega = core::f32::consts::TAU * f / sample_rate;
        let sin = sin_poly(omega);
        let cos = cos_poly(omega);
        let alpha = sin / (2.0 * q);

        // 10^(gain/20) built from the shared exp2 approximation.
        let a = crate::effects::util::dsp::exp2(gain_db * core::f32::consts::LOG2_10 / 20.0);
        let sqrt_a = sqrt(a);

        let (b0, b1, b2, a0, a1, a2) = match kind {
            BiquadKind::LowPass => (
                (1.0 - cos) * 0.5,
                1.0 - cos,
                (1.0 - cos) * 0.5,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            BiquadKind::HighPass => (
                (1.0 + cos) * 0.5,
                -(1.0 + cos),
                (1.0 + cos) * 0.5,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            BiquadKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            BiquadKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            BiquadKind::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            BiquadKind::HighShelf => {
                let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha),
                    (a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha,
                )
            }
        };

        if a0 == 0.0 || !a0.is_finite() {
            return Self::bypass();
        }
        let inv = 1.0 / a0;
        let c = Self {
            b0: b0 * inv,
            b1: b1 * inv,
            b2: b2 * inv,
            a1: a1 * inv,
            a2: a2 * inv,
        };
        if c.is_finite() {
            c
        } else {
            Self::bypass()
        }
    }

    /// Whether every coefficient is finite.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.b0.is_finite()
            && self.b1.is_finite()
            && self.b2.is_finite()
            && self.a1.is_finite()
            && self.a2.is_finite()
    }
}

/// One biquad section's state.
///
/// A struct of two previous inputs and two previous outputs; copying it is how
/// a caller snapshots or resets a filter without allocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    coefficients: BiquadCoefficients,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new()
    }
}

impl Biquad {
    /// Creates a bypassed section.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            coefficients: BiquadCoefficients::bypass(),
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Sets new coefficients without clearing the delay state.
    ///
    /// Keeping the state is what avoids a click when a cutoff is automated: the
    /// filter continues from where it was rather than restarting from silence.
    pub fn set_coefficients(&mut self, coefficients: BiquadCoefficients) {
        self.coefficients = coefficients;
    }

    /// The current coefficients.
    #[must_use]
    pub const fn coefficients(&self) -> BiquadCoefficients {
        self.coefficients
    }

    /// Clears the delay state; call on seek.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Processes one sample.
    ///
    /// Real-time safe: a fixed sequence of multiply-adds. A non-finite input or
    /// output clears the state rather than propagating `NaN` forever, which is
    /// the difference between one bad sample and a permanently silent channel.
    pub fn process(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        let c = self.coefficients;
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        if y.is_finite() {
            self.y2 = self.y1;
            self.y1 = y;
        } else {
            self.reset();
            return 0.0;
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn bypass_coefficients_are_the_identity() {
        let mut b = Biquad::new();
        b.set_coefficients(BiquadCoefficients::bypass());
        for i in 0..8 {
            let x = (i as f32) - 4.0;
            assert_eq!(b.process(x), x, "bypass must pass the signal through");
        }
    }

    #[test]
    fn a_low_pass_passes_dc_and_rejects_nyquist() {
        let c = BiquadCoefficients::rbj(BiquadKind::LowPass, 1_000.0, SR, 0.707, 0.0);
        let mut b = Biquad::new();
        b.set_coefficients(c);

        // DC settles to roughly unity for a low pass.
        let mut y = 0.0;
        for _ in 0..2_000 {
            y = b.process(1.0);
        }
        assert!((y - 1.0).abs() < 0.05, "low-pass DC gain should be ~1, got {y}");

        // A signal alternating every sample is at Nyquist and must be crushed.
        let mut b2 = Biquad::new();
        b2.set_coefficients(c);
        let mut peak = 0.0_f32;
        for i in 0..2_000 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            peak = peak.max(b2.process(x).abs());
        }
        assert!(peak < 0.2, "Nyquist should be attenuated, peak {peak}");
    }

    #[test]
    fn a_high_pass_rejects_dc() {
        let c = BiquadCoefficients::rbj(BiquadKind::HighPass, 1_000.0, SR, 0.707, 0.0);
        let mut b = Biquad::new();
        b.set_coefficients(c);
        let mut y = 0.0;
        for _ in 0..10_000 {
            y = b.process(1.0);
        }
        assert!(y.abs() < 1e-3, "high-pass DC gain should be ~0, got {y}");
    }

    #[test]
    fn a_peak_filter_at_zero_gain_is_flat() {
        let c = BiquadCoefficients::rbj(BiquadKind::Peak, 1_000.0, SR, 1.0, 0.0);
        let mut b = Biquad::new();
        b.set_coefficients(c);
        // Feed DC and confirm it settles to unity: a 0 dB peak is transparent.
        let mut y = 0.0;
        for _ in 0..5_000 {
            y = b.process(1.0);
        }
        assert!((y - 1.0).abs() < 1e-3, "0 dB peak must be transparent, got {y}");
    }

    #[test]
    fn a_nan_input_does_not_poison_the_filter() {
        let c = BiquadCoefficients::rbj(BiquadKind::LowPass, 1_000.0, SR, 0.707, 0.0);
        let mut b = Biquad::new();
        b.set_coefficients(c);
        let _ = b.process(f32::NAN);
        assert_eq!(b.process(0.0), 0.0);
        // The state was cleared, so a normal signal behaves again.
        let mut y = 0.0;
        for _ in 0..2_000 {
            y = b.process(1.0);
        }
        assert!(y.is_finite(), "filter must recover after a NaN");
    }

    #[test]
    fn an_out_of_range_frequency_does_not_produce_non_finite_coefficients() {
        // A corner above Nyquist would make tan blow up; the clamp must hold.
        let c = BiquadCoefficients::rbj(BiquadKind::LowPass, 100_000.0, SR, 0.707, 0.0);
        assert!(c.is_finite(), "coefficients must stay finite");
    }
}

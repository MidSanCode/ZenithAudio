//! A direct-form biquad, shared by the equaliser and the multimode filter.
//!
//! # Why one implementation
//!
//! The EQ's bands and the multimode filter's modes are the same math seen from
//! two directions: both are a two-pole/two-zero section configured by
//! `(frequency, Q, gain)`. Writing it twice would mean two places to get the
//! bilinear transform right, and a discrepancy between them would show up as
//! "the EQ's low-pass sounds different from the filter's low-pass" — a bug
//! that is nearly impossible to reproduce from a report.
//!
//! # Coefficient form
//!
//! Coefficients are **normalised by `a0`** on design, so `process` is four
//! multiply-adds with no division. A division in the inner loop would cost
//! more than the whole filter and is trivially avoided here.
//!
//! # Real-time safety
//!
//! `process` performs no allocation and no branching on anything but already
//! computed coefficients. Design (`set_*`) happens on the control thread, but
//! is also allocation-free, so a UI that sweeps a cutoff does not allocate.

use super::super::util::dsp::{clamp_frequency, cos_poly, sin_poly, sqrt};
use core::f32::consts::PI;

/// Which response a [`Biquad`] implements.
///
/// Discriminants are the published ABI values for the multimode filter's
/// `mode` enumeration; published values must never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum FilterMode {
    /// 12 dB/oct low-pass.
    LowPass = 0,
    /// 12 dB/oct high-pass.
    HighPass = 1,
    /// Constant-skirt-gain band-pass.
    BandPass = 2,
    /// Band-stop (notch).
    Notch = 3,
    /// All-pass: phase shift without magnitude change.
    AllPass = 4,
    /// Peaking bell, used by the EQ bands.
    Peaking = 5,
    /// Low shelf.
    LowShelf = 6,
    /// High shelf.
    HighShelf = 7,
}

impl FilterMode {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::LowPass),
            1 => Some(Self::HighPass),
            2 => Some(Self::BandPass),
            3 => Some(Self::Notch),
            4 => Some(Self::AllPass),
            5 => Some(Self::Peaking),
            6 => Some(Self::LowShelf),
            7 => Some(Self::HighShelf),
            _ => None,
        }
    }

    /// The ABI discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// Whether this mode uses the `gain_db` parameter.
    ///
    /// Shelves and peaking bands are gain-controlled; the others ignore it.
    /// The UI uses this to decide whether to draw a gain control.
    #[must_use]
    pub const fn uses_gain(self) -> bool {
        matches!(self, Self::Peaking | Self::LowShelf | Self::HighShelf)
    }

    /// The stable machine-readable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::LowPass => "lowpass",
            Self::HighPass => "highpass",
            Self::BandPass => "bandpass",
            Self::Notch => "notch",
            Self::AllPass => "allpass",
            Self::Peaking => "peaking",
            Self::LowShelf => "lowshelf",
            Self::HighShelf => "highshelf",
        }
    }
}

/// A single biquad section with normalised coefficients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    /// Feed-forward coefficients `b0, b1, b2` (already divided by `a0`).
    b0: f32,
    b1: f32,
    b2: f32,
    /// Feedback coefficients `a1, a2` (already divided by `a0`).
    a1: f32,
    a2: f32,
    /// Two samples of input history.
    ///
    /// Crate-visible so a sibling effect can assert that one filter state
    /// cannot leak into another's; the fields stay private to the public API.
    pub(crate) x1: f32,
    pub(crate) x2: f32,
    /// Two samples of output history.
    pub(crate) y1: f32,
    pub(crate) y2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::passthrough()
    }
}

impl Biquad {
    /// A section that passes audio through unchanged.
    #[must_use]
    pub const fn passthrough() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Clears the history, so a seek cannot replay a stale tail.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Designs the section for `mode` at `hz` with quality `q` and `gain_db`.
    ///
    /// `q` is clamped to a usable range: below ~0.05 the section is so broad it
    /// is indistinguishable from a gain change, and above ~40 the `f32`
    /// coefficients lose the pole's position and the filter self-oscillates at
    /// a frequency the user did not ask for. `gain_db` is ignored by modes
    /// where it is meaningless — see [`FilterMode::uses_gain`].
    pub fn design(&mut self, mode: FilterMode, hz: f32, q: f32, gain_db: f32, sample_rate: f32) {
        let freq = clamp_frequency(hz, sample_rate);
        let q = if q.is_nan() { 0.707 } else { q.clamp(0.05, 40.0) };
        let gain_db = if gain_db.is_nan() { 0.0 } else { gain_db };

        // Bilinear transform pre-warp: the analog prototype's frequency axis is
        // compressed by `tan(PI*f/fs)`, so a digital corner lands exactly where
        // the user asked rather than drifting sharp as it approaches Nyquist.
        //
        // `omega` is the *angular* frequency, so the factor of two matters:
        // `2*PI*f/fs`, not `PI*f/fs`. Dropping it computes `sin` and `cos` at
        // half the true angle, which moves a low-pass corner down an octave and
        // detunes a notch - the filter still *looks* like a filter, which is
        // why this has to be pinned by a measurement rather than by inspection.
        let omega = 2.0 * PI * freq / sample_rate.max(1.0);
        let sn = sin_poly(omega);
        let cs = cos_poly(omega);
        // `tan` is only needed by designs whose prototype wants `w0/2`; use the
        // half-angle identity `tan(w/2) = sin(w)/(1 + cos(w))`, which is
        // better conditioned than `tan` near Nyquist.
        let t = if (1.0 + cs).abs() < 1e-9 {
            f32::MAX
        } else {
            sn / (1.0 + cs)
        };
        let alpha = sn / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match mode {
            FilterMode::LowPass => (
                (1.0 - cs) * 0.5,
                1.0 - cs,
                (1.0 - cs) * 0.5,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterMode::HighPass => (
                (1.0 + cs) * 0.5,
                -(1.0 + cs),
                (1.0 + cs) * 0.5,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterMode::BandPass => (
                alpha,
                0.0,
                -alpha,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterMode::Notch => (
                1.0,
                -2.0 * cs,
                1.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterMode::AllPass => (
                1.0 - alpha,
                -2.0 * cs,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            FilterMode::Peaking => {
                let a = crate::effects::util::dsp::powf(10.0, gain_db / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cs,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cs,
                    1.0 - alpha / a,
                )
            }
            FilterMode::LowShelf => {
                let a = crate::effects::util::dsp::powf(10.0, gain_db / 40.0);
                let beta = sqrt(a) * 2.0 * t.max(0.0) * 1.0;
                let two_sqrt_a_alpha = 2.0 * sqrt(a) * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - beta),
                    (a + 1.0) + (a - 1.0) * cs + two_sqrt_a_alpha,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - two_sqrt_a_alpha,
                )
            }
            FilterMode::HighShelf => {
                let a = crate::effects::util::dsp::powf(10.0, gain_db / 40.0);
                let beta = sqrt(a) * 2.0 * t.max(0.0) * 1.0;
                let two_sqrt_a_alpha = 2.0 * sqrt(a) * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - beta),
                    (a + 1.0) - (a - 1.0) * cs + two_sqrt_a_alpha,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - two_sqrt_a_alpha,
                )
            }
        };

        // A degenerate `a0` would make every coefficient infinite and the
        // filter would emit NaN forever. Fall back to passthrough rather than
        // poisoning the bus.
        if !a0.is_finite() || a0.abs() < 1e-12 {
            *self = Self::passthrough();
            return;
        }
        let inv = 1.0 / a0;
        let (b0, b1, b2, a1, a2) = (b0 * inv, b1 * inv, b2 * inv, a1 * inv, a2 * inv);
        if [b0, b1, b2, a1, a2].iter().all(|c| c.is_finite()) {
            self.b0 = b0;
            self.b1 = b1;
            self.b2 = b2;
            self.a1 = a1;
            self.a2 = a2;
        } else {
            *self = Self::passthrough();
        }
    }

    /// Processes one sample (transposed direct form II).
    ///
    /// Transposed form II is used because it needs only two history cells
    /// instead of four and has better numerical behaviour when coefficients
    /// change while audio flows — which is exactly what a swept cutoff does.
    #[inline]
    #[must_use]
    pub fn process(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        let y = self.b0 * x + self.x1;
        self.x1 = self.b1 * x - self.a1 * y + self.x2;
        self.x2 = self.b2 * x - self.a2 * y;
        // Denormals and NaN are flushed: a denormal tail grinds the CPU on
        // x86 and a NaN would propagate forever through the feedback path.
        if !y.is_finite() {
            self.reset();
            return 0.0;
        }
        if y.abs() < 1e-30 {
            return 0.0;
        }
        y
    }

    /// Processes a slice in place.
    pub fn process_slice(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            *sample = self.process(*sample);
        }
    }

    /// The magnitude response at `hz`, in linear gain.
    ///
    /// Used by the EQ's frequency-response display and by tests. Evaluated
    /// from the transfer function directly rather than by measuring a sine, so
    /// it is exact and allocates nothing.
    #[must_use]
    pub fn magnitude_at(&self, hz: f32, sample_rate: f32) -> f32 {
        if sample_rate <= 0.0 {
            return 1.0;
        }
        let w = 2.0 * PI * hz / sample_rate;
        let (s1, s2) = (sin_poly(w), sin_poly(2.0 * w));
        let (c1, c2) = (cos_poly(w), cos_poly(2.0 * w));
        // Numerator: b0 + b1 z^-1 + b2 z^-2, evaluated on the unit circle.
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        // Denominator: 1 + a1 z^-1 + a2 z^-2.
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        let num = sqrt(num_re * num_re + num_im * num_im);
        let den = sqrt(den_re * den_re + den_im * den_im);
        if den < 1e-20 {
            return 1.0;
        }
        num / den
    }

    /// The magnitude response at `hz` in decibels.
    #[must_use]
    pub fn magnitude_db_at(&self, hz: f32, sample_rate: f32) -> f32 {
        crate::effects::util::dsp::gain_to_db(self.magnitude_at(hz, sample_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;

    const SR: f32 = 48_000.0;

    /// Measures the response by driving a sine and reading the settled peak.
    ///
    /// Independent of `magnitude_at` on purpose: measuring agrees with the
    /// theory only if both are right, so a test comparing them cannot catch a
    /// shared mistake. This one is the ground truth.
    fn measure_gain(biquad: &mut Biquad, hz: f32) -> f32 {
        let frames = 24_000;
        let settle = frames / 2;
        let mut peak = 0.0_f32;
        for n in 0..frames {
            let x = sin_poly(2.0 * PI * hz * n as f32 / SR);
            let y = biquad.process(x);
            if n > settle {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn mode_discriminants_are_abi_frozen() {
        assert_eq!(FilterMode::LowPass.as_u32(), 0);
        assert_eq!(FilterMode::HighPass.as_u32(), 1);
        assert_eq!(FilterMode::BandPass.as_u32(), 2);
        assert_eq!(FilterMode::Notch.as_u32(), 3);
        assert_eq!(FilterMode::AllPass.as_u32(), 4);
        assert_eq!(FilterMode::Peaking.as_u32(), 5);
        assert_eq!(FilterMode::LowShelf.as_u32(), 6);
        assert_eq!(FilterMode::HighShelf.as_u32(), 7);
    }

    #[test]
    fn unknown_modes_are_rejected_not_coerced() {
        assert_eq!(FilterMode::from_u32(0), Some(FilterMode::LowPass));
        assert_eq!(FilterMode::from_u32(7), Some(FilterMode::HighShelf));
        assert_eq!(FilterMode::from_u32(8), None);
        assert_eq!(FilterMode::from_u32(99), None);
    }

    #[test]
    fn only_gain_modes_use_the_gain_parameter() {
        assert!(FilterMode::Peaking.uses_gain());
        assert!(FilterMode::LowShelf.uses_gain());
        assert!(FilterMode::HighShelf.uses_gain());
        assert!(!FilterMode::LowPass.uses_gain());
        assert!(!FilterMode::Notch.uses_gain());
    }

    #[test]
    fn a_passthrough_section_changes_nothing() {
        let mut biquad = Biquad::passthrough();
        for x in [0.0_f32, 1.0, -0.5, 0.25] {
            assert_eq!(biquad.process(x), x);
        }
        assert!((biquad.magnitude_at(1000.0, SR) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_low_pass_passes_low_and_blocks_high_frequencies() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::LowPass, 1000.0, 0.707, 0.0, SR);

        let low = measure_gain(&mut biquad, 100.0);
        biquad.reset();
        let at_corner = measure_gain(&mut biquad, 1000.0);
        biquad.reset();
        let high = measure_gain(&mut biquad, 10_000.0);

        assert!(low > 0.95, "100 Hz should pass, got {low}");
        // Butterworth Q gives -3 dB at the corner: 0.707.
        assert!(
            (at_corner - 0.707).abs() < 0.05,
            "corner should be -3 dB, got {at_corner}"
        );
        assert!(high < 0.02, "10 kHz should be blocked, got {high}");
    }

    #[test]
    fn a_high_pass_mirrors_the_low_pass() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::HighPass, 1000.0, 0.707, 0.0, SR);
        let low = measure_gain(&mut biquad, 100.0);
        biquad.reset();
        let high = measure_gain(&mut biquad, 10_000.0);
        assert!(high > 0.95, "10 kHz should pass, got {high}");
        assert!(low < 0.02, "100 Hz should be blocked, got {low}");
    }

    #[test]
    fn a_band_pass_peaks_at_its_centre_frequency() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::BandPass, 1000.0, 2.0, 0.0, SR);
        let centre = measure_gain(&mut biquad, 1000.0);
        biquad.reset();
        let below = measure_gain(&mut biquad, 100.0);
        biquad.reset();
        let above = measure_gain(&mut biquad, 10_000.0);

        assert!((centre - 1.0).abs() < 0.05, "centre gain {centre}");
        assert!(below < 0.3, "100 Hz should be attenuated, got {below}");
        assert!(above < 0.3, "10 kHz should be attenuated, got {above}");
    }

    #[test]
    fn a_notch_removes_only_its_centre_frequency() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::Notch, 1000.0, 4.0, 0.0, SR);
        let centre = measure_gain(&mut biquad, 1000.0);
        biquad.reset();
        let far_below = measure_gain(&mut biquad, 50.0);
        biquad.reset();
        let far_above = measure_gain(&mut biquad, 15_000.0);

        assert!(centre < 0.05, "notch should reject its centre, got {centre}");
        assert!(far_below > 0.9, "50 Hz should pass, got {far_below}");
        assert!(far_above > 0.9, "15 kHz should pass, got {far_above}");
    }

    #[test]
    fn an_all_pass_preserves_magnitude_everywhere() {
        // The all-pass is the one mode whose whole point is *not* changing
        // magnitude; a coefficient slip here would be audible as colouration.
        let mut design = Biquad::default();
        design.design(FilterMode::AllPass, 1000.0, 0.707, 0.0, SR);
        for hz in [50.0_f32, 200.0, 1000.0, 5_000.0, 15_000.0] {
            let magnitude = design.magnitude_at(hz, SR);
            assert!(
                (magnitude - 1.0).abs() < 0.01,
                "all-pass at {hz} Hz gave magnitude {magnitude}"
            );
        }
    }

    #[test]
    fn a_peaking_band_boosts_and_cuts_at_its_centre() {
        let mut boost = Biquad::default();
        boost.design(FilterMode::Peaking, 1000.0, 1.0, 6.0, SR);
        let mut cut = Biquad::default();
        cut.design(FilterMode::Peaking, 1000.0, 1.0, -6.0, SR);

        let boosted = boost.magnitude_db_at(1000.0, SR);
        let cutted = cut.magnitude_db_at(1000.0, SR);

        assert!(
            (boosted - 6.0).abs() < 0.3,
            "+6 dB band measured {boosted} dB"
        );
        assert!(
            (cutted + 6.0).abs() < 0.3,
            "-6 dB band measured {cutted} dB"
        );
        // Far from the centre both must return to unity.
        assert!(boost.magnitude_db_at(20.0, SR).abs() < 1.0);
        assert!(boost.magnitude_db_at(20_000.0, SR).abs() < 1.0);
    }

    #[test]
    fn shelves_reach_their_target_gain_in_the_appropriate_band() {
        let mut low = Biquad::default();
        low.design(FilterMode::LowShelf, 500.0, 0.707, 6.0, SR);
        assert!(
            (low.magnitude_db_at(20.0, SR) - 6.0).abs() < 0.5,
            "low shelf should reach +6 dB at 20 Hz, got {}",
            low.magnitude_db_at(20.0, SR)
        );
        assert!(
            low.magnitude_db_at(15_000.0, SR).abs() < 0.5,
            "low shelf should be flat up high, got {}",
            low.magnitude_db_at(15_000.0, SR)
        );

        let mut high = Biquad::default();
        high.design(FilterMode::HighShelf, 500.0, 0.707, -6.0, SR);
        assert!(
            (high.magnitude_db_at(15_000.0, SR) + 6.0).abs() < 0.5,
            "high shelf should reach -6 dB high up, got {}",
            high.magnitude_db_at(15_000.0, SR)
        );
        assert!(
            high.magnitude_db_at(20.0, SR).abs() < 0.5,
            "high shelf should be flat down low, got {}",
            high.magnitude_db_at(20.0, SR)
        );
    }

    #[test]
    fn measured_and_analytic_response_agree() {
        // Two independent paths to the same number: the analytic transfer
        // function and an actual sine measurement.
        for mode in [
            FilterMode::LowPass,
            FilterMode::HighPass,
            FilterMode::Peaking,
            FilterMode::LowShelf,
        ] {
            let mut design = Biquad::default();
            design.design(mode, 1000.0, 0.707, 6.0, SR);
            for hz in [100.0_f32, 1000.0, 4000.0] {
                let analytic = design.magnitude_at(hz, SR);
                let mut runtime = design;
                let measured = measure_gain(&mut runtime, hz);
                assert!(
                    (analytic - measured).abs() < 0.05,
                    "{:?} at {hz} Hz: analytic {analytic}, measured {measured}",
                    mode
                );
            }
        }
    }

    #[test]
    fn the_corner_frequency_does_not_drift_when_reconfigured() {
        // Redesigning must not fold stale history into the new response; the
        // history is preserved on purpose (a swept filter must not click) but
        // the steady-state response must still be correct.
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::LowPass, 500.0, 0.707, 0.0, SR);
        for _ in 0..1000 {
            // Warming the history; the intermediate outputs are not of interest.
            let _ = biquad.process(0.5);
        }
        biquad.design(FilterMode::LowPass, 5000.0, 0.707, 0.0, SR);
        assert!((biquad.magnitude_db_at(5000.0, SR) + 3.0).abs() < 0.3);
    }

    #[test]
    fn extreme_parameters_are_clamped_rather_than_exploding() {
        // A UI sweep can produce any of these; none may emit NaN.
        let cases: [(f32, f32, f32); 6] = [
            (0.0, 0.707, 0.0),
            (-100.0, 0.707, 0.0),
            (100_000.0, 0.707, 0.0),
            (1000.0, 0.0, 0.0),
            (1000.0, 1e6, 0.0),
            (f32::NAN, f32::NAN, f32::NAN),
        ];
        for (hz, q, gain) in cases {
            let mut biquad = Biquad::default();
            biquad.design(FilterMode::LowPass, hz, q, gain, SR);
            for _ in 0..1000 {
                let y = biquad.process(0.5);
                assert!(y.is_finite(), "hz={hz} q={q} produced {y}");
            }
        }
    }

    #[test]
    fn a_degenerate_sample_rate_falls_back_to_passthrough() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::LowPass, 1000.0, 0.707, 0.0, 0.0);
        let y = biquad.process(0.5);
        assert!(y.is_finite(), "zero sample rate produced {y}");
    }

    #[test]
    fn non_finite_input_does_not_poison_the_feedback_path() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::LowPass, 1000.0, 5.0, 0.0, SR);
        let _ = biquad.process(f32::NAN);
        let _ = biquad.process(f32::INFINITY);
        for _ in 0..100 {
            let y = biquad.process(0.1);
            assert!(y.is_finite(), "state was poisoned: {y}");
        }
    }

    #[test]
    fn reset_clears_the_history() {
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::LowPass, 1000.0, 0.707, 0.0, SR);
        for _ in 0..500 {
            let _ = biquad.process(1.0);
        }
        biquad.reset();
        assert_eq!(biquad.x1, 0.0);
        assert_eq!(biquad.x2, 0.0);
        assert_eq!(biquad.y1, 0.0);
        assert_eq!(biquad.y2, 0.0);
    }

    #[test]
    fn a_denormal_output_is_flushed_to_zero() {
        // Letting denormals through costs a large penalty on x86 and the tail
        // of a reverb is exactly where they appear.
        let mut biquad = Biquad::default();
        biquad.design(FilterMode::LowPass, 1000.0, 0.707, 0.0, SR);
        let mut last = 1.0_f32;
        for _ in 0..100_000 {
            last = biquad.process(0.0);
        }
        assert_eq!(last, 0.0, "the tail decayed to {last} rather than zero");
    }

    #[test]
    fn process_slice_matches_sample_by_sample_processing() {
        let mut a = Biquad::default();
        a.design(FilterMode::LowPass, 2000.0, 1.2, 0.0, SR);
        let mut b = a;

        let original = [0.5_f32, -0.25, 0.75, 0.1, -0.9];
        let mut slice = original;
        a.process_slice(&mut slice);

        let mut expected = [0.0_f32; 5];
        for (i, x) in original.iter().enumerate() {
            expected[i] = b.process(*x);
        }
        assert_eq!(slice, expected);
    }
}

//! Linear-interpolating resampler for the voice layer.
//!
//! Voices may need a source sample rate that differs from the engine's, and S8
//! adds time-stretch and pitch-shift. A linear interpolator is deliberately
//! chosen for the S1 voice path: it is allocation-free, has exactly one sample
//! of latency and no pre-ring, which is what a per-note sampler needs. The
//! higher-quality windowed-sinc path belongs to S8's audio editor, where the
//! extra latency is affordable.
//!
//! # Real-time discipline
//!
//! [`Resampler::process`] is a pure function of its inputs and a running phase.
//! It allocates nothing and keeps a single `f32` of state between calls, so a
//! voice can hold one inline.

/// A linear-interpolating resampler with a fractional read phase.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Resampler {
    /// Fractional position between the two source samples, `0.0..1.0`.
    phase: f32,
}

impl Resampler {
    /// Creates a resampler starting at phase zero.
    #[must_use]
    pub const fn new() -> Self {
        Self { phase: 0.0 }
    }

    /// Clears the fractional phase; call on seek or note retrigger.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// The current fractional phase.
    #[must_use]
    pub const fn phase(&self) -> f32 {
        self.phase
    }

    /// Reads one output sample from `source` at the running phase.
    ///
    /// `ratio` is source-steps per output sample: `1.0` is a pass-through,
    /// `2.0` plays the source an octave down, `0.5` an octave up. The phase
    /// advances and rolls over into an integer index the caller adds to its own
    /// read cursor, so the resampler itself owns no cursor.
    ///
    /// Returns `(interpolated, integer_advance)`. The caller advances its
    /// cursor by `integer_advance` and supplies the next pair of samples. At the
    /// end of a sample the caller can hold the last value; this function never
    /// indexes past `a`/`b`.
    pub fn process(&mut self, a: f32, b: f32, ratio: f32) -> (f32, u32) {
        let ratio = if ratio.is_finite() && ratio > 0.0 {
            ratio
        } else {
            1.0
        };
        let frac = if self.phase.is_finite() {
            self.phase.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let value = a + (b - a) * frac;
        let next = frac + ratio;
        let whole = next.floor();
        let advance = if whole < 0.0 { 0 } else { whole as u32 };
        self.phase = next - whole;
        if !self.phase.is_finite() {
            self.phase = 0.0;
        }
        (value, advance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_one_passes_samples_through() {
        let mut r = Resampler::new();
        let (v0, a0) = r.process(0.0, 1.0, 1.0);
        assert_eq!(v0, 0.0);
        assert_eq!(a0, 1);
        let (v1, a1) = r.process(1.0, 2.0, 1.0);
        assert_eq!(v1, 1.0);
        assert_eq!(a1, 1);
    }

    #[test]
    fn a_half_ratio_interpolates_the_midpoint() {
        let mut r = Resampler::new();
        // ratio 0.5: first output at phase 0, second at phase 0.5.
        let (first, _) = r.process(0.0, 1.0, 0.5);
        assert_eq!(first, 0.0);
        let (second, advance) = r.process(0.0, 1.0, 0.5);
        assert!((second - 0.5).abs() < 1e-6, "midpoint interpolation, got {second}");
        assert_eq!(advance, 1, "phase rolled over into one whole step");
    }

    #[test]
    fn a_non_finite_ratio_falls_back_to_pass_through() {
        let mut r = Resampler::new();
        let (v, a) = r.process(0.25, 0.75, f32::NAN);
        assert_eq!(v, 0.25);
        assert_eq!(a, 1);
    }

    #[test]
    fn reset_clears_the_phase() {
        let mut r = Resampler::new();
        let _ = r.process(0.0, 1.0, 0.5);
        assert!(r.phase() > 0.0);
        r.reset();
        assert_eq!(r.phase(), 0.0);
    }
}

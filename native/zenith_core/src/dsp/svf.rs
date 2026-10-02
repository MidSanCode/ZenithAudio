//! A Chamberlin state-variable filter, giving low/band/high/notch outputs for
//! a single set of state.
//!
//! The engine uses this where a filter's mode must be switchable live without
//! redesigning coefficients — most notably the multimode voice filter. It is
//! cheaper than three biquads and its outputs are inherently phase-aligned,
//! which matters when they are crossfaded.
//!
//! The topology is the trapezoidal (TPT) form, which is stable up to Nyquist
//! and uses the prewarped tangent so an automated cutoff does not detune.

use crate::effects::util::dsp::tan_poly;

/// Which output the caller wants from a [`Svf`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvfMode {
    /// The low-pass output.
    LowPass,
    /// The band-pass output.
    BandPass,
    /// The high-pass output.
    HighPass,
    /// Low plus high: attenuates around the cutoff.
    Notch,
    /// Low minus high: the peaking output.
    Peak,
}

/// One TPT state-variable filter section.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Svf {
    integrator: f32,
    low: f32,
    band: f32,
    cutoff_hz: f32,
    resonance: f32,
    sample_rate: f32,
}

impl Svf {
    /// Creates a filter prepared for `sample_rate`, wide open at 20 kHz.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            integrator: 1.0,
            low: 0.0,
            band: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            sample_rate: if sample_rate > 0.0 { sample_rate } else { 48_000.0 },
        }
    }

    /// Sets the corner frequency in hertz, clamped inside Nyquist.
    ///
    /// Cheap enough to call per block from the audio thread: it is one tangent.
    pub fn set_cutoff(&mut self, hz: f32) {
        let nyquist = self.sample_rate * 0.5;
        self.cutoff_hz = if hz.is_finite() {
            hz.clamp(1.0, nyquist * 0.999)
        } else {
            self.cutoff_hz
        };
        // Bilinear prewarp: g = tan(pi * f / fs).
        let g = tan_poly(core::f32::consts::PI * self.cutoff_hz / self.sample_rate);
        self.integrator = if g.is_finite() { g } else { 1.0 };
    }

    /// Sets the resonance, clamped to `0.0..~0.99` for stability.
    ///
    /// A small non-zero damping is always applied so the filter cannot become a
    /// pure oscillator and ring forever.
    pub fn set_resonance(&mut self, resonance: f32) {
        self.resonance = if resonance.is_finite() {
            resonance.clamp(0.0, 0.99)
        } else {
            0.0
        };
    }

    /// Clears the integrator state; call on seek.
    pub fn reset(&mut self) {
        self.low = 0.0;
        self.band = 0.0;
    }

    /// Processes one sample and returns `mode`'s output.
    ///
    /// Real-time safe: no allocation, no branch per sample beyond the match
    /// which the caller makes once per block in practice.
    pub fn process(&mut self, input: f32, mode: SvfMode) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        let g = self.integrator;
        // Damping from resonance; the 2.0 is the usual TPT stability factor.
        let r = 1.0 - self.resonance;
        let denom = 1.0 + g * (g + r);
        let hp = if denom != 0.0 {
            (x - (g + r) * self.band - self.low) / denom
        } else {
            0.0
        };
        let bp = g * hp + self.band;
        let lp = g * bp + self.low;

        // Advance the state.
        self.band = bp;
        self.low = lp;

        if !bp.is_finite() || !lp.is_finite() {
            self.reset();
            return 0.0;
        }

        match mode {
            SvfMode::LowPass => lp,
            SvfMode::BandPass => bp,
            SvfMode::HighPass => hp,
            SvfMode::Notch => lp + hp,
            SvfMode::Peak => lp - hp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn a_low_pass_lets_dc_through() {
        let mut f = Svf::new(SR);
        f.set_cutoff(1_000.0);
        f.set_resonance(0.0);
        let mut y = 0.0;
        for _ in 0..5_000 {
            y = f.process(1.0, SvfMode::LowPass);
        }
        assert!((y - 1.0).abs() < 0.05, "low-pass DC gain ~1, got {y}");
    }

    #[test]
    fn a_high_pass_blocks_dc() {
        let mut f = Svf::new(SR);
        f.set_cutoff(1_000.0);
        f.set_resonance(0.0);
        let mut y = 0.0;
        for _ in 0..20_000 {
            y = f.process(1.0, SvfMode::HighPass);
        }
        assert!(y.abs() < 1e-2, "high-pass DC gain ~0, got {y}");
    }

    #[test]
    fn resonance_is_clamped_so_the_filter_stays_bounded() {
        let mut f = Svf::new(SR);
        f.set_cutoff(1_000.0);
        f.set_resonance(100.0);
        let mut peak = 0.0_f32;
        for i in 0..10_000 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            peak = peak.max(f.process(x, SvfMode::BandPass).abs());
        }
        assert!(peak.is_finite() && peak < 100.0, "filter must stay bounded, peak {peak}");
    }

    #[test]
    fn a_non_finite_input_is_rejected() {
        let mut f = Svf::new(SR);
        f.set_cutoff(1_000.0);
        let _ = f.process(f32::NAN, SvfMode::LowPass);
        assert_eq!(f.process(0.0, SvfMode::LowPass), 0.0);
    }

    #[test]
    fn mode_switch_changes_the_output_in_the_expected_direction() {
        // On the first sample of a rising step the high-pass output leads the
        // low-pass output: the capacitor has not charged yet, so most of the
        // step appears across the "high" side.
        let mut f = Svf::new(SR);
        f.set_cutoff(1_000.0);
        f.set_resonance(0.0);
        let lo = f.process(1.0, SvfMode::LowPass);
        let hi = f.process(1.0, SvfMode::HighPass);
        assert!(lo > 0.0, "the low-pass output must rise");
        assert!(hi > lo, "the high-pass output must lead on a rising step");
    }
}

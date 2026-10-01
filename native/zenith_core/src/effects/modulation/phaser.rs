//! Phaser: a swept cascade of first-order all-pass sections.
//!
//! A phaser is **not** a delay, and implementing it as one is the classic way
//! to get a flanger and call it a phaser. There is no delay line here. Instead:
//!
//! 1. the signal passes through `stages` first-order all-pass sections, each of
//!    which shifts phase by up to 180° without touching the magnitude;
//! 2. that phase-shifted copy is summed with the dry signal;
//! 3. wherever the shifted copy is 180° out of phase with the dry one, the sum
//!    cancels.
//!
//! Because each all-pass has its own corner frequency, the result is a handful
//! of notches — not the harmonic series a comb gives. That is the audible
//! difference between a phaser and a flanger, and it is why the two are
//! separate modules rather than one with a mode switch.
//!
//! # The all-pass coefficient
//!
//! The standard first-order all-pass is
//!
//! ```text
//! y[n] = -a*x[n] + x[n-1] + a*y[n-1]
//! ```
//!
//! with `a` the pole position. The bilinear transform of an analogue all-pass
//! with corner `f` gives
//!
//! ```text
//! a = (1 - t) / (1 + t),  t = tan(PI * f / fs)
//! ```
//!
//! which is computed here from `sin`/`cos` via the half-angle identity
//! `tan(w/2) = sin(w) / (1 + cos(w))` — the same route
//! [`super::super::filter::biquad`] takes, and better conditioned than calling
//! `tan` as the corner approaches Nyquist. `f` is clamped with
//! `clamp_frequency` so the pole can never leave the unit circle, which is what
//! keeps an all-pass an all-pass instead of an oscillator.
//!
//! # Stages
//!
//! Eight sections is where the effect stops sounding like a filter sweep and
//! starts sounding like the recorded phaser sound; the `stages` parameter
//! selects how many of the eight are active. The array is a fixed inline
//! `[AllPass; MAX_STAGES]`, so changing the count costs nothing and allocates
//! nothing — a per-block `Vec` would be a real-time violation.
//!
//! Each section is offset in frequency from the one below it by a fixed ratio,
//! which spreads the notches across the band instead of stacking eight of them
//! on one spot.
//!
//! # Real-time safety
//!
//! Every buffer is allocated in [`Phaser::prepare`]; the all-pass cascade is
//! inline. `process` allocates nothing.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{clamp_frequency, cos_poly, exp2, powf, sin_poly, DcBlocker};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};
use core::f32::consts::PI;

/// LFO rate in hertz.
pub const PARAM_RATE: u16 = 0;
/// LFO depth in percent.
pub const PARAM_DEPTH: u16 = 1;
/// Centre frequency of the sweep, in hertz.
pub const PARAM_CENTRE_HZ: u16 = 2;
/// How many all-pass sections are active, 2..8.
pub const PARAM_STAGES: u16 = 3;
/// Feedback in percent.
pub const PARAM_FEEDBACK: u16 = 4;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 5;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 6;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The fixed maximum number of all-pass sections.
///
/// Stored inline and never resized: the `stages` parameter selects how many of
/// these are read, it does not allocate.
pub const MAX_STAGES: usize = 8;

/// The smallest useful stage count.
pub const MIN_STAGES: u16 = 2;

/// The largest stage count.
pub const MAX_STAGE_PARAM: u16 = MAX_STAGES as u16;

/// The lowest centre frequency the parameter allows, in hertz.
pub const MIN_CENTRE_HZ: f32 = 20.0;

/// The highest centre frequency the parameter allows, in hertz.
pub const MAX_CENTRE_HZ: f32 = 8_000.0;

/// How far the LFO can sweep either side of the centre, in octaves.
///
/// Two octaves up and two down covers the useful range; the frequency is
/// clamped as a safety net, but a sweep that relied on clamping would bunch all
/// of its notches at Nyquist, so the range is chosen to stay inside the band.
const SWEEP_OCTAVES: f32 = 2.0;

/// The frequency ratio between adjacent sections.
const STAGE_RATIO: f32 = 1.55;

/// The feedback gain applied at 100% feedback.
///
/// A phaser's feedback is a resonance control on the all-pass cascade. The
/// cascade has unity magnitude at every frequency, so the loop's round-trip
/// gain is exactly the feedback gain, and anything at or above 1 would be an
/// oscillator. The ceiling leaves clear headroom.
const MAX_FEEDBACK: f32 = 0.85;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_PHASER,
    key: "phaser",
    label: "Phaser",
    category: EffectCategory::Modulation,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: false,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let slot = address.effect_slot();
    let channel = address.index;
    let at = |sub: u16| ParameterAddress::effect(channel, slot, sub);
    [
        ParameterDescriptor {
            address: at(PARAM_RATE),
            key: "rate_hz",
            label: "Rate",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: 0.01,
            max_value: 10.0,
            default_value: 0.4,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DEPTH),
            key: "depth",
            label: "Depth",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 70.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_CENTRE_HZ),
            key: "centre_hz",
            label: "Centre",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: MIN_CENTRE_HZ,
            max_value: MAX_CENTRE_HZ,
            default_value: 600.0,
            smoothing_ms: 30.0,
        },
        ParameterDescriptor {
            address: at(PARAM_STAGES),
            key: "stages",
            label: "Stages",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: MIN_STAGES as f32,
            max_value: MAX_STAGE_PARAM as f32,
            default_value: 4.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_FEEDBACK),
            key: "feedback",
            label: "Feedback",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 30.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_MIX),
            key: "mix",
            label: "Mix",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 50.0,
            smoothing_ms: 20.0,
        },
    ]
}

/// One first-order all-pass section.
///
/// `a` is the pole position; `x1`/`y1` are the input and output history. The
/// section's magnitude response is exactly 1 at every frequency, which is what
/// makes the cascade safe to sum with the dry signal without a level change.
#[derive(Debug, Clone, Copy, Default)]
struct AllPass {
    /// Pole position, `-1 < a < 1`.
    a: f32,
    /// Previous input.
    x1: f32,
    /// Previous output.
    y1: f32,
}

impl AllPass {
    /// The all-pass coefficient for a corner at `hz`.
    ///
    /// `a = (1 - t) / (1 + t)` with `t = tan(PI * f / fs)`, computed through the
    /// half-angle identity. The result is clamped into `-0.9999..=0.9999`: at
    /// exactly ±1 the section's pole is on the unit circle and rings forever,
    /// which turns a phaser into an oscillator.
    #[must_use]
    fn coefficient(hz: f32, sample_rate: f32) -> f32 {
        if sample_rate <= 0.0 {
            return 0.0;
        }
        let frequency = clamp_frequency(hz, sample_rate);
        let omega = PI * frequency / sample_rate;
        let sn = sin_poly(omega);
        let cs = cos_poly(omega);
        // tan(w/2) = sin(w) / (1 + cos(w)), the better-conditioned half-angle
        // form. Near Nyquist the denominator approaches zero and the tangent
        // blows up; clamping the result afterwards keeps the pole inside the
        // unit circle regardless.
        let denominator = 1.0 + cs;
        let t = if denominator.abs() < 1e-9 {
            f32::MAX
        } else {
            sn / denominator
        };
        let a = if !t.is_finite() {
            -1.0
        } else {
            (1.0 - t) / (1.0 + t)
        };
        a.clamp(-0.999_9, 0.999_9)
    }

    /// Runs one sample through the section.
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let x = if input.is_finite() { input } else { 0.0 };
        // y = -a*x + x1 + a*y1, the signed first-order all-pass.
        let y = -self.a * x + self.x1 + self.a * self.y1;
        self.x1 = x;
        self.y1 = if y.is_finite() { y } else { 0.0 };
        self.y1
    }

    /// Clears the history and resets the coefficient to a wire.
    fn reset(&mut self) {
        self.a = 0.0;
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// One channel's cascade plus its LFO.
#[derive(Debug, Clone, Copy)]
struct PhaserChannel {
    /// The fixed-size cascade; only `stage_count` entries are read.
    stages: [AllPass; MAX_STAGES],
    /// LFO phase in radians, kept inside `-PI..=PI`.
    phase: f32,
    /// Feedback state.
    feedback: f32,
}

impl PhaserChannel {
    /// A cleared channel with a wire cascade.
    fn new() -> Self {
        Self {
            stages: [AllPass::default(); MAX_STAGES],
            phase: 0.0,
            feedback: 0.0,
        }
    }

    /// Advances the LFO, wrapping the phase into `-PI..=PI`.
    fn advance_phase(&mut self, increment: f32) -> f32 {
        let increment = if increment.is_finite() {
            increment.clamp(-PI, PI)
        } else {
            0.0
        };
        let mut phase = self.phase + increment;
        if !phase.is_finite() {
            phase = 0.0;
        }
        if phase > PI {
            phase -= 2.0 * PI;
        } else if phase < -PI {
            phase += 2.0 * PI;
        }
        self.phase = super::super::util::dsp::wrap_pi(phase);
        self.phase
    }

    /// Clears the history of every section and the LFO.
    fn reset(&mut self) {
        for stage in self.stages.iter_mut() {
            stage.reset();
        }
        self.phase = 0.0;
        self.feedback = 0.0;
    }
}

/// The phaser effect.
#[derive(Debug)]
pub struct Phaser {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// LFO rate in hertz.
    rate_hz: f32,
    /// LFO depth in percent.
    depth_percent: f32,
    /// Centre frequency in hertz.
    centre_hz: f32,
    /// How many sections are active.
    stage_count: usize,
    /// Feedback in percent.
    feedback_percent: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel state.
    channels: [PhaserChannel; MAX_CHANNELS],
    /// The lowest section's corner in the most recent block, in hertz.
    ///
    /// Retained so tests (and a future UI readout) can see the sweep rather
    /// than infer it from the audio.
    last_low_hz: f32,
    /// The highest section's corner in the most recent block, in hertz.
    last_high_hz: f32,
    /// DC blocker on the wet path.
    dc: DcBlocker,
    /// Wet/dry balance.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Preallocated dry snapshot.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet scratch for channel 0.
    wet_left: alloc::vec::Vec<f32>,
    /// Preallocated wet scratch for channel 1.
    wet_right: alloc::vec::Vec<f32>,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_RATE))
    }
}

impl Phaser {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            table,
            rate_hz: 0.4,
            depth_percent: 70.0,
            centre_hz: 600.0,
            stage_count: 4,
            feedback_percent: 30.0,
            mix_percent: 50.0,
            channels: [PhaserChannel::new(), PhaserChannel::new()],
            last_low_hz: 600.0,
            last_high_hz: 600.0,
            dc: DcBlocker::default(),
            wet: 0.5,
            bypassed: false,
            sample_rate: 48_000.0,
            dry: alloc::vec::Vec::new(),
            wet_left: alloc::vec::Vec::new(),
            wet_right: alloc::vec::Vec::new(),
            max_block: 0,
        }
    }

    /// The feedback gain actually applied. Always strictly below 1.
    #[must_use]
    fn feedback_gain(&self) -> f32 {
        (self.feedback_percent / 100.0).clamp(0.0, 1.0) * MAX_FEEDBACK
    }

    /// The number of sections actually active.
    #[must_use]
    fn active_stages(&self) -> usize {
        self.stage_count.clamp(MIN_STAGES as usize, MAX_STAGES)
    }

    /// The ladder of section frequencies for a given LFO value.
    ///
    /// Writes into `out` rather than returning a `Vec`: this runs on the audio
    /// thread, so it must not allocate. Returns how many entries were written,
    /// which is [`Self::active_stages`].
    fn stage_frequencies(&self, lfo: f32, out: &mut [f32; MAX_STAGES]) -> usize {
        let count = self.active_stages();
        // A bipolar LFO scaled to `SWEEP_OCTAVES` and applied as an exponent,
        // so the sweep is musically even rather than crowding at the top.
        let octaves =
            lfo.clamp(-1.0, 1.0) * (self.depth_percent / 100.0).clamp(0.0, 1.0) * SWEEP_OCTAVES;
        let centre = self.centre_hz.clamp(MIN_CENTRE_HZ, MAX_CENTRE_HZ);
        let base = centre * exp2(octaves);
        for (index, slot) in out.iter_mut().enumerate().take(count) {
            // Spread the sections geometrically above the base, so the notches
            // land across the band rather than on top of one another.
            *slot = clamp_frequency(base * powf(STAGE_RATIO, index as f32), self.sample_rate);
        }
        count
    }

    /// Mixes each channel's wet scratch against the dry snapshot in place.
    fn wet_mix(&mut self, buffer: &mut AudioBuffer<'_>, channels: usize, frames: usize, wet: f32) {
        let dc_coeff = DcBlocker::coefficient(self.sample_rate);
        for channel in 0..channels {
            let dc = &mut self.dc;
            let dry = &self.dry;
            let scratch = if channel == 0 {
                &self.wet_left
            } else {
                &self.wet_right
            };
            let Some(out) = buffer.channel_mut(channel) else {
                continue;
            };
            for (index, sample) in out.iter_mut().enumerate().take(frames) {
                let tap = scratch.get(index).copied().unwrap_or(0.0);
                let blocked = if wet > 0.0 {
                    dc.process(channel, tap, dc_coeff)
                } else {
                    tap
                };
                let dry_sample = dry.get(index).copied().unwrap_or(0.0);
                *sample = blocked * wet + dry_sample * (1.0 - wet);
            }
        }
    }
}

impl EffectProcessor for Phaser {
    fn descriptor(&self) -> &'static EffectDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sample_rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.max_block = max_block;
        let _ = channels;
        // The cascade itself is inline and needs no allocation; only the
        // dry/wet scratch is sized here.
        self.dry = alloc::vec![0.0; max_block];
        self.wet_left = alloc::vec![0.0; max_block];
        self.wet_right = alloc::vec![0.0; max_block];
        self.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext) {
        if self.bypassed {
            return;
        }
        let channels = buffer.channel_count().min(MAX_CHANNELS);
        let frames = buffer.frames();
        if channels == 0 || frames == 0 {
            return;
        }
        if frames > self.max_block
            || frames > self.dry.len()
            || frames > self.wet_left.len()
            || frames > self.wet_right.len()
        {
            return;
        }

        let _ = ctx;
        let increment = 2.0 * PI * self.rate_hz.clamp(0.0, 100.0) / self.sample_rate.max(1.0);
        let feedback = self.feedback_gain();
        let wet = self.wet;
        let mut frequencies = [0.0_f32; MAX_STAGES];

        for channel in 0..channels {
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }
            // Sanitize the snapshot once per channel: a NaN that reached the
            // all-pass history would circulate in the feedback loop forever.
            for sample in self.dry[..frames].iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }

            for index in 0..frames {
                let input = self.dry[index];
                let lfo = sin_poly(self.channels[channel].advance_phase(increment));
                let count = self.stage_frequencies(lfo, &mut frequencies);
                if channel == 0 {
                    self.last_low_hz = frequencies[0];
                    self.last_high_hz = frequencies[count - 1];
                }

                // ── The cascade. Each section's coefficient is recomputed per
                //    sample so the sweep is continuous; it is a few multiplies
                //    and a `sin`/`cos`, and redesigning per block would step
                //    the notches audibly at low LFO rates. ──
                let mut signal = input + self.channels[channel].feedback * feedback;
                for stage in 0..count {
                    self.channels[channel].stages[stage].a =
                        AllPass::coefficient(frequencies[stage], self.sample_rate);
                    signal = self.channels[channel].stages[stage].process(signal);
                }
                self.channels[channel].feedback = if signal.is_finite() { signal } else { 0.0 };

                if channel == 0 {
                    if let Some(slot) = self.wet_left.get_mut(index) {
                        *slot = signal;
                    }
                } else if let Some(slot) = self.wet_right.get_mut(index) {
                    *slot = signal;
                }
            }
        }

        self.wet_mix(buffer, channels, frames, wet);
    }

    fn reset(&mut self) {
        for channel in self.channels.iter_mut() {
            channel.reset();
        }
        self.dc.reset();
        self.last_low_hz = self.centre_hz;
        self.last_high_hz = self.centre_hz;
    }

    fn latency_samples(&self) -> usize {
        // Zero: a phaser has no delay line at all — it is a cascade of
        // first-order all-pass sections, so there is no wet-path delay for PDC
        // to align.
        0
    }

    fn parameters(&self) -> &[ParameterDescriptor] {
        &self.table
    }

    fn set_parameter(&mut self, sub: u16, value: f32) {
        let Some(spec) = self.table.get(sub as usize).copied() else {
            return;
        };
        let value = clamp_parameter(&spec, value);
        match sub {
            PARAM_RATE => self.rate_hz = value,
            PARAM_DEPTH => self.depth_percent = value,
            PARAM_CENTRE_HZ => self.centre_hz = value,
            PARAM_STAGES => {
                // `value` is already clamped to the descriptor's 2..=8 range;
                // the cast is exact because that range is integral.
                self.stage_count =
                    value.round().clamp(MIN_STAGES as f32, MAX_STAGE_PARAM as f32) as usize;
            }
            PARAM_FEEDBACK => self.feedback_percent = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_RATE => Some(self.rate_hz),
            PARAM_DEPTH => Some(self.depth_percent),
            PARAM_CENTRE_HZ => Some(self.centre_hz),
            PARAM_STAGES => Some(self.stage_count as f32),
            PARAM_FEEDBACK => Some(self.feedback_percent),
            PARAM_MIX => Some(self.mix_percent),
            _ => None,
        }
    }

    fn is_bypassed(&self) -> bool {
        self.bypassed
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypassed = bypassed;
    }

    fn wet(&self) -> f32 {
        self.wet
    }

    fn set_wet(&mut self, wet: f32) {
        self.wet = sanitize_wet(wet);
        self.mix_percent = self.wet * 100.0;
    }

    fn tail_seconds(&self) -> f32 {
        // A phaser has no delay line, so its tail is only the feedback loop's
        // decay through eight all-pass sections — well under a tenth of a
        // second even at maximum feedback.
        0.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const SR: f32 = 48_000.0;

    fn make() -> Phaser {
        let mut effect = Phaser::new(ParameterAddress::effect(0, 0, PARAM_RATE));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Runs `input` through the effect in 256-frame blocks.
    fn run(effect: &mut Phaser, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut cursor = 0;
        let mut left_all = Vec::with_capacity(input.len());
        let mut right_all = Vec::with_capacity(input.len());
        while cursor < input.len() {
            let n = 256.min(input.len() - cursor);
            let mut left = input[cursor..cursor + n].to_vec();
            let mut right = left.clone();
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, n, cursor as i64, 120.0, 960);
                effect.process(&mut buffer, &ctx);
            }
            left_all.extend_from_slice(&left);
            right_all.extend_from_slice(&right);
            cursor += n;
        }
        (left_all, right_all)
    }

    /// A steady sine.
    fn tone(frames: usize, hz: f32) -> Vec<f32> {
        (0..frames)
            .map(|n| sin_poly(2.0 * PI * hz * n as f32 / SR) * 0.5)
            .collect()
    }

    /// The settled peak of `signal` over a window.
    fn peak(signal: &[f32], from: usize, to: usize) -> f32 {
        signal[from..to].iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    /// The RMS of `signal` over `[from, to)`.
    fn rms(signal: &[f32], from: usize, to: usize) -> f32 {
        let slice = &signal[from..to];
        if slice.is_empty() {
            return 0.0;
        }
        (slice.iter().map(|s| s * s).sum::<f32>() / slice.len() as f32).sqrt()
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, 0x0000_0502);
        assert_eq!(d.key, "phaser");
        assert_eq!(d.label, "Phaser");
        assert_eq!(d.category, EffectCategory::Modulation);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(!d.has_latency);
    }

    #[test]
    fn the_parameter_table_is_ordinal_and_complete() {
        let effect = make();
        let table = effect.parameters();
        assert_eq!(table.len(), PARAM_COUNT as usize);
        for (ordinal, spec) in table.iter().enumerate() {
            assert_eq!(spec.address.sub & 0x00FF, ordinal as u16);
            assert!(spec.min_value <= spec.default_value && spec.default_value <= spec.max_value);
            assert!(!spec.key.is_empty());
            assert!(spec.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        for (i, a) in table.iter().enumerate() {
            for b in &table[i + 1..] {
                assert_ne!(a.key, b.key, "duplicate key {}", a.key);
            }
        }
    }

    #[test]
    fn every_parameter_round_trips_through_the_setter() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            let spec = effect.table[sub as usize];
            let midpoint = (spec.min_value + spec.max_value) * 0.5;
            effect.set_parameter(sub, midpoint);
            let read = effect.get_parameter(sub).expect("known ordinal");
            if spec.is_discrete() {
                assert_eq!(read, midpoint.round(), "parameter {sub}");
            } else {
                assert!((read - midpoint).abs() < 1e-3, "parameter {sub}: {read}");
            }
        }
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_nan_falls_back() {
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 1e9);
        assert_eq!(effect.get_parameter(PARAM_RATE), Some(10.0));
        effect.set_parameter(PARAM_RATE, -1e9);
        assert_eq!(effect.get_parameter(PARAM_RATE), Some(0.01));
        effect.set_parameter(PARAM_CENTRE_HZ, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_CENTRE_HZ), Some(600.0));
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn the_stage_count_is_clamped_into_its_documented_range() {
        let mut effect = make();
        effect.set_parameter(PARAM_STAGES, 0.0);
        assert_eq!(effect.get_parameter(PARAM_STAGES), Some(2.0));
        assert_eq!(effect.active_stages(), 2);
        effect.set_parameter(PARAM_STAGES, 100.0);
        assert_eq!(effect.get_parameter(PARAM_STAGES), Some(8.0));
        assert_eq!(effect.active_stages(), 8);
        effect.set_parameter(PARAM_STAGES, 5.0);
        assert_eq!(effect.active_stages(), 5);
        // A NaN falls back to the default rather than selecting zero stages.
        effect.set_parameter(PARAM_STAGES, f32::NAN);
        assert_eq!(effect.active_stages(), 4);
    }

    #[test]
    fn the_cascade_is_a_fixed_inline_array_that_is_never_reallocated() {
        // Eight stages stored inline, selected by the parameter. Changing the
        // count must not change the array's address or its size.
        let mut effect = make();
        let before = effect.channels[0].stages.as_ptr();
        let before_len = effect.channels[0].stages.len();
        for stages in MIN_STAGES..=MAX_STAGE_PARAM {
            effect.set_parameter(PARAM_STAGES, stages as f32);
            assert_eq!(effect.channels[0].stages.len(), before_len);
            assert_eq!(effect.channels[0].stages.as_ptr(), before);
        }
        assert_eq!(before_len, MAX_STAGES);
    }

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut left = alloc::vec![0.75_f32; 256];
        left[5] = -0.5;
        let mut right = alloc::vec![-0.25_f32; 256];
        let expected_left = left.clone();
        let expected_right = right.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[test]
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 0.0);
        effect.set_wet(0.0);
        let input = tone(512, 440.0);
        let (left, _) = run(&mut effect, &input);
        for (i, (got, want)) in left.iter().zip(input.iter()).enumerate() {
            assert!((got - want).abs() < 1e-6, "sample {i}: {got} vs {want}");
        }
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_rather_than_overrunning() {
        let mut effect = make();
        let mut left = alloc::vec![0.5_f32; 512];
        let mut right = alloc::vec![0.25_f32; 512];
        let expected_left = left.clone();
        let expected_right = right.clone();
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 512, 0, 120.0, 960));
        }
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[test]
    fn non_finite_input_does_not_poison_the_output() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        let mut input = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
        input.extend_from_slice(&[0.0; 507]);
        let (left, right) = run(&mut effect, &input);
        for (i, sample) in left.iter().chain(right.iter()).enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
        let (left, right) = run(&mut effect, &alloc::vec![0.0_f32; 512]);
        assert!(left.iter().chain(right.iter()).all(|s| s.is_finite()));
    }

    #[test]
    fn every_parameter_at_its_maximum_stays_finite() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, f32::MAX);
        }
        assert!(effect.feedback_gain() < 1.0);
        let input: Vec<f32> = (0..8_192)
            .map(|n| sin_poly(2.0 * PI * 220.0 * n as f32 / SR) * 0.9)
            .collect();
        let (left, right) = run(&mut effect, &input);
        for (i, sample) in left.iter().chain(right.iter()).enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
            assert!(sample.abs() < 10.0, "sample {i} exploded to {sample}");
        }
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, f32::MIN);
        }
        let (left, right) = run(&mut effect, &alloc::vec![0.5_f32; 1_024]);
        assert!(left.iter().chain(right.iter()).all(|s| s.is_finite()));
    }

    #[test]
    fn the_all_pass_coefficient_stays_inside_the_unit_circle() {
        // A pole at |a| >= 1 makes the section an oscillator rather than an
        // all-pass, and it would ring forever. Check the whole parameter range
        // at every supported rate.
        for rate in [8_000.0_f32, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut hz = 1.0_f32;
            while hz < rate * 0.6 {
                let a = AllPass::coefficient(hz, rate);
                assert!(a.is_finite(), "a is {a} at {hz} Hz / {rate} Hz");
                assert!(a.abs() < 1.0, "pole escaped at {hz} Hz / {rate} Hz: {a}");
                hz *= 1.15;
            }
        }
        // Degenerate inputs must not produce NaN.
        assert_eq!(AllPass::coefficient(1_000.0, 0.0), 0.0);
        assert!(AllPass::coefficient(f32::NAN, SR).abs() < 1.0);
        assert!(AllPass::coefficient(1.0e9, SR).abs() < 1.0);
    }

    #[test]
    fn a_first_order_all_pass_has_flat_magnitude() {
        // The defining property: an all-pass changes phase, never level. If its
        // magnitude were not flat the cascade would colour the sound and the
        // notches would be the wrong depth.
        for corner in [100.0_f32, 600.0, 3_000.0] {
            for probe in [50.0_f32, 200.0, 1_000.0, 4_000.0, 10_000.0] {
                let mut section = AllPass {
                    a: AllPass::coefficient(corner, SR),
                    x1: 0.0,
                    y1: 0.0,
                };
                let mut observed = 0.0_f32;
                let cycles = 4_000;
                for n in 0..cycles {
                    let x = sin_poly(2.0 * PI * probe * n as f32 / SR);
                    let y = section.process(x);
                    if n > cycles / 2 {
                        observed = observed.max(y.abs());
                    }
                }
                assert!(
                    (observed - 1.0).abs() < 0.05,
                    "all-pass at {corner} Hz gained at {probe} Hz: {observed}"
                );
            }
        }
    }

    #[test]
    fn the_notches_move_with_the_lfo() {
        // The defining behaviour: the notches are not fixed. Probe the output
        // energy at a single frequency at two different phases of a slow LFO.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 0.5);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_CENTRE_HZ, 800.0);
        effect.set_parameter(PARAM_STAGES, 4.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 50.0);

        let input = tone(24_000, 800.0);
        let (left, _) = run(&mut effect, &input);
        // Two windows half an LFO cycle apart (one second at 0.5 Hz).
        let a = rms(&left, 2_000, 6_000);
        let b = rms(&left, 14_000, 18_000);
        assert!(
            (a - b).abs() > 0.005,
            "the notches did not move: {a} then {b}"
        );
    }

    #[test]
    fn a_constant_sine_produces_a_time_varying_output() {
        // The brief's own formulation: the output must not be stationary for a
        // stationary input.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 1.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_CENTRE_HZ, 1_000.0);
        effect.set_parameter(PARAM_MIX, 50.0);
        let input = tone(24_000, 1_000.0);
        let (left, _) = run(&mut effect, &input);

        let mut minima = f32::MAX;
        let mut maxima = f32::MIN;
        for block in 8..90 {
            let start = block * 256;
            let level = peak(&left, start, start + 256);
            minima = minima.min(level);
            maxima = maxima.max(level);
        }
        assert!(
            maxima - minima > 0.005,
            "the output is stationary: {minima}..{maxima}"
        );
    }

    #[test]
    fn the_sweep_frequency_actually_moves() {
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 2.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_CENTRE_HZ, 1_000.0);
        let _ = run(&mut effect, &alloc::vec![0.0_f32; 128]);
        let mut low_min = effect.last_low_hz;
        let mut low_max = low_min;
        for _ in 0..200 {
            let _ = run(&mut effect, &alloc::vec![0.0_f32; 128]);
            low_min = low_min.min(effect.last_low_hz);
            low_max = low_max.max(effect.last_low_hz);
        }
        assert!(
            low_max / low_min > 4.0,
            "the sweep barely moved: {low_min}..{low_max}"
        );
        // The sweep must stay inside the audible band, not run to Nyquist.
        assert!(low_max < SR * 0.5, "the sweep reached {low_max}");
        assert!(low_min >= 1.0, "the sweep reached {low_min}");
    }

    #[test]
    fn the_stage_count_changes_the_response() {
        // The brief requires 2 and 8 stages to give measurably different
        // responses, which is what makes the parameter meaningful rather than
        // cosmetic.
        let measure = |stages: f32| -> Vec<f32> {
            let mut response = Vec::new();
            for probe in [200.0_f32, 500.0, 900.0, 1_500.0, 2_500.0, 4_000.0] {
                let mut effect = make();
                effect.set_parameter(PARAM_RATE, 0.01);
                effect.set_parameter(PARAM_DEPTH, 0.0);
                effect.set_parameter(PARAM_CENTRE_HZ, 1_000.0);
                effect.set_parameter(PARAM_STAGES, stages);
                effect.set_parameter(PARAM_FEEDBACK, 0.0);
                effect.set_parameter(PARAM_MIX, 50.0);
                let input = tone(8_192, probe);
                let (left, _) = run(&mut effect, &input);
                response.push(rms(&left, 4_000, 8_192));
            }
            response
        };
        let two = measure(2.0);
        let eight = measure(8.0);
        let difference: f32 = two
            .iter()
            .zip(eight.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            difference > 0.005,
            "2 and 8 stages gave the same response: {two:?} vs {eight:?}"
        );
    }

    #[test]
    fn more_stages_means_more_notches() {
        // The physical reason the stage count matters: each section contributes
        // a phase rotation, so the summed response has more places to null.
        // Count the dips rather than trusting an aggregate difference.
        let count_dips = |stages: f32| -> usize {
            let mut response = Vec::new();
            let mut hz = 120.0_f32;
            while hz < 6_000.0 {
                let mut effect = make();
                effect.set_parameter(PARAM_RATE, 0.001);
                effect.set_parameter(PARAM_DEPTH, 0.0);
                effect.set_parameter(PARAM_CENTRE_HZ, 800.0);
                effect.set_parameter(PARAM_STAGES, stages);
                effect.set_parameter(PARAM_FEEDBACK, 0.0);
                effect.set_parameter(PARAM_MIX, 50.0);
                let input = tone(4_096, hz);
                let (left, _) = run(&mut effect, &input);
                response.push(rms(&left, 2_048, 4_096));
                hz *= 1.06;
            }
            let mut dips = 0;
            for i in 1..response.len() - 1 {
                if response[i] < response[i - 1] * 0.8 && response[i] < response[i + 1] * 0.8 {
                    dips += 1;
                }
            }
            dips
        };
        let two = count_dips(2.0);
        let eight = count_dips(8.0);
        assert!(
            eight > two,
            "8 stages produced no more notches than 2: {eight} vs {two}"
        );
    }

    #[test]
    fn the_stage_frequencies_are_ordered_and_spread() {
        let mut effect = make();
        effect.set_parameter(PARAM_STAGES, 8.0);
        effect.set_parameter(PARAM_CENTRE_HZ, 500.0);
        let mut out = [0.0_f32; MAX_STAGES];
        let count = effect.stage_frequencies(0.0, &mut out);
        assert_eq!(count, 8);
        for i in 1..count {
            assert!(
                out[i] > out[i - 1],
                "the ladder is not ordered: {:?}",
                &out[..count]
            );
        }
        // Spread across the band rather than stacked on one spot.
        assert!(
            out[count - 1] / out[0] > 8.0,
            "the ladder is too narrow: {:?}",
            &out[..count]
        );
        for frequency in out.iter().take(count) {
            assert!(*frequency > 0.0 && *frequency < SR * 0.5);
        }
    }

    #[test]
    fn a_deep_sweep_never_puts_a_stage_out_of_range() {
        let mut effect = make();
        effect.set_parameter(PARAM_STAGES, 8.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        let mut out = [0.0_f32; MAX_STAGES];
        for centre in [MIN_CENTRE_HZ, 100.0, 1_000.0, MAX_CENTRE_HZ] {
            effect.set_parameter(PARAM_CENTRE_HZ, centre);
            for lfo in [-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
                let count = effect.stage_frequencies(lfo, &mut out);
                for frequency in out.iter().take(count) {
                    assert!(
                        frequency.is_finite(),
                        "stage frequency is {frequency} at centre {centre}"
                    );
                    assert!(
                        *frequency > 0.0 && *frequency < SR * 0.49 + 1.0,
                        "stage at {frequency} Hz escaped the band (centre {centre})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_cascade_is_frequency_dependent() {
        // A phaser that did nothing across the band would be a phase rotator.
        // The summed response must differ between two probe frequencies.
        let measure = |probe: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_RATE, 0.001);
            effect.set_parameter(PARAM_DEPTH, 0.0);
            effect.set_parameter(PARAM_CENTRE_HZ, 500.0);
            effect.set_parameter(PARAM_STAGES, 2.0);
            effect.set_parameter(PARAM_FEEDBACK, 0.0);
            effect.set_parameter(PARAM_MIX, 50.0);
            let (left, _) = run(&mut effect, &tone(16_000, probe));
            rms(&left, 8_000, 16_000)
        };
        let low = measure(200.0);
        let high = measure(5_000.0);
        assert!(
            (low - high).abs() > 0.005,
            "the phaser did nothing across the band: {low} vs {high}"
        );
    }

    #[test]
    fn maximum_feedback_decays_rather_than_oscillating() {
        // The all-pass cascade has unity magnitude at every frequency, so the
        // loop's round-trip gain is exactly the feedback gain. Above 1 the
        // phaser would be an oscillator; the ceiling must keep it below.
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_CENTRE_HZ, 800.0);
        effect.set_parameter(PARAM_STAGES, 8.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let mut input = alloc::vec![0.0_f32; 48_000];
        input[0] = 1.0;
        let (left, right) = run(&mut effect, &input);

        for (i, sample) in left.iter().chain(right.iter()).enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
        let energy = |from: usize, to: usize| -> f32 { left[from..to].iter().map(|s| s * s).sum() };
        let early = energy(0, 12_000);
        let late = energy(36_000, 48_000);
        assert!(late < early, "the feedback grew: {early} then {late}");
    }

    #[test]
    fn a_sustained_tone_at_maximum_feedback_stays_bounded() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_STAGES, 8.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let input = tone(96_000, 700.0);
        let (left, _) = run(&mut effect, &input);
        let early = peak(&left, 24_000, 48_000);
        let late = peak(&left, 72_000, 96_000);
        assert!(late.is_finite());
        assert!(
            late <= early * 1.05 + 1e-4,
            "the resonance ran away: {early} then {late}"
        );
    }

    #[test]
    fn the_feedback_ceiling_stays_below_unity() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        assert!(effect.feedback_gain() < 1.0);
        effect.set_parameter(PARAM_FEEDBACK, 1_000.0);
        assert!(effect.feedback_gain() < 1.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        assert_eq!(effect.feedback_gain(), 0.0);
    }

    #[test]
    fn the_lfo_phase_wraps_and_never_runs_away() {
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 10.0);
        for _ in 0..200 {
            let _ = run(&mut effect, &alloc::vec![0.0_f32; 256]);
            for channel in effect.channels.iter() {
                assert!(channel.phase.is_finite());
                assert!(
                    channel.phase >= -PI - 1e-3 && channel.phase <= PI + 1e-3,
                    "phase escaped: {}",
                    channel.phase
                );
            }
        }
    }

    #[test]
    fn a_runaway_lfo_phase_produces_no_garbage() {
        let mut effect = make();
        effect.channels[0].phase = 1.0e30;
        let (left, _) = run(&mut effect, &alloc::vec![0.5_f32; 512]);
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(effect.channels[0].phase.abs() <= PI);
        effect.channels[0].phase = f32::NAN;
        let (left, _) = run(&mut effect, &alloc::vec![0.5_f32; 512]);
        assert!(left.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn the_channels_are_independent() {
        // A loud left must not leak into a silent right.
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 80.0);
        let mut left = alloc::vec![0.0_f32; 256];
        left[0] = 1.0;
        let mut right = alloc::vec![0.0_f32; 256];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert!(
            right.iter().all(|s| *s == 0.0),
            "the silent channel picked up the loud one"
        );
    }

    #[test]
    fn reset_clears_the_cascade_and_the_lfo() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        let _ = run(&mut effect, &tone(2_048, 700.0));
        assert!(effect.channels[0].stages[0].x1.abs() > 0.0);
        effect.reset();
        for stage in effect.channels[0].stages.iter() {
            assert_eq!(stage.x1, 0.0);
            assert_eq!(stage.y1, 0.0);
            assert_eq!(stage.a, 0.0);
        }
        assert_eq!(effect.channels[0].phase, 0.0);
        assert_eq!(effect.channels[0].feedback, 0.0);
        let (left, right) = run(&mut effect, &alloc::vec![0.0_f32; 512]);
        assert!(left.iter().chain(right.iter()).all(|s| *s == 0.0));
    }

    #[test]
    fn latency_is_zero_because_there_is_no_delay_line() {
        let effect = make();
        assert_eq!(effect.latency_samples(), 0);
    }

    #[test]
    fn wet_is_clamped_and_nan_safe() {
        let mut effect = make();
        effect.set_wet(5.0);
        assert_eq!(effect.wet(), 1.0);
        effect.set_wet(f32::NAN);
        assert_eq!(effect.wet(), 1.0);
        effect.set_wet(-1.0);
        assert_eq!(effect.wet(), 0.0);
        assert_eq!(effect.get_parameter(PARAM_MIX), Some(0.0));
    }

    #[test]
    fn an_empty_block_and_a_single_sample_block_are_both_safe() {
        let mut effect = make();
        let mut empty: [&mut [f32]; 0] = [];
        {
            let mut buffer = AudioBuffer::new(&mut empty);
            effect.process(&mut buffer, &RenderContext::default());
        }
        let mut left = alloc::vec![1.0_f32; 1];
        let mut right = alloc::vec![0.0_f32; 1];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 1, 0, 120.0, 960));
        }
        assert!(left[0].is_finite());
    }

    #[test]
    fn a_fully_wet_all_pass_leaves_the_level_alone() {
        // At 100% mix there is no dry signal to cancel against, so the notches
        // must vanish: an all-pass cascade has flat magnitude. This is what
        // distinguishes the phaser's wet path from a comb's.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 0.001);
        effect.set_parameter(PARAM_DEPTH, 0.0);
        effect.set_parameter(PARAM_CENTRE_HZ, 1_000.0);
        effect.set_parameter(PARAM_STAGES, 4.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let input = tone(8_192, 1_000.0);
        let (left, _) = run(&mut effect, &input);
        let settled = peak(&left, 4_000, 8_192);
        assert!(
            (settled - 0.5).abs() < 0.05,
            "a fully wet all-pass should pass the level unchanged, got {settled}"
        );
    }

    #[test]
    fn a_half_wet_setting_reaches_a_shallower_level_than_fully_wet() {
        // The dry/shifted sum cancels where the phases oppose, which is the
        // notch. Fully wet cannot cancel, so it must be louder at a frequency
        // that lands on a notch at 50% mix.
        let measure = |mix: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_RATE, 0.001);
            effect.set_parameter(PARAM_DEPTH, 0.0);
            effect.set_parameter(PARAM_CENTRE_HZ, 1_000.0);
            effect.set_parameter(PARAM_STAGES, 4.0);
            effect.set_parameter(PARAM_FEEDBACK, 0.0);
            effect.set_parameter(PARAM_MIX, mix);
            let (left, _) = run(&mut effect, &tone(16_000, 1_000.0));
            rms(&left, 8_000, 16_000)
        };
        let half = measure(50.0);
        let full = measure(100.0);
        assert!(
            half < full,
            "the notch did not attenuate: {half} vs {full}"
        );
    }
}

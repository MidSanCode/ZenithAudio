//! Chorus: a modulated short delay mixed back with the dry signal.
//!
//! PLAN §3.S5 groups chorus, flanger and phaser under modulation. This module
//! is the chorus member of that family.
//!
//! # What makes a chorus a chorus
//!
//! A chorus is a delay line whose length is swept by a slow LFO, mixed with
//! the dry signal. Two properties are audible and both are easy to get wrong:
//!
//! 1. **The delay has to be fractional.** A chorus's delay sits between about
//!    5 ms and 40 ms, and the LFO moves it by a few milliseconds per cycle —
//!    a fraction of a sample per sample. Reading the line at an *integer* tap
//!    rounds that motion to a staircase, which turns the detuning into
//!    ring-modulation sidebands. The line here interpolates linearly.
//! 2. **The LFO has to be bipolar and centred.** A unipolar LFO sweeping
//!    0..40 ms spends half its time at a delay so short it is inaudible as
//!    detuning. The LFO here runs `-1..=1` around a centre time.
//!
//! # Feedback
//!
//! Feedback is what makes a chorus thicken rather than merely double. It is
//! also what makes it explode: the loop is `line → damping → gain → line`, and
//! a gain of 1 turns a chorus into an oscillator at the LFO's rate. The gain is
//! therefore clamped well below unity (`MAX_FEEDBACK`) and the loop still
//! passes through a one-pole low-pass, so the round-trip gain is below 1 at
//! every frequency.
//!
//! # Stereo
//!
//! The right channel's LFO runs a quarter cycle ahead of the left's. Without
//! that offset the two channels produce identical, perfectly correlated
//! detuning, which sums back to a mono effect with a comb — the opposite of
//! what the effect is for.
//!
//! # Real-time safety
//!
//! Both delay lines, both LFOs and the dry/wet scratch are allocated in
//! [`Chorus::prepare`]; `process` performs no allocation, no locking and no IO.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{cos_poly, sin_poly, DcBlocker};
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
/// Centre delay in milliseconds.
pub const PARAM_DELAY_MS: u16 = 2;
/// Feedback in percent.
pub const PARAM_FEEDBACK: u16 = 3;
/// Stereo phase offset between the two channels, in percent of a cycle.
pub const PARAM_STEREO_SPREAD: u16 = 4;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 5;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 6;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The narrowest centre delay the parameter allows, in milliseconds.
///
/// Below about 5 ms the effect stops being a chorus and starts being a flanger
/// ([`super::flanger`] is that effect, with the feedback and tuning it needs).
pub const MIN_DELAY_MS: f32 = 5.0;

/// The widest centre delay the parameter allows, in milliseconds.
pub const MAX_DELAY_MS: f32 = 40.0;

/// The most the LFO can move the delay either side of its centre, in
/// milliseconds.
///
/// The modulation window is therefore `centre ± this`, clamped to the
/// parameter range, which is what keeps a deep setting at a short centre time
/// from asking for a negative delay.
const MAX_MOD_DEPTH_MS: f32 = 20.0;

/// The feedback gain applied at 100% feedback.
///
/// A chorus is not meant to self-oscillate; the ceiling keeps the loop
/// comfortably inside the stability bound even before the damping low-pass is
/// taken into account.
const MAX_FEEDBACK: f32 = 0.7;

/// The low-pass corner inside the feedback loop, in hertz.
///
/// One pole, fixed rather than exposed: a chorus's feedback is a thickness
/// control, not a tone control, and a fixed corner means the loop gain is
/// bounded by a value the stability argument can name.
const FEEDBACK_DAMPING_HZ: f32 = 7_000.0;

/// The delay-line capacity, in milliseconds.
///
/// The centre range plus the modulation window plus the spread offset, with
/// headroom. Sized once in `prepare`.
const LINE_CAPACITY_MS: f32 = MAX_DELAY_MS + MAX_MOD_DEPTH_MS + 10.0;

/// How far apart the two channels' LFOs can be, as a fraction of a cycle.
const MAX_STEREO_SPREAD: f32 = 0.5;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_CHORUS,
    key: "chorus",
    label: "Chorus",
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
            default_value: 0.6,
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
            default_value: 45.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DELAY_MS),
            key: "delay_ms",
            label: "Delay",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: MIN_DELAY_MS,
            max_value: MAX_DELAY_MS,
            default_value: 18.0,
            smoothing_ms: 30.0,
        },
        ParameterDescriptor {
            address: at(PARAM_FEEDBACK),
            key: "feedback",
            label: "Feedback",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 0.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_STEREO_SPREAD),
            key: "stereo_spread",
            label: "Stereo",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 50.0,
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

/// One channel's modulated delay line.
#[derive(Debug)]
struct ChorusLine {
    /// The ring buffer, allocated in `prepare`.
    ring: alloc::vec::Vec<f32>,
    /// Write cursor.
    write: usize,
    /// LFO phase in radians, always kept inside `-PI..=PI` by `wrap`.
    phase: f32,
    /// One-pole low-pass state for the feedback damping.
    damping: f32,
}

impl ChorusLine {
    /// An empty line; `prepare` sizes the ring.
    fn new() -> Self {
        Self {
            ring: alloc::vec::Vec::new(),
            write: 0,
            phase: 0.0,
            damping: 0.0,
        }
    }

    /// Allocates a ring holding `capacity` usable samples.
    fn prepare(&mut self, capacity: usize) {
        self.ring = alloc::vec![0.0; capacity.max(4) + 2];
        self.write = 0;
    }

    /// The usable delay range, in samples.
    #[must_use]
    fn capacity(&self) -> usize {
        self.ring.len().saturating_sub(2)
    }

    /// Advances the LFO by `increment` radians and returns the new phase.
    ///
    /// The phase is wrapped every call rather than left to grow: an `f32` phase
    /// accumulated at 48 kHz reaches the point where consecutive values differ
    /// by more than a cycle within a few hours, and `sin` of a large argument
    /// is where a naive oscillator degenerates into noise. Wrapping keeps the
    /// argument small forever, at the cost of one comparison.
    ///
    /// The single comparison is enough because `increment` is itself clamped:
    /// a rate the user could not have meant cannot step the phase by more than
    /// a whole cycle at once, so one wrap always lands inside the window.
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
        // A phase handed in from outside the window (or a wrapping that left a
        // residual) is folded back with the shared, well-conditioned wrapper.
        self.phase = wrap_phase(phase);
        self.phase
    }

    /// Reads `delay_samples` back from the cursor, with linear interpolation.
    #[must_use]
    fn read(&self, delay_samples: f32) -> f32 {
        let capacity = self.capacity();
        let len = self.ring.len();
        if capacity == 0 || len == 0 {
            return 0.0;
        }
        let delay = if delay_samples.is_finite() {
            delay_samples.clamp(1.0, capacity as f32)
        } else {
            1.0
        };
        let whole = delay as usize;
        let fraction = delay - whole as f32;
        let base = self.write + len - 1 - whole;
        let first = self.ring[base % len];
        let second = self.ring[(base + len - 1) % len];
        first + (second - first) * fraction
    }

    /// Writes one sample and advances the cursor.
    fn write_sample(&mut self, sample: f32) {
        let len = self.ring.len();
        if len == 0 {
            return;
        }
        self.ring[self.write % len] = if sample.is_finite() { sample } else { 0.0 };
        self.write = (self.write + 1) % len;
    }

    /// Clears the ring, the LFO phase and the damping state.
    fn reset(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.phase = 0.0;
        self.damping = 0.0;
    }
}

/// The chorus effect.
#[derive(Debug)]
pub struct Chorus {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// LFO rate in hertz.
    rate_hz: f32,
    /// LFO depth in percent.
    depth_percent: f32,
    /// Centre delay in milliseconds.
    delay_ms: f32,
    /// Feedback in percent.
    feedback_percent: f32,
    /// Stereo LFO offset in percent of a cycle.
    stereo_spread_percent: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel state.
    lines: [ChorusLine; MAX_CHANNELS],
    /// The modulation delay used by the most recent block on channel 0, in
    /// samples. Retained for diagnostics and tests.
    last_delay_left: f32,
    /// The modulation delay used by the most recent block on channel 1.
    last_delay_right: f32,
    /// DC blocker on the wet path.
    dc: DcBlocker,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Preallocated dry snapshot, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet scratch for channel 0, `max_block`.
    wet_left: alloc::vec::Vec<f32>,
    /// Preallocated wet scratch for channel 1, `max_block`.
    wet_right: alloc::vec::Vec<f32>,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_RATE))
    }
}

impl Chorus {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            table,
            rate_hz: 0.6,
            depth_percent: 45.0,
            delay_ms: 18.0,
            feedback_percent: 0.0,
            stereo_spread_percent: 50.0,
            mix_percent: 50.0,
            lines: [ChorusLine::new(), ChorusLine::new()],
            last_delay_left: 0.0,
            last_delay_right: 0.0,
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

    /// The feedback gain actually applied.
    ///
    /// Always strictly below 1: the ceiling is the effect's stability
    /// guarantee, and the tests assert the resulting loop decays.
    #[must_use]
    fn feedback_gain(&self) -> f32 {
        (self.feedback_percent / 100.0).clamp(0.0, 1.0) * MAX_FEEDBACK
    }

    /// The channel's LFO phase offset, in radians.
    #[must_use]
    fn channel_offset(&self, channel: usize) -> f32 {
        let spread = (self.stereo_spread_percent / 100.0).clamp(0.0, 1.0) * MAX_STEREO_SPREAD;
        if channel == 0 {
            0.0
        } else {
            spread * 2.0 * PI
        }
    }

    /// The modulation delay for one channel, given the LFO's current value.
    ///
    /// `lfo` is the bipolar LFO output in `-1..=1`; the depth is scaled by the
    /// *available* window rather than blindly, so a deep setting at a short
    /// centre time cannot ask for a negative delay.
    #[must_use]
    fn modulated_delay_ms(&self, lfo: f32, channel: usize) -> f32 {
        let depth = (self.depth_percent / 100.0).clamp(0.0, 1.0) * MAX_MOD_DEPTH_MS;
        let centre = self
            .delay_ms
            .clamp(MIN_DELAY_MS, MAX_DELAY_MS);
        let swing = depth.min(centre - MIN_DELAY_MS).max(0.0);
        let _ = channel;
        centre + lfo * swing
    }

    /// Converts a millisecond delay to samples.
    #[must_use]
    fn ms_to_samples(&self, ms: f32) -> f32 {
        ms * 0.001 * self.sample_rate
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

impl EffectProcessor for Chorus {
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
        // Every allocation happens here. The capacity is the parameter range
        // plus the modulation window plus the stereo offset, so no setting and
        // no LFO phase can walk off the end of the line.
        let capacity = (LINE_CAPACITY_MS * 0.001 * self.sample_rate).ceil() as usize + 4;
        self.dry = alloc::vec![0.0; max_block];
        self.wet_left = alloc::vec![0.0; max_block];
        self.wet_right = alloc::vec![0.0; max_block];
        for line in self.lines.iter_mut() {
            line.prepare(capacity);
        }
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

        let rate = if ctx.sample_rate > 0.0 {
            ctx.sample_rate
        } else {
            self.sample_rate
        };
        let _ = rate;
        // One LFO cycle per second per hertz of rate.
        let increment = 2.0 * PI * self.rate_hz.clamp(0.0, 100.0) / self.sample_rate.max(1.0);
        let feedback = self.feedback_gain();
        let damping_coeff = one_pole_lowpass(FEEDBACK_DAMPING_HZ, self.sample_rate);
        let wet = self.wet;

        for channel in 0..channels {
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }
            // Sanitize the snapshot once per channel. A NaN that reached the
            // delay line would never come back out — it would sit in the ring
            // and be re-read every period — so the whole effect would be dead
            // from one bad sample. Replacing it here confines the damage to the
            // one frame that produced it.
            for sample in self.dry[..frames].iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
            let offset = self.channel_offset(channel);
            for index in 0..frames {
                let input = self.dry[index];
                // Bipolar LFO: the sine runs -1..=1 and the offset shifts only
                // the right channel, which is what produces width.
                let phase = self.lines[channel].advance_phase(increment) + offset;
                let lfo = sin_poly(phase);
                let delay = self.ms_to_samples(self.modulated_delay_ms(lfo, channel));
                if channel == 0 {
                    self.last_delay_left = delay;
                } else {
                    self.last_delay_right = delay;
                }
                let tap = self.lines[channel].read(delay);
                // Feedback is damped: the loop gain is `MAX_FEEDBACK` at DC
                // and falls with frequency, so the closed loop cannot grow.
                let state = &mut self.lines[channel].damping;
                *state += (tap - *state) * damping_coeff;
                if !state.is_finite() {
                    *state = 0.0;
                }
                let damped = *state;
                self.lines[channel].write_sample(input + damped * feedback);
                if channel == 0 {
                    if let Some(slot) = self.wet_left.get_mut(index) {
                        *slot = tap;
                    }
                } else if let Some(slot) = self.wet_right.get_mut(index) {
                    *slot = tap;
                }
            }
        }

        self.wet_mix(buffer, channels, frames, wet);
    }

    fn reset(&mut self) {
        for line in self.lines.iter_mut() {
            line.reset();
        }
        self.dc.reset();
        self.last_delay_left = 0.0;
        self.last_delay_right = 0.0;
    }

    fn latency_samples(&self) -> usize {
        // Zero: a chorus's delay is the effect's audible content and the dry
        // path is undelayed, so there is nothing for PDC to compensate.
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
            PARAM_DELAY_MS => self.delay_ms = value,
            PARAM_FEEDBACK => self.feedback_percent = value,
            PARAM_STEREO_SPREAD => self.stereo_spread_percent = value,
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
            PARAM_DELAY_MS => Some(self.delay_ms),
            PARAM_FEEDBACK => Some(self.feedback_percent),
            PARAM_STEREO_SPREAD => Some(self.stereo_spread_percent),
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
        // A chorus's feedback is short and heavily damped; one delay period is
        // a generous bound and prevents an offline bounce from truncating the
        // tail of a heavily-fed chorus.
        let longest = self
            .last_delay_left
            .max(self.last_delay_right)
            .max(0.0)
            / self.sample_rate.max(1.0);
        (longest * 4.0).max(0.05)
    }
}

/// The one-pole low-pass coefficient for `hz`. Never above 1.
#[must_use]
fn one_pole_lowpass(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 1.0;
    }
    let x = 2.0 * PI * hz.max(0.0) / sample_rate;
    x.clamp(0.0, 1.0)
}

/// Folds an arbitrary phase into `-PI..=PI`.
///
/// Uses the shared, well-conditioned wrapper rather than a local
/// `x - 2*PI*round(x/2PI)`: near a multiple of a cycle the subtraction cancels
/// catastrophically and the residual is all rounding error.
#[must_use]
fn wrap_phase(x: f32) -> f32 {
    super::super::util::dsp::wrap_pi(x)
}

/// Kept so the module compiles with the same import set the reference uses;
/// the chorus itself only needs `sin_poly`.
#[allow(dead_code)]
fn cosine_for_documentation(x: f32) -> f32 {
    cos_poly(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const SR: f32 = 48_000.0;

    fn make() -> Chorus {
        let mut effect = Chorus::new(ParameterAddress::effect(0, 0, PARAM_RATE));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Renders a stereo pair in 256-frame blocks, running `f(index)` per frame.
    fn render<F>(effect: &mut Chorus, frames: usize, bpm: f32, mut f: F) -> (Vec<f32>, Vec<f32>)
    where
        F: FnMut(usize) -> f32,
    {
        let chunk = 256;
        let mut left_out = Vec::with_capacity(frames);
        let mut right_out = Vec::with_capacity(frames);
        let mut produced = 0;
        while produced < frames {
            let n = chunk.min(frames - produced);
            let mut left: Vec<f32> = (0..n).map(|i| f(produced + i)).collect();
            let mut right = left.clone();
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, n, produced as i64, bpm, 960);
                effect.process(&mut buffer, &ctx);
            }
            left_out.extend_from_slice(&left);
            right_out.extend_from_slice(&right);
            produced += n;
        }
        (left_out, right_out)
    }

    /// A slow sine, `hz` cycles per second.
    fn tone(frames: usize, hz: f32) -> Vec<f32> {
        (0..frames)
            .map(|n| sin_poly(2.0 * PI * hz * n as f32 / SR) * 0.5)
            .collect()
    }

    /// Runs a pre-built signal through the effect in 256-frame blocks.
    fn run(effect: &mut Chorus, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut index = 0;
        let mut left_all = Vec::with_capacity(input.len());
        let mut right_all = Vec::with_capacity(input.len());
        let mut cursor = 0;
        while cursor < input.len() {
            let n = 256.min(input.len() - cursor);
            let mut left = input[cursor..cursor + n].to_vec();
            let mut right = left.clone();
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, n, index as i64, 120.0, 960);
                effect.process(&mut buffer, &ctx);
            }
            left_all.extend_from_slice(&left);
            right_all.extend_from_slice(&right);
            index += n;
            cursor += n;
        }
        (left_all, right_all)
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, 0x0000_0500);
        assert_eq!(d.key, "chorus");
        assert_eq!(d.label, "Chorus");
        assert_eq!(d.category, EffectCategory::Modulation);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(!d.has_latency);
        assert!(!d.is_analysis_only);
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
            assert!(
                (read - midpoint).abs() < 1e-3,
                "parameter {sub} read back {read}, expected {midpoint}"
            );
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
        effect.set_parameter(PARAM_DELAY_MS, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_DELAY_MS), Some(18.0));
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut left = alloc::vec![0.75_f32; 256];
        left[3] = -0.5;
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
        assert_eq!(left, expected_left, "oversized block must be a safe no-op");
        assert_eq!(right, expected_right);
    }

    #[test]
    fn non_finite_input_does_not_poison_the_output_or_the_lines() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        let mut input = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
        input.extend_from_slice(&[0.0; 507]);
        let (left, right) = run(&mut effect, &input);
        for (i, sample) in left.iter().chain(right.iter()).enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
        // The poison must not survive into the next block either.
        let (left, right) = run(&mut effect, &alloc::vec![0.0_f32; 256]);
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
    fn the_lfo_phase_wraps_and_never_runs_away() {
        // A phase that accumulated without wrapping eventually reaches the
        // magnitude where `sin` of it is meaningless. Run for a long time at a
        // high rate and check the phase stays in its documented window — and,
        // more importantly, that the LFO still moves.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 10.0);
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for _ in 0..200 {
            let _ = run(&mut effect, &alloc::vec![0.0_f32; 256]);
            for line in effect.lines.iter() {
                assert!(line.phase.is_finite());
                assert!(
                    line.phase >= -PI - 1e-3 && line.phase <= PI + 1e-3,
                    "phase escaped its window: {}",
                    line.phase
                );
                min = min.min(line.phase);
                max = max.max(line.phase);
            }
        }
        assert!(max - min > 0.5, "the LFO never moved: {max} vs {min}");
    }

    #[test]
    fn a_runaway_lfo_phase_produces_no_garbage() {
        // Force the phase to a hostile value; the next advance must bring it
        // back into range rather than feed `sin_poly` a huge argument.
        let mut effect = make();
        effect.lines[0].phase = 1.0e30;
        let (left, _) = run(&mut effect, &alloc::vec![0.5_f32; 512]);
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(effect.lines[0].phase.is_finite());
        assert!(effect.lines[0].phase.abs() <= PI);
        effect.lines[0].phase = f32::NAN;
        let (left, _) = run(&mut effect, &alloc::vec![0.5_f32; 512]);
        assert!(left.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn the_two_channels_differ_which_is_what_makes_it_stereo() {
        // With the stereo offset at its default the two channels' LFOs are
        // deliberately out of phase. Identical outputs would mean the effect
        // collapses to mono.
        let mut effect = make();
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_STEREO_SPREAD, 50.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 50.0);
        let input = tone(24_000, 220.0);
        let (left, right) = run(&mut effect, &input);
        // Compare the settled halves.
        let mut difference = 0.0_f32;
        for index in 12_000..24_000 {
            difference = difference.max((left[index] - right[index]).abs());
        }
        assert!(
            difference > 1e-3,
            "the channels are identical (max difference {difference})"
        );
    }

    #[test]
    fn a_zero_spread_makes_the_channels_identical() {
        // The converse of the previous test: it pins the *cause* of the
        // difference to the stereo offset rather than to some incidental
        // per-channel state.
        let mut effect = make();
        effect.set_parameter(PARAM_STEREO_SPREAD, 0.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        let input = tone(12_000, 220.0);
        let (left, right) = run(&mut effect, &input);
        for (i, (a, b)) in left.iter().zip(right.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn the_delay_is_modulated_over_time() {
        // The definition of a chorus: the delay moves. A stationary delay
        // would be a doubler, not a chorus.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 2.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_DELAY_MS, 18.0);
        let _ = run(&mut effect, &alloc::vec![0.0_f32; 128]);
        let first = effect.last_delay_left;
        let mut min = first;
        let mut max = first;
        for _ in 0..80 {
            let _ = run(&mut effect, &alloc::vec![0.0_f32; 128]);
            min = min.min(effect.last_delay_left);
            max = max.max(effect.last_delay_left);
        }
        assert!(max - min > 100.0, "the delay barely moved: {min}..{max}");
        // And it stays inside the documented 5..40 ms window.
        assert!(min >= MIN_DELAY_MS * 0.001 * SR - 1.0, "min was {min}");
        assert!(max <= MAX_DELAY_MS * 0.001 * SR + 1.0, "max was {max}");
    }

    #[test]
    fn a_deep_setting_at_a_short_centre_never_asks_for_a_negative_delay() {
        // A unipolar or naively scaled LFO at 5 ms centre and 100% depth would
        // ask for a delay below zero, which reads the future.
        let mut effect = make();
        effect.set_parameter(PARAM_DELAY_MS, MIN_DELAY_MS);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        for lfo in [-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
            let ms = effect.modulated_delay_ms(lfo, 0);
            assert!(ms >= MIN_DELAY_MS - 1e-3, "delay {ms} at lfo {lfo}");
            assert!(ms <= MAX_DELAY_MS + 1e-3);
        }
        // The read itself clamps too, so even a hostile value is safe.
        let samples = effect.ms_to_samples(-100.0);
        assert!(effect.lines[0].read(samples).is_finite());
    }

    #[test]
    fn the_delay_read_is_interpolated_rather_than_quantised() {
        // A chorus with integer-only delay steps is a ring modulator. Drive it
        // with a smooth input and check the output has no step larger than the
        // input's own.
        let mut effect = make();
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_RATE, 0.01);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        // Park the LFO so the delay is a fixed fraction of a sample.
        effect.lines[0].phase = 0.3;
        let input = tone(8_192, 100.0);
        let input_step = input
            .windows(2)
            .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
        let (left, _) = run(&mut effect, &input);
        let out_step = left[2_000..]
            .windows(2)
            .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(
            out_step < input_step * 1.5 + 1e-4,
            "interpolation artefact: output stepped {out_step}, input stepped {input_step}"
        );
    }

    #[test]
    fn the_output_is_detuned_against_the_dry_signal() {
        // The audible signature of a chorus: the delayed copy is a different
        // pitch from the input, so a steady tone's delayed copy drifts against
        // it. Compare the phase of the delayed signal at two points in time.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 1.0);
        effect.set_parameter(PARAM_DEPTH, 80.0);
        effect.set_parameter(PARAM_DELAY_MS, 20.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        let input = tone(48_000, 400.0);
        let (left, _) = run(&mut effect, &input);

        // Cross-correlate a window early on and a window one period later
        // against the input to find the prevailing delay in each.
        let estimate = |signal: &[f32], start: usize| -> f32 {
            let mut best = 0.0_f32;
            let mut best_lag = 0.0_f32;
            for lag in 300..1_500 {
                let mut sum = 0.0_f32;
                for n in 0..256 {
                    sum += signal[start + n] * input[start + n - lag];
                }
                if sum.abs() > best.abs() {
                    best = sum;
                    best_lag = lag as f32;
                }
            }
            best_lag
        };
        let early = estimate(&left, 24_000);
        let late = estimate(&left, 36_000);
        assert!(
            (early - late).abs() > 5.0,
            "the delay did not move between the two windows: {early} then {late}"
        );
    }

    #[test]
    fn maximum_feedback_decays_rather_than_growing() {
        // The stability requirement. An impulse into a chorus at maximum
        // feedback must produce a decaying, finite tail.
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_DELAY_MS, 10.0);
        effect.set_parameter(PARAM_DEPTH, 50.0);
        effect.set_parameter(PARAM_RATE, 0.5);
        effect.set_parameter(PARAM_MIX, 100.0);
        let mut input = alloc::vec![0.0_f32; 48_000];
        input[0] = 1.0;
        let (left, right) = run(&mut effect, &input);

        for sample in left.iter().chain(right.iter()) {
            assert!(sample.is_finite(), "non-finite sample: {sample}");
        }
        let energy = |from: usize, to: usize| -> f32 {
            left[from..to].iter().map(|s| s * s).sum::<f32>()
        };
        let early = energy(0, 12_000);
        let late = energy(36_000, 48_000);
        assert!(
            late < early,
            "the tail grew instead of decaying: {early} then {late}"
        );
    }

    #[test]
    fn the_feedback_ceiling_is_below_unity() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        assert!(effect.feedback_gain() < 1.0);
        effect.set_parameter(PARAM_FEEDBACK, 1_000.0);
        assert!(effect.feedback_gain() < 1.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        assert_eq!(effect.feedback_gain(), 0.0);
    }

    #[test]
    fn the_damping_low_pass_bounds_the_round_trip_gain() {
        // The loop is closed through a one-pole low-pass whose gain is at most
        // 1, so the round-trip gain at every frequency is below unity. Sweep
        // the input across the band and check nothing at the output grows.
        let coeff = one_pole_lowpass(FEEDBACK_DAMPING_HZ, SR);
        assert!(coeff > 0.0 && coeff <= 1.0);
        for hz in [50.0_f32, 500.0, 5_000.0, 15_000.0] {
            let mut state = 0.0_f32;
            let mut peak = 0.0_f32;
            for n in 0..4_800 {
                let x = sin_poly(2.0 * PI * hz * n as f32 / SR);
                state += (x - state) * coeff;
                peak = peak.max(state.abs());
            }
            assert!(peak <= 1.05, "damping gained at {hz} Hz: {peak}");
        }
    }

    #[test]
    fn reset_clears_the_lines_and_the_lfo() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        let input = tone(2_048, 220.0);
        let _ = run(&mut effect, &input);
        assert!(
            effect.lines[0].ring.iter().any(|s| s.abs() > 0.0),
            "the line should hold something before a reset"
        );
        effect.reset();
        assert!(effect.lines[0].ring.iter().all(|s| *s == 0.0));
        assert_eq!(effect.lines[0].write, 0);
        assert_eq!(effect.lines[0].phase, 0.0);
        assert_eq!(effect.lines[0].damping, 0.0);
        let (left, right) = run(&mut effect, &alloc::vec![0.0_f32; 512]);
        assert!(left.iter().chain(right.iter()).all(|s| *s == 0.0));
    }

    #[test]
    fn latency_is_zero() {
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
    fn the_delay_line_holds_the_whole_parameter_range() {
        // Whatever the parameters say, the read must be in range: a chorus
        // that clamped silently would sound wrong at its extremes without ever
        // announcing it.
        let mut effect = make();
        effect.set_parameter(PARAM_DELAY_MS, MAX_DELAY_MS);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_STEREO_SPREAD, 100.0);
        let capacity = effect.lines[0].capacity() as f32;
        let needed = effect.ms_to_samples(MAX_DELAY_MS + MAX_MOD_DEPTH_MS);
        assert!(
            capacity >= needed,
            "the line holds {capacity}, needs {needed}"
        );
    }

    #[test]
    fn the_mix_control_blends_rather_than_switching() {
        // 50% mix must contain both the dry and the wet signal, which is what
        // makes the effect additive rather than a replacement.
        let mut effect = make();
        effect.set_parameter(PARAM_MIX, 50.0);
        effect.set_parameter(PARAM_DELAY_MS, 20.0);
        effect.set_parameter(PARAM_DEPTH, 0.0);
        let input = tone(8_192, 200.0);
        let (left, _) = run(&mut effect, &input);
        // Half the dry is still there: the output is not silent and not equal
        // to zero.
        let peak = left[4_000..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "a 50% mix should pass audio, got {peak}");
        // With no depth the wet path is a pure delay of the dry signal, so the
        // sum of dry and wet is a comb: some frequency dips, none of it dies.
        assert!(peak <= 0.6);
    }
}

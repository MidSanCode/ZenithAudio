//! Flanger: a very short modulated delay with heavy feedback.
//!
//! Same family as [`super::chorus`], different sound. A flanger sweeps a delay
//! of roughly 0.5–10 ms — short enough that the delayed copy sums with the dry
//! signal *coherently*, so instead of hearing a second voice the listener hears
//! a comb: a series of notches evenly spaced in frequency. Sweeping the delay
//! moves the notches, and that whoosh is the effect.
//!
//! # Why the delay range is short
//!
//! The notch spacing is `sample_rate / delay`. At 1 ms and 48 kHz the notches
//! are 1 kHz apart and there are a dozen in the audible band; at 20 ms they are
//! 50 Hz apart and there are hundreds, which the ear hears as a hollow
//! resonance rather than as a comb. Both are useful, but only the first is a
//! flanger, so the parameter range stops at 10 ms.
//!
//! # Why the feedback is high
//!
//! A flanger without feedback is a thin, static-sounding effect. The feedback
//! is what sharpens the notches into the metallic resonance the effect is
//! known for. It is also the part that can run away, so:
//!
//! * the gain is clamped strictly below unity (`MAX_FEEDBACK`);
//! * the polarity switch means the *signed* gain is bounded in
//!   `-MAX_FEEDBACK..=MAX_FEEDBACK`, so inverting cannot push it past the
//!   stability bound either;
//! * the loop passes through a one-pole low-pass whose gain never exceeds 1.
//!
//! The round-trip gain is therefore below 1 at every frequency, which the tests
//! verify by driving an impulse at maximum feedback and checking the tail
//! decays.
//!
//! # Real-time safety
//!
//! Both delay lines and both LFOs are inline; the scratch is allocated in
//! [`Flanger::prepare`]. `process` allocates nothing.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{sin_poly, DcBlocker};
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
/// Feedback polarity.
pub const PARAM_POLARITY: u16 = 4;
/// Stereo LFO offset in percent of a cycle.
pub const PARAM_STEREO_SPREAD: u16 = 5;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 6;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 7;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The narrowest centre delay the parameter allows, in milliseconds.
pub const MIN_DELAY_MS: f32 = 0.1;

/// The widest centre delay the parameter allows, in milliseconds.
pub const MAX_DELAY_MS: f32 = 10.0;

/// The most the LFO can move the delay either side of its centre, in
/// milliseconds.
const MAX_MOD_DEPTH_MS: f32 = 10.0;

/// The absolute feedback gain applied at 100% feedback.
///
/// Strictly below 1 by construction; the loop is short and barely damped at the
/// low end of the band, so the ceiling is what keeps it from oscillating.
const MAX_FEEDBACK: f32 = 0.95;

/// The low-pass corner inside the feedback loop, in hertz.
///
/// Higher than the chorus's because a flanger's resonance lives further up the
/// band — but still finite, so the round-trip gain falls with frequency.
const FEEDBACK_DAMPING_HZ: f32 = 12_000.0;

/// The delay-line capacity, in milliseconds.
const LINE_CAPACITY_MS: f32 = MAX_DELAY_MS + MAX_MOD_DEPTH_MS + 5.0;

/// How far apart the two channels' LFOs can be, as a fraction of a cycle.
const MAX_STEREO_SPREAD: f32 = 0.5;

/// The feedback polarity.
///
/// Inverting the feedback moves the comb's notches onto the frequencies the
/// summed response would otherwise reinforce, which is the difference between a
/// "through" and a "hollow" character. The switch is a signed gain, not a
/// second signal path, so the stability bound is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Polarity {
    /// Feedback adds, placing notches at odd multiples of half the comb period.
    Positive = 0,
    /// Feedback subtracts, shifting every notch by half a period.
    Negative = 1,
}

impl Polarity {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Positive),
            1 => Some(Self::Negative),
            _ => None,
        }
    }

    /// The ABI discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// The sign applied to the feedback gain.
    #[must_use]
    pub const fn sign(self) -> f32 {
        match self {
            Self::Positive => 1.0,
            Self::Negative => -1.0,
        }
    }
}

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_FLANGER,
    key: "flanger",
    label: "Flanger",
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
            default_value: 0.25,
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
            default_value: 60.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DELAY_MS),
            key: "delay_ms",
            label: "Manual",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: MIN_DELAY_MS,
            max_value: MAX_DELAY_MS,
            default_value: 1.5,
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
            default_value: 60.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_POLARITY),
            key: "polarity",
            label: "Polarity",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 1.0,
            default_value: Polarity::Positive.as_u32() as f32,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_STEREO_SPREAD),
            key: "stereo_spread",
            label: "Stereo",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 0.0,
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
struct FlangerLine {
    /// The ring buffer, allocated in `prepare`.
    ring: alloc::vec::Vec<f32>,
    /// Write cursor.
    write: usize,
    /// LFO phase in radians, kept inside `-PI..=PI`.
    phase: f32,
    /// One-pole low-pass state for the feedback damping.
    damping: f32,
}

impl FlangerLine {
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
        self.phase = wrap_phase(phase);
        self.phase
    }

    /// Reads `delay_samples` back, with linear interpolation.
    ///
    /// A flanger's whole character comes from a delay that is a fraction of a
    /// sample long and moving; an integer read would quantise the sweep into
    /// steps and turn the resonance into a rattle.
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

    /// Clears the ring and every piece of state.
    fn reset(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.phase = 0.0;
        self.damping = 0.0;
    }
}

/// The flanger effect.
#[derive(Debug)]
pub struct Flanger {
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
    /// Feedback polarity.
    polarity: Polarity,
    /// Stereo LFO offset in percent of a cycle.
    stereo_spread_percent: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel state.
    lines: [FlangerLine; MAX_CHANNELS],
    /// The modulation delay used by the most recent block on channel 0.
    last_delay_left: f32,
    /// The modulation delay used by the most recent block on channel 1.
    last_delay_right: f32,
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

impl Default for Flanger {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_RATE))
    }
}

impl Flanger {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            table,
            rate_hz: 0.25,
            depth_percent: 60.0,
            delay_ms: 1.5,
            feedback_percent: 60.0,
            polarity: Polarity::Positive,
            stereo_spread_percent: 0.0,
            mix_percent: 50.0,
            lines: [FlangerLine::new(), FlangerLine::new()],
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

    /// The signed feedback gain actually applied.
    ///
    /// Its absolute value is always below 1: the fold is symmetric, so
    /// inverting the polarity cannot push the magnitude past the stability
    /// bound the positive case already respects.
    #[must_use]
    fn feedback_gain(&self) -> f32 {
        (self.feedback_percent / 100.0).clamp(0.0, 1.0) * MAX_FEEDBACK * self.polarity.sign()
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

    /// The modulation delay for `lfo`, in milliseconds.
    ///
    /// The depth is scaled by the window available at the current centre, so a
    /// deep setting at 0.1 ms centre cannot ask for a negative delay.
    #[must_use]
    fn modulated_delay_ms(&self, lfo: f32) -> f32 {
        let depth = (self.depth_percent / 100.0).clamp(0.0, 1.0) * MAX_MOD_DEPTH_MS;
        let centre = self.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS);
        let swing = depth.min(centre - MIN_DELAY_MS).max(0.0);
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

impl EffectProcessor for Flanger {
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

        let _ = ctx;
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
            // delay line would never leave — it would sit in the ring and be
            // re-read every period — so one bad sample would kill the effect.
            for sample in self.dry[..frames].iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
            }
            let offset = self.channel_offset(channel);
            for index in 0..frames {
                let input = self.dry[index];
                let phase = self.lines[channel].advance_phase(increment) + offset;
                let lfo = sin_poly(phase);
                let delay = self.ms_to_samples(self.modulated_delay_ms(lfo));
                if channel == 0 {
                    self.last_delay_left = delay;
                } else {
                    self.last_delay_right = delay;
                }
                let tap = self.lines[channel].read(delay);
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
        // Zero: the dry path is undelayed and the comb is the effect's audible
        // content, so PDC has nothing to align.
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
            PARAM_POLARITY => {
                if let Some(polarity) = Polarity::from_u32(value as u32) {
                    self.polarity = polarity;
                }
            }
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
            PARAM_POLARITY => Some(self.polarity.as_u32() as f32),
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
        // A flanger's comb decay is dominated by the feedback; the delay is
        // very short, so even a generous multiple of it keeps an offline bounce
        // bounded.
        let longest = self
            .last_delay_left
            .max(self.last_delay_right)
            .max(0.0)
            / self.sample_rate.max(1.0);
        (longest * 64.0).min(1.0).max(0.02)
    }
}

/// The one-pole low-pass coefficient for `hz`. Never above 1.
#[must_use]
fn one_pole_lowpass(hz: f32, sample_rate: f32) -> f32 {
    if sample_rate <= 0.0 {
        return 1.0;
    }
    (2.0 * PI * hz.max(0.0) / sample_rate).clamp(0.0, 1.0)
}

/// Folds an arbitrary phase into `-PI..=PI` using the shared wrapper.
#[must_use]
fn wrap_phase(x: f32) -> f32 {
    super::super::util::dsp::wrap_pi(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const SR: f32 = 48_000.0;

    fn make() -> Flanger {
        let mut effect = Flanger::new(ParameterAddress::effect(0, 0, PARAM_RATE));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Runs `input` through the effect in 256-frame blocks.
    fn run(effect: &mut Flanger, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
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
        assert_eq!(d.kind, 0x0000_0501);
        assert_eq!(d.key, "flanger");
        assert_eq!(d.label, "Flanger");
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
                assert_eq!(read, midpoint.floor());
            } else {
                assert!((read - midpoint).abs() < 1e-3, "parameter {sub}: {read}");
            }
        }
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_nan_falls_back() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 1e9);
        assert_eq!(effect.get_parameter(PARAM_FEEDBACK), Some(100.0));
        effect.set_parameter(PARAM_FEEDBACK, -1e9);
        assert_eq!(effect.get_parameter(PARAM_FEEDBACK), Some(0.0));
        effect.set_parameter(PARAM_DELAY_MS, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_DELAY_MS), Some(1.5));
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut left = alloc::vec![0.75_f32; 256];
        left[11] = -0.5;
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
        assert!(effect.feedback_gain().abs() < 1.0);
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
    fn the_comb_notches_move_with_the_lfo() {
        // The defining behaviour: the notches are not fixed. Probe the output
        // energy at a single frequency at two different LFO phases; a static
        // comb would give the same answer both times.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 0.5);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_DELAY_MS, 5.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 50.0);

        let probe = 1_000.0_f32;
        let input = tone(24_000, probe);
        let (left, _) = run(&mut effect, &input);
        // Two windows half a cycle of the 0.5 Hz LFO apart (one second).
        let a = rms(&left, 2_000, 6_000);
        let b = rms(&left, 14_000, 18_000);
        assert!(
            (a - b).abs() > 0.005,
            "the comb did not move: {a} then {b} at {probe} Hz"
        );
    }

    #[test]
    fn a_constant_input_produces_a_time_varying_output() {
        // The simplest statement of "the comb moves": with stationary input the
        // output is not stationary.
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 1.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_DELAY_MS, 4.0);
        effect.set_parameter(PARAM_FEEDBACK, 30.0);
        effect.set_parameter(PARAM_MIX, 50.0);
        let input = tone(24_000, 700.0);
        let (left, _) = run(&mut effect, &input);

        let mut minima = f32::MAX;
        let mut maxima = f32::MIN;
        for block in 8..90 {
            let start = block * 256;
            let peak = left[start..start + 256]
                .iter()
                .fold(0.0_f32, |m, s| m.max(s.abs()));
            minima = minima.min(peak);
            maxima = maxima.max(peak);
        }
        assert!(
            maxima - minima > 0.01,
            "the output is stationary: {minima}..{maxima}"
        );
    }

    #[test]
    fn the_delay_stays_inside_the_short_flanger_window() {
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 2.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_DELAY_MS, 5.0);
        let _ = run(&mut effect, &alloc::vec![0.0_f32; 128]);
        let mut min = effect.last_delay_left;
        let mut max = min;
        for _ in 0..200 {
            let _ = run(&mut effect, &alloc::vec![0.0_f32; 128]);
            min = min.min(effect.last_delay_left);
            max = max.max(effect.last_delay_left);
        }
        assert!(max - min > 1.0, "the delay never moved: {min}..{max}");
        assert!(min >= MIN_DELAY_MS * 0.001 * SR - 1.0, "min was {min}");
        assert!(max <= MAX_DELAY_MS * 0.001 * SR + 1.0, "max was {max}");
        // The whole point of a flanger rather than a chorus.
        assert!(max < SR * 0.011, "the flanger strayed into chorus range");
    }

    #[test]
    fn a_deep_setting_at_a_short_centre_never_asks_for_a_negative_delay() {
        let mut effect = make();
        effect.set_parameter(PARAM_DELAY_MS, MIN_DELAY_MS);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        for lfo in [-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
            let ms = effect.modulated_delay_ms(lfo);
            assert!(ms >= MIN_DELAY_MS - 1e-3, "delay {ms} at lfo {lfo}");
            assert!(ms <= MAX_DELAY_MS + 1e-3);
        }
        assert!(effect.lines[0].read(effect.ms_to_samples(-100.0)).is_finite());
    }

    #[test]
    fn the_delay_read_is_interpolated() {
        let mut effect = make();
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_RATE, 0.01);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        effect.lines[0].phase = 0.7;
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
            "interpolation artefact: output stepped {out_step}, input {input_step}"
        );
    }

    #[test]
    fn the_comb_reaches_the_expected_null_depth() {
        // A comb at 50% mix should all but cancel a tone that lands on a null.
        // This pins the effect as a real comb rather than a gain stage.
        let mut effect = make();
        effect.set_parameter(PARAM_DEPTH, 0.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 50.0);
        effect.set_parameter(PARAM_DELAY_MS, 1.0);
        // 1 ms at 48 kHz is 48 samples, so the first null is at 500 Hz.
        let input = tone(8_192, 500.0);
        let (left, _) = run(&mut effect, &input);
        let settled = left[4_000..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            settled < 0.1,
            "a tone on a comb null should cancel, got {settled}"
        );

        // And a tone between the nulls survives.
        let mut effect = make();
        effect.set_parameter(PARAM_DEPTH, 0.0);
        effect.set_parameter(PARAM_FEEDBACK, 0.0);
        effect.set_parameter(PARAM_MIX, 50.0);
        effect.set_parameter(PARAM_DELAY_MS, 1.0);
        let input = tone(8_192, 1000.0);
        let (left, _) = run(&mut effect, &input);
        let settled = left[4_000..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            settled > 0.3,
            "a tone between the nulls should pass, got {settled}"
        );
    }

    #[test]
    fn the_polarity_switch_moves_the_notches() {
        // Inverting the feedback shifts the whole comb, so the same probe must
        // give a measurably different result.
        let measure = |polarity: Polarity| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DEPTH, 0.0);
            effect.set_parameter(PARAM_FEEDBACK, 80.0);
            effect.set_parameter(PARAM_MIX, 50.0);
            effect.set_parameter(PARAM_DELAY_MS, 1.0);
            effect.set_parameter(PARAM_POLARITY, polarity.as_u32() as f32);
            let input = tone(16_000, 500.0);
            let (left, _) = run(&mut effect, &input);
            rms(&left, 8_000, 16_000)
        };
        let positive = measure(Polarity::Positive);
        let negative = measure(Polarity::Negative);
        assert!(
            (positive - negative).abs() > 0.01,
            "the polarity switch did nothing: {positive} vs {negative}"
        );
    }

    #[test]
    fn the_polarity_signs_the_feedback_without_enlarging_it() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        effect.set_parameter(PARAM_POLARITY, Polarity::Positive.as_u32() as f32);
        let positive = effect.feedback_gain();
        effect.set_parameter(PARAM_POLARITY, Polarity::Negative.as_u32() as f32);
        let negative = effect.feedback_gain();
        assert!((positive + negative).abs() < 1e-6, "not a mirror");
        assert!(negative.abs() < 1.0);
        assert!(positive.abs() < 1.0);
    }

    #[test]
    fn maximum_feedback_decays_rather_than_growing() {
        // The stability requirement: an impulse at maximum feedback must leave
        // a tail that shrinks, in both polarities.
        for polarity in [Polarity::Positive, Polarity::Negative] {
            let mut effect = make();
            effect.set_parameter(PARAM_FEEDBACK, 100.0);
            effect.set_parameter(PARAM_DELAY_MS, 2.0);
            effect.set_parameter(PARAM_DEPTH, 30.0);
            effect.set_parameter(PARAM_RATE, 0.3);
            effect.set_parameter(PARAM_MIX, 100.0);
            effect.set_parameter(PARAM_POLARITY, polarity.as_u32() as f32);

            let mut input = alloc::vec![0.0_f32; 48_000];
            input[0] = 1.0;
            let (left, right) = run(&mut effect, &input);

            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(sample.is_finite(), "{polarity:?} sample {i} is {sample}");
            }
            let energy =
                |from: usize, to: usize| -> f32 { left[from..to].iter().map(|s| s * s).sum::<f32>() };
            let early = energy(0, 12_000);
            let late = energy(36_000, 48_000);
            assert!(late < early, "{polarity:?} tail grew: {early} then {late}");
        }
    }

    #[test]
    fn the_feedback_ceiling_stays_below_unity() {
        let mut effect = make();
        for polarity in [Polarity::Positive, Polarity::Negative] {
            effect.set_parameter(PARAM_POLARITY, polarity.as_u32() as f32);
            effect.set_parameter(PARAM_FEEDBACK, 100.0);
            assert!(effect.feedback_gain().abs() < 1.0);
            effect.set_parameter(PARAM_FEEDBACK, 1_000.0);
            assert!(effect.feedback_gain().abs() < 1.0);
            effect.set_parameter(PARAM_FEEDBACK, 0.0);
            assert_eq!(effect.feedback_gain(), 0.0);
        }
    }

    #[test]
    fn a_resonant_setting_does_not_ring_up_forever() {
        // The characteristic flanger sound is a near-unity loop; it must still
        // be a *decaying* resonance rather than an oscillator.
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 95.0);
        effect.set_parameter(PARAM_DELAY_MS, 3.0);
        effect.set_parameter(PARAM_DEPTH, 10.0);
        effect.set_parameter(PARAM_MIX, 100.0);
        let input = tone(96_000, 1_000.0);
        let (left, _) = run(&mut effect, &input);
        let early = left[24_000..48_000]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
        let late = left[72_000..96_000]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(late.is_finite());
        assert!(
            late <= early * 1.05 + 1e-4,
            "the resonance ran away: {early} then {late}"
        );
    }

    #[test]
    fn the_two_channels_can_be_spread() {
        let mut effect = make();
        effect.set_parameter(PARAM_STEREO_SPREAD, 100.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        let input = tone(12_000, 500.0);
        let (left, right) = run(&mut effect, &input);
        let difference = (4_000..12_000).fold(0.0_f32, |m, i| m.max((left[i] - right[i]).abs()));
        assert!(difference > 1e-3, "the channels are identical: {difference}");

        // With no spread they must match exactly.
        let mut effect = make();
        effect.set_parameter(PARAM_STEREO_SPREAD, 0.0);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        let (left, right) = run(&mut effect, &input);
        for (i, (a, b)) in left.iter().zip(right.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn the_lfo_phase_wraps_and_never_runs_away() {
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 10.0);
        for _ in 0..200 {
            let _ = run(&mut effect, &alloc::vec![0.0_f32; 256]);
            for line in effect.lines.iter() {
                assert!(line.phase.is_finite());
                assert!(
                    line.phase >= -PI - 1e-3 && line.phase <= PI + 1e-3,
                    "phase escaped: {}",
                    line.phase
                );
            }
        }
    }

    #[test]
    fn a_runaway_lfo_phase_produces_no_garbage() {
        let mut effect = make();
        effect.lines[0].phase = 1.0e30;
        let (left, _) = run(&mut effect, &alloc::vec![0.5_f32; 512]);
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(effect.lines[0].phase.abs() <= PI);
        effect.lines[0].phase = f32::NAN;
        let (left, _) = run(&mut effect, &alloc::vec![0.5_f32; 512]);
        assert!(left.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn reset_clears_the_lines_and_the_lfo() {
        let mut effect = make();
        effect.set_parameter(PARAM_FEEDBACK, 100.0);
        let _ = run(&mut effect, &tone(2_048, 220.0));
        assert!(effect.lines[0].ring.iter().any(|s| s.abs() > 0.0));
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
    fn the_delay_line_holds_the_whole_parameter_range() {
        let mut effect = make();
        effect.set_parameter(PARAM_DELAY_MS, MAX_DELAY_MS);
        effect.set_parameter(PARAM_DEPTH, 100.0);
        effect.set_parameter(PARAM_STEREO_SPREAD, 100.0);
        let capacity = effect.lines[0].capacity() as f32;
        let needed = effect.ms_to_samples(MAX_DELAY_MS + MAX_MOD_DEPTH_MS);
        assert!(capacity >= needed, "holds {capacity}, needs {needed}");
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
}

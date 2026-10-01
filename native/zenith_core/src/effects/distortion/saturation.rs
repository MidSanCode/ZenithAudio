//! Saturation: a memoryless waveshaper, oversampled.
//!
//! # What it does
//!
//! A waveshaper applies a static transfer curve `y = f(x)` to every sample.
//! "Static" is the whole point: the curve has no memory, so the output for a
//! given input never depends on what came before, which is what makes the
//! effect instantaneous and (with the oversampling accounted for) zero-latency.
//!
//! Four curves are offered, because "distortion" is not one sound:
//!
//! | Character | Curve | What it does |
//! |---|---|---|
//! | [`Character::Soft`] | `tanh` | Rounds the peaks off. The odd-symmetric, infinitely differentiable case: it adds odd harmonics and grows them gently, which is the "warm" end of the range. |
//! | [`Character::Hard`] | `clamp(-1, 1)` | Shears the peaks flat. A discontinuity in the *derivative*, so it generates a much richer, harsher harmonic series - the "aggressive" end. |
//! | [`Character::Asymmetric`] | `tanh` with a different gain either side of zero | The curve is no longer odd, so it generates **even** harmonics as well, and a DC offset. This is the valve-like case; the DC is why the effect blocks it. |
//! | [`Character::Fold`] | triangle-wave reflection | Past the rail the signal *turns around* instead of flattening, so a louder input produces a lower output. Inharmonic and metallic - a sound effect rather than a mix tool. |
//!
//! # Why the fold is a reflection and not a modulo
//!
//! `y = ((x + 1) mod 4) - 1` looks like a fold and is one, for the first
//! period. But `mod` wraps at the point where the signal crosses zero, so a
//! smoothly rising input produces a jump discontinuity in the output every full
//! period - a click. The reflection below instead bounces the signal off the
//! rails: it is continuous everywhere, and only its derivative changes
//! direction. That is the difference between a musical effect and a fault.
//!
//! # Why oversampling
//!
//! Any nonlinearity generates harmonics above Nyquist, and those fold back down
//! as inharmonic tones that no downstream filter removes. A hard-clipped 7 kHz
//! tone puts energy at 21 kHz, 35 kHz, ... and the terms above 24 kHz land
//! squarely in the audible band. Running the curve at 4x and filtering on the
//! way back down keeps the images where the decimation filter can remove them.
//! [`crate::effects::util::Oversampler`] is the suite's single implementation
//! of that (the S5 plan forbids each effect rolling its own half-band filter),
//! and `oversampling_reduces_the_folded_energy_of_a_hard_clipped_tone` measures
//! the improvement rather than assuming it.
//!
//! # Gain compensation
//!
//! Driving a saturator harder makes it louder as well as dirtier, which makes
//! A/B comparison useless - the louder one always "sounds better". The trim is
//! therefore applied automatically: the output is divided by `gain^0.7`, which
//! cancels most of the level rise without flattening the effect's dynamics
//! entirely. The `output_db` parameter is an *additional* manual trim on top,
//! for the cases where the automatic one is not what the user wants.
//!
//! # Real-time safety
//!
//! The dry snapshot, the wet working buffer and the oversampling scratch are
//! all allocated in [`Saturation::prepare`]. `process` allocates nothing: the
//! shaping closure captures two `f32`s by value and is passed straight to the
//! oversampler.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{db_to_gain, powf, tanh_poly, DcBlocker};
use super::super::util::{Oversampler, OversamplingFactor};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Parameter ordinals, published in this order.
pub const PARAM_DRIVE: u16 = 0;
/// Curve selection.
pub const PARAM_CHARACTER: u16 = 1;
/// Asymmetry applied before the curve, in percent.
pub const PARAM_BIAS: u16 = 2;
/// Manual output trim in decibels, on top of the automatic compensation.
pub const PARAM_OUTPUT: u16 = 3;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 4;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 5;

/// Channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The oversampling factor the nonlinearity runs at.
const FACTOR: OversamplingFactor = OversamplingFactor::X4;

/// The exponent of the automatic gain compensation.
///
/// `out = shaped(x * g) / g^0.7`. At 0 the compensation is complete (output level
/// is constant in `g`, which also removes the effect's dynamics); at 1 it is
/// absent. 0.7 leaves a little level rise so pushing the drive still *feels*
/// louder, without the 12 dB swing that makes an A/B useless.
const COMPENSATION_EXPONENT: f32 = 0.7;

/// How much of the bias is injected as DC before the curve.
///
/// The bias parameter is a percentage; this maps 100 % to 0.5 of full scale,
/// which is enough to make the curve audibly asymmetric without pushing a quiet
/// signal so far up the curve that it is all second harmonic and no fundamental.
const BIAS_SCALE: f32 = 0.005;

/// Which transfer curve the shaper applies.
///
/// Discriminants are the published ABI values and must never change; new
/// characters are appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Character {
    /// `tanh`: odd-symmetric, smooth, odd harmonics, gentle.
    Soft = 0,
    /// Hard clip at the rails: odd harmonics, harsh.
    Hard = 1,
    /// `tanh` with a different gain either side of zero: adds even harmonics.
    Asymmetric = 2,
    /// Triangle-wave reflection past the rails: inharmonic and metallic.
    Fold = 3,
}

impl Character {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    ///
    /// Unknown values must fail rather than coerce: silently treating a future
    /// curve as `Soft` would make an old build sound subtly wrong instead of
    /// obviously wrong.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Soft),
            1 => Some(Self::Hard),
            2 => Some(Self::Asymmetric),
            3 => Some(Self::Fold),
            _ => None,
        }
    }

    /// The ABI discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// The stable machine-readable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Soft => "soft",
            Self::Hard => "hard",
            Self::Asymmetric => "asymmetric",
            Self::Fold => "fold",
        }
    }

    /// Whether this curve generates a DC offset for a zero-mean input.
    ///
    /// Only the asymmetric one does; the caller uses this to skip nothing (DC
    /// blocking is unconditional) but the tests assert it, and a UI can use it
    /// to explain why a level meter reads high.
    #[must_use]
    pub const fn is_asymmetric(self) -> bool {
        matches!(self, Self::Asymmetric)
    }

    /// Applies the curve, without any drive or bias.
    ///
    /// Exposed so a test can check the shape directly, and so the transfer
    /// function is written once rather than being duplicated between the audio
    /// path and the tests.
    #[must_use]
    pub fn shape(self, x: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        match self {
            Self::Soft => tanh_poly(x),
            Self::Hard => x.clamp(-1.0, 1.0),
            Self::Asymmetric => {
                // A different slope either side of zero makes the curve
                // non-odd, which is exactly what produces the even harmonics.
                let scaled = if x >= 0.0 { x } else { x * 2.0 };
                tanh_poly(scaled)
            }
            Self::Fold => Self::fold(x),
        }
    }

    /// A triangle-wave reflection: continuous everywhere, with a discontinuous
    /// derivative at the rail.
    ///
    /// Written as an explicit reflection rather than a modulo so the result is
    /// continuous as the input crosses zero. See the module docs.
    #[must_use]
    fn fold(x: f32) -> f32 {
        // Period 4 triangle centred on zero: rise to +1, fall to -1, rise
        // again - reading back down gives the reflection.
        let period = 4.0_f32;
        let shifted = x + 1.0;
        // `rem_euclid` is not used because it is not const-friendly across the
        // targets this crate builds for, and because the manual reduction below
        // is easy to audit.
        let mut wrapped = shifted - (shifted / period).floor() * period;
        if wrapped < 0.0 {
            wrapped += period;
        }
        // `wrapped` is now in 0..4. The triangle is |wrapped - 2| - 1, negated
        // so it rises first, matching the sign of the input.
        1.0 - (wrapped - 2.0).abs()
    }
}

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_SATURATION,
    key: "saturation",
    label: "Saturation",
    category: EffectCategory::Distortion,
    first_param: 0,
    param_count: PARAM_COUNT,
    // The nonlinearity runs at 4x, so the wet path carries the half-band
    // filter's delay. Reporting zero would misalign the channel against every
    // other track.
    has_latency: true,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_DRIVE),
            key: "drive_db",
            label: "Drive",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 36.0,
            default_value: 0.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_CHARACTER),
            key: "character",
            label: "Character",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 3.0,
            default_value: Character::Soft.as_u32() as f32,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_BIAS),
            key: "bias",
            label: "Bias",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::BIPOLAR
                | parameter_flags::SMOOTHED,
            min_value: -100.0,
            max_value: 100.0,
            default_value: 0.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_OUTPUT),
            key: "output_db",
            label: "Output",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: -24.0,
            max_value: 24.0,
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
            default_value: 100.0,
            smoothing_ms: 20.0,
        },
    ]
}

/// The auto-compensation gain for a drive of `gain`.
///
/// `1 / gain^0.7`, so the shaped output of a small signal comes back near where
/// it started. At unity drive this is exactly 1.0, which is what makes "0 dB
/// drive" a true bypass of the nonlinearity.
#[must_use]
pub fn compensation(drive_gain: f32) -> f32 {
    if drive_gain <= 1.0 {
        return 1.0;
    }
    1.0 / powf(drive_gain, COMPENSATION_EXPONENT)
}

/// The saturation effect.
#[derive(Debug)]
pub struct Saturation {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Drive in decibels.
    drive_db: f32,
    /// Selected curve.
    character: Character,
    /// Bias in percent, bipolar.
    bias_percent: f32,
    /// Manual output trim in decibels.
    output_db: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel DC blockers, fed by the bias.
    dc: [DcBlocker; MAX_CHANNELS],
    /// Per-channel oversamplers, one each so their filter state stays
    /// contiguous.
    oversamplers: [Oversampler; MAX_CHANNELS],
    /// Preallocated oversampling scratch, `max_block * factor`.
    scratch: alloc::vec::Vec<f32>,
    /// Preallocated snapshot of the dry input, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated driven input (dry x drive + bias), `max_block`.
    driven: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer, `max_block`.
    wet_buf: alloc::vec::Vec<f32>,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Channels currently active.
    active_channels: usize,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for Saturation {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, 0))
    }
}

impl Saturation {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            drive_db: table[PARAM_DRIVE as usize].default_value,
            character: Character::Soft,
            bias_percent: table[PARAM_BIAS as usize].default_value,
            output_db: table[PARAM_OUTPUT as usize].default_value,
            mix_percent: table[PARAM_MIX as usize].default_value,
            wet: table[PARAM_MIX as usize].default_value / 100.0,
            table,
            dc: [DcBlocker::default(); MAX_CHANNELS],
            oversamplers: [
                Oversampler::new(FACTOR),
                Oversampler::new(FACTOR),
            ],
            scratch: alloc::vec::Vec::new(),
            dry: alloc::vec::Vec::new(),
            driven: alloc::vec::Vec::new(),
            wet_buf: alloc::vec::Vec::new(),
            sample_rate: 48_000.0,
            active_channels: MAX_CHANNELS,
            max_block: 0,
            bypassed: false,
        }
    }

    /// Whether the nonlinearity is engaged.
    ///
    /// At exactly 0 dB drive with no bias, the curve is already the identity
    /// for a small signal, so applying it (and its oversampling) buys nothing
    /// and costs a filter delay. Skipping it is what makes
    /// `latency_samples` return 0 at 0 dB.
    fn is_engaged(&self) -> bool {
        self.drive_db > 0.01 || self.bias_percent.abs() > 1e-3
    }

    /// The transfer function this instance applies, drive and bias included.
    ///
    /// Public within the crate so a test can compare the audio path against the
    /// curve it claims to implement, without going through the oversampler.
    #[must_use]
    pub fn transfer(&self, x: f32) -> f32 {
        let gain = db_to_gain(self.drive_db);
        let bias = self.bias_percent * BIAS_SCALE;
        let comp = compensation(gain);
        let trim = db_to_gain(self.output_db);
        self.character.shape(x * gain + bias) * comp * trim
    }
}

impl EffectProcessor for Saturation {
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
        self.active_channels = channels.clamp(1, MAX_CHANNELS);

        // Every allocation this effect will ever make happens here.
        let factor = FACTOR.multiplier();
        self.dry = alloc::vec![0.0; max_block];
        self.driven = alloc::vec![0.0; max_block];
        self.wet_buf = alloc::vec![0.0; max_block];
        self.scratch = alloc::vec![0.0; max_block * factor];

        for (index, oversampler) in self.oversamplers.iter_mut().enumerate() {
            if index < self.active_channels {
                oversampler.configure(FACTOR, self.sample_rate, max_block);
            }
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
        // A block larger than `prepare` sized for is refused rather than
        // indexed past the scratch.
        if frames > self.max_block
            || frames > self.dry.len()
            || frames > self.driven.len()
            || frames > self.wet_buf.len()
        {
            return;
        }
        let _ = ctx;

        let wet = self.wet;
        let gain = db_to_gain(self.drive_db);
        let bias = self.bias_percent * BIAS_SCALE;
        let comp = compensation(gain);
        let trim = db_to_gain(self.output_db);
        let shape = self.character;
        let engaged = self.is_engaged();
        let factor = FACTOR.multiplier();
        let scratch_len = (frames * factor).min(self.scratch.len());
        let dc_coefficient = DcBlocker::coefficient(self.sample_rate);

        for channel in 0..channels {
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }

            if engaged {
                // The drive is applied to the *input*, before the shaper, so a
                // hotter signal sits further up the curve. Scaling the output
                // instead would be a plain gain change with extra steps. The
                // bias joins it here, in the oversampled domain, so the curve
                // sees a genuinely offset signal rather than an offset already
                // band-limited away.
                for index in 0..frames {
                    self.driven[index] = self.dry[index] * gain + bias;
                }
                let nonlinearity = |x: f32| shape.shape(x);
                let written = if self.oversamplers[channel].is_prepared() && scratch_len > 0 {
                    let scratch = &mut self.scratch[..scratch_len];
                    self.oversamplers[channel].process_channel(
                        &self.driven[..frames],
                        &mut self.wet_buf[..frames],
                        scratch,
                        nonlinearity,
                    )
                } else {
                    // The oversampler refused the block. Fall back to the
                    // base-rate curve rather than emitting silence: it aliases,
                    // but a silent block is a fault the user cannot diagnose.
                    0
                };
                if written != frames {
                    for index in 0..frames {
                        self.wet_buf[index] = nonlinearity(self.driven[index]);
                    }
                }
                for sample in self.wet_buf[..frames].iter_mut() {
                    *sample *= comp * trim;
                }
            } else {
                // 0 dB drive with no bias is the identity: skip the shaper
                // entirely, which is also what keeps the reported latency at 0.
                for index in 0..frames {
                    self.wet_buf[index] = self.dry[index] * trim;
                }
            }

            // -- DC block --
            // An asymmetric curve (or any bias) shifts the waveform's mean, and
            // DC on a bus eats headroom and makes the meter read high for no
            // audible reason.
            let dc = &mut self.dc[channel];
            for sample in self.wet_buf[..frames].iter_mut() {
                *sample = dc.process(channel, *sample, dc_coefficient);
            }

            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let wet_sample = self.wet_buf.get(index).copied().unwrap_or(0.0);
                    let dry_sample = self.dry.get(index).copied().unwrap_or(0.0);
                    *out = wet_sample * wet + dry_sample * (1.0 - wet);
                }
            }
        }
    }

    fn reset(&mut self) {
        for blocker in self.dc.iter_mut() {
            blocker.reset();
        }
        for oversampler in self.oversamplers.iter_mut() {
            oversampler.reset();
        }
    }

    fn latency_samples(&self) -> usize {
        // At 0 dB drive with no bias the shaper is skipped, so nothing delays
        // the wet path and PDC must not shift the channel.
        if self.is_engaged() {
            self.oversamplers[0].latency_samples()
        } else {
            0
        }
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
            PARAM_DRIVE => self.drive_db = value,
            PARAM_CHARACTER => {
                if let Some(character) = Character::from_u32(value as u32) {
                    self.character = character;
                }
            }
            PARAM_BIAS => self.bias_percent = value,
            PARAM_OUTPUT => self.output_db = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_DRIVE => Some(self.drive_db),
            PARAM_CHARACTER => Some(self.character.as_u32() as f32),
            PARAM_BIAS => Some(self.bias_percent),
            PARAM_OUTPUT => Some(self.output_db),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::{gain_to_db, sin_poly};
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn make() -> Saturation {
        let mut effect = Saturation::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Runs `blocks` blocks of `chunk` frames with input from `fill`, handing
    /// each output block to `observe`.
    fn run<F, G>(effect: &mut Saturation, blocks: usize, chunk: usize, fill: F, mut observe: G)
    where
        F: Fn(usize, usize) -> f32,
        G: FnMut(usize, &[f32], &[f32]),
    {
        let mut left = alloc::vec![0.0_f32; chunk];
        let mut right = alloc::vec![0.0_f32; chunk];
        for block in 0..blocks {
            for index in 0..chunk {
                let value = fill(block, index);
                left[index] = value;
                right[index] = value;
            }
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, (block * chunk) as i64, 120.0, 960);
                effect.process(&mut buffer, &ctx);
            }
            observe(block, &left, &right);
        }
    }

    /// Drives a sine and returns the settled peak amplitude.
    fn measure(effect: &mut Saturation, hz: f32, frames: usize) -> f32 {
        let chunk = 256;
        let blocks = frames.div_ceil(chunk);
        let mut peak = 0.0_f32;
        run(
            effect,
            blocks,
            chunk,
            |block, index| sin_poly(2.0 * PI * hz * (block * chunk + index) as f32 / SR),
            |block, left, _| {
                // Skip the oversampler's start-up transient.
                if block > 4 {
                    for sample in left {
                        peak = peak.max(sample.abs());
                    }
                }
            },
        );
        peak
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum / samples.len() as f32).sqrt()
    }

    /// Energy above `from_hz`, measured with a Goertzel scan of the output.
    ///
    /// A plain sum over a coarse frequency sweep, which is enough to tell
    /// "aliased" from "clean" without an FFT.
    fn high_band_energy(signal: &[f32], from_hz: f32, to_hz: f32, sample_rate: f32) -> f32 {
        let mut total = 0.0_f32;
        let mut probe = from_hz;
        while probe < to_hz {
            let mut re = 0.0_f32;
            let mut im = 0.0_f32;
            for (n, &sample) in signal.iter().enumerate().skip(512) {
                let phase = 2.0 * PI * probe * n as f32 / sample_rate;
                re += sample * crate::effects::util::dsp::cos_poly(phase);
                im += sample * sin_poly(phase);
            }
            total += re * re + im * im;
            probe += 500.0;
        }
        total
    }

    // -- Structure and contract --

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, super::super::super::registry::KIND_SATURATION);
        assert_eq!(d.key, "saturation");
        assert_eq!(d.label, "Saturation");
        assert_eq!(d.category, EffectCategory::Distortion);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(d.has_latency, "the 4x oversampler delays the wet path");
        assert!(!d.is_analysis_only);
    }

    #[test]
    fn the_parameter_table_is_ordinal_and_complete() {
        let effect = make();
        let table = effect.parameters();
        assert_eq!(table.len(), PARAM_COUNT as usize);
        for (ordinal, spec) in table.iter().enumerate() {
            assert_eq!(spec.address.sub & 0x00FF, ordinal as u16);
            assert!(
                spec.min_value <= spec.default_value && spec.default_value <= spec.max_value,
                "parameter {ordinal} default is outside its range"
            );
            assert!(
                !spec.key.is_empty()
                    && spec.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "parameter {ordinal} key {} is not a stable machine key",
                spec.key
            );
        }
        for (i, a) in table.iter().enumerate() {
            for b in &table[i + 1..] {
                assert_ne!(a.key, b.key, "duplicate parameter key");
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
            if sub == PARAM_CHARACTER {
                assert_eq!(read, midpoint.floor(), "character snaps to an integer");
            } else {
                assert!(
                    (read - midpoint).abs() < 1e-3,
                    "parameter {sub} read back {read}, expected {midpoint}"
                );
            }
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_and_nan_falls_back() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 1e9);
        assert_eq!(effect.get_parameter(PARAM_DRIVE), Some(36.0));
        effect.set_parameter(PARAM_DRIVE, -1e9);
        assert_eq!(effect.get_parameter(PARAM_DRIVE), Some(0.0));
        effect.set_parameter(PARAM_DRIVE, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_DRIVE), Some(0.0));
    }

    #[test]
    fn unknown_parameter_ordinals_are_ignored() {
        let mut effect = make();
        let before = effect.get_parameter(PARAM_MIX);
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(PARAM_MIX), before);
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut channel = alloc::vec![0.75_f32; 256];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert_eq!(channel, expected);
    }

    #[test]
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 24.0);
        effect.set_wet(0.0);
        let mut channel = alloc::vec![0.5_f32; 256];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        for (i, (got, want)) in channel.iter().zip(expected.iter()).enumerate() {
            assert!((got - want).abs() < 1e-6, "sample {i}: {got} vs {want}");
        }
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_rather_than_overrunning() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 24.0);
        let mut channel = alloc::vec![0.5_f32; 512];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 512, 0, 120.0, 960));
        }
        assert_eq!(channel, expected, "oversized block must be a safe no-op");
    }

    #[test]
    fn non_finite_input_never_reaches_the_output() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 24.0);
        let mut channel = alloc::vec![f32::NAN, 1.0, f32::INFINITY, -1.0, 0.5];
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 5, 0, 120.0, 960));
        }
        for (i, sample) in channel.iter().enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
    }

    #[test]
    fn output_stays_finite_with_every_parameter_at_its_maximum() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, effect.table[sub as usize].max_value);
        }
        run(
            &mut effect,
            32,
            256,
            |_, index| if index % 2 == 0 { 1.0 } else { -1.0 },
            |block, left, _| {
                for (i, sample) in left.iter().enumerate() {
                    assert!(sample.is_finite(), "block {block} sample {i} is {sample}");
                    assert!(
                        sample.abs() <= 8.0,
                        "block {block} sample {i} exploded to {sample}"
                    );
                }
            },
        );
    }

    #[test]
    fn reset_clears_the_dc_blocker_and_oversampler_state() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 24.0);
        run(&mut effect, 4, 256, |_, _| 1.0, |_, _, _| {});
        effect.reset();
        // After a reset the first sample of a constant input must pass through
        // the DC blocker untouched, which is only true if its history is clear.
        assert_eq!(effect.dc[0].process(0, 1.0, 0.99), 1.0);
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
    }

    #[test]
    fn character_discriminants_are_abi_frozen() {
        // These values cross the ABI in a project file; renumbering one would
        // silently load a different curve into the user's session.
        assert_eq!(Character::Soft.as_u32(), 0);
        assert_eq!(Character::Hard.as_u32(), 1);
        assert_eq!(Character::Asymmetric.as_u32(), 2);
        assert_eq!(Character::Fold.as_u32(), 3);
        assert_eq!(Character::from_u32(4), None);
        assert_eq!(Character::from_u32(99), None);
        assert!(Character::Asymmetric.is_asymmetric());
        assert!(!Character::Soft.is_asymmetric());
    }

    // -- Latency --

    #[test]
    fn latency_is_zero_at_zero_drive() {
        // With no drive and no bias the nonlinearity is the identity, so the
        // oversampler is skipped and nothing delays the wet path.
        let mut effect = make();
        assert_eq!(effect.latency_samples(), 0, "0 dB drive bypasses the shaper");
        effect.set_parameter(PARAM_DRIVE, 12.0);
        assert_eq!(
            effect.latency_samples(),
            Oversampler::new(FACTOR).latency_samples(),
            "a driven saturator must report its oversampling delay for PDC"
        );
        effect.set_parameter(PARAM_DRIVE, 0.0);
        assert_eq!(effect.latency_samples(), 0);
    }

    #[test]
    fn a_bias_alone_engages_the_shaper_and_its_latency() {
        // Even at 0 dB drive, a bias makes the curve non-trivial, so the
        // oversampler must run and be reported.
        let mut effect = make();
        assert_eq!(effect.latency_samples(), 0);
        effect.set_parameter(PARAM_BIAS, 40.0);
        assert_eq!(
            effect.latency_samples(),
            Oversampler::new(FACTOR).latency_samples()
        );
    }

    // -- The transfer curves --

    #[test]
    fn the_soft_curve_is_odd_and_monotonic() {
        for step in -100..=100 {
            let x = step as f32 * 0.1;
            let y = Character::Soft.shape(x);
            assert!(y.is_finite());
            assert!(y.abs() <= 1.0, "tanh({x}) escaped the rails: {y}");
            // Odd symmetry: the curve has no even harmonics and no DC.
            assert!(
                (Character::Soft.shape(-x) + y).abs() < 1e-4,
                "tanh is not odd at {x}"
            );
        }
        // Monotonic: a saturator that folded back would be a fold, not a
        // saturator.
        let mut previous = -2.0_f32;
        for step in -200..=200 {
            let y = Character::Soft.shape(step as f32 * 0.1);
            assert!(y >= previous - 1e-5, "tanh decreased at {step}");
            previous = y;
        }
    }

    #[test]
    fn the_hard_curve_is_a_flat_clip_at_the_rails() {
        assert_eq!(Character::Hard.shape(0.5), 0.5);
        assert_eq!(Character::Hard.shape(-0.5), -0.5);
        assert_eq!(Character::Hard.shape(1.0), 1.0);
        assert_eq!(Character::Hard.shape(3.0), 1.0);
        assert_eq!(Character::Hard.shape(-3.0), -1.0);
    }

    #[test]
    fn the_asymmetric_curve_behaves_differently_on_each_side_of_zero() {
        // That asymmetry is the whole reason this curve exists: it is the only
        // one that generates even harmonics.
        let positive = Character::Asymmetric.shape(0.5);
        let negative = Character::Asymmetric.shape(-0.5);
        assert!(
            (positive + negative).abs() > 0.05,
            "the asymmetric curve is symmetric after all: {positive} vs {negative}"
        );
    }

    #[test]
    fn the_fold_curve_reflects_rather_than_wrapping() {
        // The fold must be *continuous* everywhere. A modulo implementation
        // jumps at the wrap point, which is the bug this test exists to catch:
        // sweep the input finely and require that no step changes the output by
        // more than the step itself (plus a small tolerance).
        let mut previous = Character::Fold.shape(-4.0);
        let step = 0.001_f32;
        let mut x = -4.0_f32;
        let mut worst = 0.0_f32;
        while x < 4.0 {
            x += step;
            let y = Character::Fold.shape(x);
            assert!(y.is_finite(), "fold({x}) is {y}");
            assert!(y.abs() <= 1.0 + 1e-4, "fold({x}) escaped the rails: {y}");
            let jump = (y - previous).abs();
            worst = worst.max(jump);
            previous = y;
        }
        // A slope of at most 1 means a step of `step` moves the output by at
        // most `step`; a modulo wrap would show a jump of ~2.0 here.
        assert!(
            worst < step * 4.0,
            "the fold is discontinuous: a {step} input step moved the output by {worst}"
        );
        // And it really does fold: a large input comes back *inside* the rails.
        assert!(Character::Fold.shape(2.5).abs() <= 1.0);
        assert!(Character::Fold.shape(10.0).abs() <= 1.0);
        assert!(Character::Fold.shape(-10.0).abs() <= 1.0);
    }

    #[test]
    fn the_fold_actually_folds_back_instead_of_saturating() {
        // Past the rail the fold's output must *decrease* as the input grows,
        // which is what distinguishes it from a clip.
        let a = Character::Fold.shape(1.5);
        let b = Character::Fold.shape(2.0);
        assert!(
            b < a,
            "the fold did not turn around: shape(1.5) = {a}, shape(2.0) = {b}"
        );
    }

    // -- Unity at 0 dB drive --

    #[test]
    fn zero_drive_is_unity_for_a_small_signal() {
        // "Saturation must not add gain when drive is 0 dB." Measured through
        // the real `process`, not against the transfer function: both coming
        // from the same wrong assumption would agree and prove nothing.
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 0.0);
        effect.set_parameter(PARAM_CHARACTER, Character::Soft.as_u32() as f32);
        effect.set_wet(1.0);
        let peak = measure(&mut effect, 200.0, 4_096);
        assert!(
            (peak - 1.0).abs() < 0.01,
            "a unit sine at 0 dB drive came out at {peak}"
        );
    }

    #[test]
    fn zero_drive_passes_a_signal_through_sample_for_sample() {
        // Stronger than the peak check: with the shaper disengaged the output
        // must equal the input exactly (the DC blocker settles and the
        // oversampler is skipped).
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 0.0);
        effect.set_wet(1.0);
        let input: alloc::vec::Vec<f32> = (0..256)
            .map(|n| sin_poly(2.0 * PI * 500.0 * n as f32 / SR) * 0.4)
            .collect();
        let mut got = alloc::vec![0.0_f32; 256];
        run(
            &mut effect,
            1,
            256,
            |_, index| input[index],
            |_, left, _| got.copy_from_slice(left),
        );
        let mut worst = 0.0_f32;
        for (i, want) in input.iter().enumerate() {
            worst = worst.max((got[i] - want).abs());
        }
        assert!(
            worst < 1e-3,
            "0 dB drive changed the signal by {worst} per sample"
        );
    }

    // -- Gain compensation --

    #[test]
    fn raising_drive_does_not_wildly_change_the_output_level() {
        // Without compensation, 24 dB of drive on a hard clipper turns a -20 dB
        // signal into a +4 dB one: a 24 dB swing that makes A/B meaningless.
        // The level across the drive range must stay within a few dB.
        let level_db = |drive: f32, character: Character| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, drive);
            effect.set_parameter(PARAM_CHARACTER, character.as_u32() as f32);
            effect.set_wet(1.0);
            // A -12 dB input, so the signal sits on the curve's knee rather
            // than being buried in its linear region.
            let mut peak = 0.0_f32;
            run(
                &mut effect,
                24,
                256,
                |block, index| {
                    sin_poly(2.0 * PI * 300.0 * (block * 256 + index) as f32 / SR) * 0.25
                },
                |block, left, _| {
                    if block > 6 {
                        for sample in left {
                            peak = peak.max(sample.abs());
                        }
                    }
                },
            );
            gain_to_db(peak)
        };

        for character in [Character::Soft, Character::Hard, Character::Fold] {
            let quiet = level_db(0.0, character);
            for drive in [6.0_f32, 12.0, 18.0, 24.0, 36.0] {
                let loud = level_db(drive, character);
                assert!(
                    (loud - quiet).abs() < 8.0,
                    "{character:?} at {drive} dB drive moved the level from {quiet} to {loud} dB"
                );
            }
        }
    }

    #[test]
    fn the_manual_output_trim_shifts_the_level_by_what_it_says() {
        let level_db = |trim: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, 12.0);
            effect.set_parameter(PARAM_OUTPUT, trim);
            effect.set_wet(1.0);
            let mut peak = 0.0_f32;
            run(
                &mut effect,
                16,
                256,
                |block, index| {
                    sin_poly(2.0 * PI * 300.0 * (block * 256 + index) as f32 / SR) * 0.25
                },
                |block, left, _| {
                    if block > 6 {
                        for sample in left {
                            peak = peak.max(sample.abs());
                        }
                    }
                },
            );
            gain_to_db(peak)
        };
        let unity = level_db(0.0);
        let boosted = level_db(6.0);
        assert!(
            (boosted - unity - 6.0).abs() < 0.5,
            "a +6 dB output trim moved the level by {} dB",
            boosted - unity
        );
    }

    #[test]
    fn the_automatic_compensation_is_exactly_unity_below_zero_drive() {
        assert_eq!(compensation(1.0), 1.0);
        assert_eq!(compensation(0.5), 1.0);
        // And it decreases as the drive grows, which is its whole purpose.
        let mut previous = 1.0_f32;
        for step in 1..=12 {
            let gain = db_to_gain(step as f32 * 3.0);
            let comp = compensation(gain);
            assert!(comp < previous, "compensation grew at {step}");
            previous = comp;
        }
    }

    // -- DC blocking --

    #[test]
    fn a_constant_input_does_not_leave_a_dc_offset() {
        // A bias shifts the waveform's mean and the asymmetric curve generates
        // an offset even without one. Both must be removed.
        for (character, bias) in [
            (Character::Asymmetric, 0.0_f32),
            (Character::Soft, 60.0),
            (Character::Hard, -40.0),
            (Character::Fold, 30.0),
        ] {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, 12.0);
            effect.set_parameter(PARAM_BIAS, bias);
            effect.set_parameter(PARAM_CHARACTER, character.as_u32() as f32);
            effect.set_wet(1.0);

            // Feed a constant, then look at the settled output.
            let chunk = 256;
            let mut last = alloc::vec![0.0_f32; chunk];
            let blocks = 400; // over two seconds: far past the 5 Hz corner.
            run(
                &mut effect,
                blocks,
                chunk,
                |_, _| 0.3,
                |block, left, _| {
                    if block == blocks - 1 {
                        last.copy_from_slice(left);
                    }
                },
            );
            let offset = rms(&last);
            assert!(
                offset < 1e-2,
                "{character:?} with {bias} % bias left {offset} of DC on a constant input"
            );
        }
    }

    #[test]
    fn the_dc_blocker_does_not_remove_a_mid_band_tone() {
        // The offset above is removed by a 5 Hz high-pass; a 300 Hz tone is two
        // decades above that corner and must survive.
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 12.0);
        effect.set_wet(1.0);
        let peak = measure(&mut effect, 300.0, 24_000);
        assert!(
            peak > 0.2,
            "the DC blocker ate a 300 Hz tone: peak {peak}"
        );
    }

    // -- The aliasing test --

    #[test]
    fn oversampling_reduces_the_folded_energy_of_a_hard_clipped_tone() {
        // The reason the module exists. A 7 kHz tone clipped hard produces
        // harmonics at 21 kHz, 35 kHz, ... and the terms above 24 kHz fold back
        // into the audible band. Oversampling must measurably reduce that.
        //
        // The comparison is between the real effect (oversampled) and a
        // *locally computed* base-rate reference: applying the same transfer
        // function without upsampling. Mirroring the approach in
        // `oversampling.rs::a_hard_clip_at_the_base_rate_aliases_less_when_oversampled`,
        // but through this effect's own `process`.
        let sr = SR;
        let freq = 7_000.0_f32;
        let frames = 2_048;

        let input: alloc::vec::Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * PI * freq * n as f32 / sr) * 0.95)
            .collect();

        // The reference: the same curve, applied at the base rate with the same
        // drive, with no upsampling in between.
        let drive_gain = db_to_gain(20.0);
        let reference: alloc::vec::Vec<f32> = input
            .iter()
            .map(|&x| Character::Hard.shape(x * drive_gain) * compensation(drive_gain))
            .collect();

        // The real thing, through `process`.
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 20.0);
        effect.set_parameter(PARAM_CHARACTER, Character::Hard.as_u32() as f32);
        effect.set_wet(1.0);
        let mut oversampled = alloc::vec![0.0_f32; frames];
        run(
            &mut effect,
            1,
            frames,
            |_, index| input[index],
            |_, left, _| oversampled.copy_from_slice(left),
        );

        // Compare energy above 12 kHz, well into the folded region.
        let aliased = high_band_energy(&reference, 13_000.0, 23_000.0, sr);
        let clean = high_band_energy(&oversampled, 13_000.0, 23_000.0, sr);
        assert!(aliased > 0.0, "the reference produced no folded energy");
        assert!(
            clean < aliased,
            "oversampling did not reduce folded energy: {clean} vs {aliased}"
        );
        // A meaningful improvement, not a rounding difference: the half-band
        // filter should remove most of the folded energy.
        assert!(
            clean < aliased * 0.5,
            "oversampling only reduced folded energy from {aliased} to {clean}"
        );
    }

    #[test]
    fn the_soft_curve_aliases_less_than_the_hard_one() {
        // A smooth curve generates a decaying harmonic series while a hard clip
        // generates one that decays as 1/n, so the soft curve must fold less at
        // the same drive. This is a property of the curves, measured through
        // the real effect.
        let folded = |character: Character| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, 20.0);
            effect.set_parameter(PARAM_CHARACTER, character.as_u32() as f32);
            effect.set_wet(1.0);
            let frames = 2_048;
            let mut out = alloc::vec![0.0_f32; frames];
            run(
                &mut effect,
                1,
                frames,
                |_, index| {
                    sin_poly(2.0 * PI * 7_000.0 * index as f32 / SR) * 0.95
                },
                |_, left, _| out.copy_from_slice(left),
            );
            high_band_energy(&out, 13_000.0, 23_000.0, SR)
        };
        let soft = folded(Character::Soft);
        let hard = folded(Character::Hard);
        assert!(
            soft < hard,
            "the soft curve folded more than the hard one: {soft} vs {hard}"
        );
    }

    #[test]
    fn the_character_control_actually_changes_the_harmonic_content() {
        // Four curves that all sounded the same would make the control a lie.
        // Measured as the total harmonic energy of a 1 kHz tone.
        let harmonic_energy = |character: Character| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, 18.0);
            effect.set_parameter(PARAM_CHARACTER, character.as_u32() as f32);
            effect.set_wet(1.0);
            let frames = 4_096;
            let mut out = alloc::vec![0.0_f32; frames];
            run(
                &mut effect,
                16,
                256,
                |block, index| {
                    sin_poly(2.0 * PI * 1_000.0 * (block * 256 + index) as f32 / SR) * 0.7
                },
                |block, left, _| {
                    out[block * 256..block * 256 + 256].copy_from_slice(left);
                },
            );
            out.truncate(frames);
            // Everything above the third harmonic is distortion.
            high_band_energy(&out, 3_500.0, 20_000.0, SR)
        };
        let soft = harmonic_energy(Character::Soft);
        let hard = harmonic_energy(Character::Hard);
        let fold = harmonic_energy(Character::Fold);
        assert!(soft > 0.0 && hard > 0.0 && fold > 0.0);
        assert!(
            hard > soft * 1.5,
            "the hard clip ({hard}) was not noticeably dirtier than tanh ({soft})"
        );
        assert!(
            fold > soft,
            "the fold ({fold}) was not dirtier than tanh ({soft})"
        );
    }

    #[test]
    fn the_asymmetric_curve_generates_even_harmonics_that_the_soft_one_does_not() {
        // The audible difference between the two is the second harmonic, so
        // measure exactly that: drive a 1 kHz tone and compare the energy at
        // 2 kHz.
        let second_harmonic = |character: Character| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, 18.0);
            effect.set_parameter(PARAM_CHARACTER, character.as_u32() as f32);
            effect.set_wet(1.0);
            let frames = 4_096;
            let mut out = alloc::vec![0.0_f32; frames];
            run(
                &mut effect,
                16,
                256,
                |block, index| {
                    sin_poly(2.0 * PI * 1_000.0 * (block * 256 + index) as f32 / SR) * 0.5
                },
                |block, left, _| {
                    out[block * 256..block * 256 + 256].copy_from_slice(left);
                },
            );
            out.truncate(frames);
            // The settled half only, so the start-up transient is not counted.
            high_band_energy(&out[2_048..], 1_990.0, 2_010.0, SR)
        };
        let soft = second_harmonic(Character::Soft);
        let asymmetric = second_harmonic(Character::Asymmetric);
        assert!(
            asymmetric > soft * 5.0,
            "the asymmetric curve's second harmonic ({asymmetric}) was not above the soft one's ({soft})"
        );
    }

    #[test]
    fn a_bias_alone_generates_even_harmonics_through_an_otherwise_odd_curve() {
        // Biasing `tanh` breaks its odd symmetry, which is the point of the
        // control: it is how a user gets even harmonics from a curve that has
        // none.
        let second_harmonic = |bias: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, 18.0);
            effect.set_parameter(PARAM_CHARACTER, Character::Soft.as_u32() as f32);
            effect.set_parameter(PARAM_BIAS, bias);
            effect.set_wet(1.0);
            let frames = 4_096;
            let mut out = alloc::vec![0.0_f32; frames];
            run(
                &mut effect,
                16,
                256,
                |block, index| {
                    sin_poly(2.0 * PI * 1_000.0 * (block * 256 + index) as f32 / SR) * 0.4
                },
                |block, left, _| {
                    out[block * 256..block * 256 + 256].copy_from_slice(left);
                },
            );
            out.truncate(frames);
            high_band_energy(&out[2_048..], 1_990.0, 2_010.0, SR)
        };
        let centred = second_harmonic(0.0);
        let biased = second_harmonic(70.0);
        assert!(
            biased > centred * 5.0,
            "bias did not add a second harmonic: {biased} vs {centred}"
        );
    }

    #[test]
    fn the_drive_control_makes_the_effect_dirtier() {
        // The most basic property of the effect: turning the drive up must add
        // harmonics, not just level.
        let distortion = |drive: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DRIVE, drive);
            effect.set_parameter(PARAM_CHARACTER, Character::Soft.as_u32() as f32);
            effect.set_wet(1.0);
            let frames = 4_096;
            let mut out = alloc::vec![0.0_f32; frames];
            run(
                &mut effect,
                16,
                256,
                |block, index| {
                    sin_poly(2.0 * PI * 1_000.0 * (block * 256 + index) as f32 / SR) * 0.5
                },
                |block, left, _| {
                    out[block * 256..block * 256 + 256].copy_from_slice(left);
                },
            );
            out.truncate(frames);
            high_band_energy(&out[2_048..], 3_000.0, 20_000.0, SR)
        };
        let clean = distortion(0.0);
        let driven = distortion(24.0);
        assert!(
            driven > clean * 100.0,
            "24 dB of drive only moved the harmonic energy from {clean} to {driven}"
        );
    }

    #[test]
    fn a_silent_input_produces_a_silent_output() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 30.0);
        effect.set_wet(1.0);
        run(&mut effect, 8, 256, |_, _| 0.0, |block, left, right| {
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(
                    sample.abs() < 1e-6,
                    "block {block} sample {i} is {sample} with no input"
                );
            }
        });
    }

    #[test]
    fn a_mono_block_is_processed_without_panicking() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 12.0);
        effect.set_wet(1.0);
        let mut channel = alloc::vec![0.5_f32; 256];
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        for (i, sample) in channel.iter().enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
    }

    #[test]
    fn stereo_channels_do_not_leak_into_each_other() {
        // Per-channel DC blockers and oversamplers: a driven left with a silent
        // right must leave the right silent.
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 24.0);
        effect.set_parameter(PARAM_BIAS, 50.0);
        effect.set_wet(1.0);
        let chunk = 256;
        let mut left = alloc::vec![0.6_f32; chunk];
        let mut right = alloc::vec![0.0_f32; chunk];
        {
            let mut views = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, chunk, 0, 120.0, 960));
        }
        for (i, sample) in right.iter().enumerate() {
            assert!(
                sample.abs() < 1e-6,
                "the silent right channel picked up {sample} at sample {i}"
            );
        }
    }

    #[test]
    fn the_transfer_function_matches_what_process_does() {
        // The public `transfer` helper and the audio path must agree, or the
        // documentation of the curve is describing something the effect does
        // not do. Compared at 0 dB drive with the shaper disengaged? No - with
        // drive engaged and a *constant* input, where the oversampler is
        // transparent once settled.
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 6.0);
        effect.set_parameter(PARAM_CHARACTER, Character::Hard.as_u32() as f32);
        effect.set_wet(1.0);
        let expected = effect.transfer(0.4);
        let mut got = 0.0_f32;
        run(
            &mut effect,
            200,
            256,
            |_, _| 0.4,
            |block, left, _| {
                if block == 199 {
                    got = left[128];
                }
            },
        );
        assert!(
            (got - expected).abs() < 2e-3,
            "process produced {got} where the transfer function promised {expected}"
        );
    }
}


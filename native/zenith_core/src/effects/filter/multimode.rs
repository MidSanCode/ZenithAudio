//! Multimode filter: an insert effect wrapping [`Biquad`].
//!
//! PLAN §3.S5 calls for "滤波器（多模，含共振与包络跟随）". The modes, resonance
//! and the envelope follower all live here; the biquad math they configure is
//! shared with the EQ so the two cannot drift apart.
//!
//! # Envelope following
//!
//! The classic use of a resonant filter is a wah: the cutoff tracks the
//! input's loudness. The follower runs on the **pre-filter** signal, otherwise
//! the filter would chase its own output and latch onto whichever mode it
//! happens to be in.
//!
//! # Drive
//!
//! A filter pushed into resonance is usually driven into saturation as well.
//! `drive_db` applies gain before the filter and compensating gain after it,
//! through a `tanh` shaper running at 2x so the added harmonics do not alias.
//! The oversampling delay is reported from [`EffectProcessor::latency_samples`]
//! so PDC can align the channel.
//!
//! # Real-time safety
//!
//! Every buffer this effect touches is allocated in [`MultimodeFilter::prepare`]
//! and stored on the struct. `process` allocates nothing: the dry snapshot, the
//! wet working copy and the oversampling scratch are all preallocated slices.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{db_to_gain, one_pole_coeff, powf, tanh_poly, DcBlocker};
use super::super::util::{Oversampler, OversamplingFactor};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use super::biquad::{Biquad, FilterMode};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Parameter ordinals, published in this order.
pub const PARAM_MODE: u16 = 0;
/// Cutoff in hertz.
pub const PARAM_CUTOFF: u16 = 1;
/// Resonance (Q).
pub const PARAM_RESONANCE: u16 = 2;
/// Input drive in decibels.
pub const PARAM_DRIVE: u16 = 3;
/// Envelope follower depth, percent, bipolar.
pub const PARAM_ENV_AMOUNT: u16 = 4;
/// Envelope follower attack in milliseconds.
pub const PARAM_ENV_ATTACK: u16 = 5;
/// Envelope follower release in milliseconds.
pub const PARAM_ENV_RELEASE: u16 = 6;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 7;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 8;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// Oversampling factor for the drive stage.
const DRIVE_OVERSAMPLING: OversamplingFactor = OversamplingFactor::X2;

/// The effect's static description. `kind` is filled from the registry so the
/// two cannot disagree.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_FILTER_MULTIMODE,
    key: "multimode_filter",
    label: "Multimode Filter",
    category: EffectCategory::Filter,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: true,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
///
/// A function rather than a `static`: descriptors carry the slot's automation
/// address, so two filters on different channels must publish different
/// addresses or they would share one automation lane.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let slot = address.effect_slot();
    let channel = address.index;
    let at = |sub: u16| ParameterAddress::effect(channel, slot, sub);
    [
        ParameterDescriptor {
            address: at(PARAM_MODE),
            key: "mode",
            label: "Mode",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE | parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 7.0,
            default_value: FilterMode::LowPass.as_u32() as f32,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_CUTOFF),
            key: "cutoff_hz",
            label: "Cutoff",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: 20.0,
            max_value: 20_000.0,
            default_value: 1_000.0,
            smoothing_ms: 10.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RESONANCE),
            key: "resonance",
            label: "Resonance",
            unit: ParameterUnit::Linear,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.1,
            max_value: 20.0,
            default_value: 0.707,
            smoothing_ms: 10.0,
        },
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
            address: at(PARAM_ENV_AMOUNT),
            key: "env_amount",
            label: "Env Amount",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::BIPOLAR
                | parameter_flags::SMOOTHED,
            min_value: -100.0,
            max_value: 100.0,
            default_value: 0.0,
            smoothing_ms: 10.0,
        },
        ParameterDescriptor {
            address: at(PARAM_ENV_ATTACK),
            key: "env_attack_ms",
            label: "Env Attack",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.1,
            max_value: 500.0,
            default_value: 10.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_ENV_RELEASE),
            key: "env_release_ms",
            label: "Env Release",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 1.0,
            max_value: 2_000.0,
            default_value: 120.0,
            smoothing_ms: 0.0,
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
            smoothing_ms: 10.0,
        },
    ]
}

/// The saturating waveshaper the drive stage uses.
///
/// Exposed so the distortion effects can share one definition of "tanh" rather
/// than each writing its own approximation with a slightly different knee.
#[must_use]
pub fn shape(x: f32) -> f32 {
    tanh_poly(x)
}

/// One channel's filter state.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelState {
    /// The audio filter.
    filter: Biquad,
    /// Envelope follower value, linear.
    envelope: f32,
    /// DC blocker on the driven path.
    dc: DcBlocker,
}

/// The multimode filter effect.
#[derive(Debug)]
pub struct MultimodeFilter {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Mode.
    mode: FilterMode,
    /// Cutoff in hertz, as set by the user.
    cutoff_hz: f32,
    /// Resonance (Q).
    resonance: f32,
    /// Drive in decibels.
    drive_db: f32,
    /// Envelope depth in percent.
    env_amount: f32,
    /// Envelope attack time in milliseconds.
    env_attack_ms: f32,
    /// Envelope release time in milliseconds.
    env_release_ms: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel state.
    channels: [ChannelState; MAX_CHANNELS],
    /// Smoothed cutoff actually applied this block, in hertz.
    smoothed_cutoff: f32,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Oversampler for the drive stage, one per channel.
    oversamplers: [Oversampler; MAX_CHANNELS],
    /// Preallocated oversampling scratch, `max_block * factor`.
    scratch: alloc::vec::Vec<f32>,
    /// Preallocated snapshot of the dry input, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer, `max_block`.
    wet_buf: alloc::vec::Vec<f32>,
    /// How many channels are active.
    active_channels: usize,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for MultimodeFilter {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_CUTOFF))
    }
}

impl MultimodeFilter {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        let default_cutoff = table[PARAM_CUTOFF as usize].default_value;
        Self {
            table,
            mode: FilterMode::LowPass,
            cutoff_hz: default_cutoff,
            resonance: 0.707,
            drive_db: 0.0,
            env_amount: 0.0,
            env_attack_ms: 10.0,
            env_release_ms: 120.0,
            mix_percent: 100.0,
            channels: [ChannelState::default(); MAX_CHANNELS],
            smoothed_cutoff: default_cutoff,
            wet: 1.0,
            bypassed: false,
            sample_rate: 48_000.0,
            oversamplers: [
                Oversampler::new(DRIVE_OVERSAMPLING),
                Oversampler::new(DRIVE_OVERSAMPLING),
            ],
            scratch: alloc::vec::Vec::new(),
            dry: alloc::vec::Vec::new(),
            wet_buf: alloc::vec::Vec::new(),
            active_channels: MAX_CHANNELS,
            max_block: 0,
        }
    }

    /// Redesigns `channel`'s biquad for `cutoff`.
    fn redesign(&mut self, channel: usize, cutoff: f32) {
        self.channels[channel]
            .filter
            .design(self.mode, cutoff, self.resonance, 0.0, self.sample_rate);
    }

    /// The cutoff the envelope follower currently asks for.
    fn effective_cutoff(&self) -> f32 {
        let envelope = self.channels[0].envelope.min(1.0);
        if self.env_amount.abs() < 1e-6 {
            return self.cutoff_hz;
        }
        // A depth of 100% opens the filter by four octaves at full level:
        // enough range for a wah without turning into a pitch sweep.
        let octaves = (self.env_amount / 100.0) * 4.0 * envelope;
        self.cutoff_hz * powf(2.0, octaves)
    }
}

impl EffectProcessor for MultimodeFilter {
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
        let factor = DRIVE_OVERSAMPLING.multiplier();
        self.dry = alloc::vec![0.0; max_block];
        self.wet_buf = alloc::vec![0.0; max_block];
        self.scratch = alloc::vec![0.0; max_block * factor];

        for (index, oversampler) in self.oversamplers.iter_mut().enumerate() {
            if index < self.active_channels {
                oversampler.configure(DRIVE_OVERSAMPLING, self.sample_rate, max_block);
            }
        }
        self.smoothed_cutoff = self.cutoff_hz;
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
        // Refuse a block larger than `prepare` sized for rather than indexing
        // past the scratch. A silent pass-through is a far better failure than
        // an out-of-bounds write in the audio thread.
        if frames > self.max_block || frames > self.dry.len() || frames > self.wet_buf.len() {
            return;
        }

        let block_ms = ctx.block_ms();
        // Cutoff smoothing needs the *effective* cutoff, which the envelope
        // changes per block; recomputing from the user value each block and
        // filtering it is what makes a sweep continuous instead of stepped.
        let cutoff_coeff = one_pole_coeff(10.0, block_ms);
        let attack = one_pole_coeff(self.env_attack_ms, block_ms);
        let release = one_pole_coeff(self.env_release_ms, block_ms);
        let drive_gain = db_to_gain(self.drive_db);
        // Compensate so engaging drive does not change the perceived level.
        let drive_comp = if self.drive_db > 0.0 {
            1.0 / powf(drive_gain, 0.7)
        } else {
            1.0
        };
        let use_drive = self.drive_db > 0.01;
        let factor = DRIVE_OVERSAMPLING.multiplier();
        let scratch_len = (frames * factor).min(self.scratch.len());
        let wet = self.wet;

        for channel in 0..channels {
            // Snapshot the dry signal: the mix needs it, and the follower must
            // see the input rather than the filter output.
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }

            // ── Envelope follow (block rate) ──
            let peak = self.dry[..frames]
                .iter()
                .fold(0.0_f32, |m, s| m.max(s.abs()));
            let mut envelope = self.channels[channel].envelope;
            let coefficient = if peak > envelope { attack } else { release };
            envelope += (peak - envelope) * coefficient;
            self.channels[channel].envelope = envelope;

            // ── Cutoff, with envelope offset ──
            let target = {
                let depth = self.env_amount / 100.0;
                if depth.abs() < 1e-6 {
                    self.cutoff_hz
                } else {
                    self.cutoff_hz * powf(2.0, depth * 4.0 * envelope.min(1.0))
                }
            };
            let smoothed = self.smoothed_cutoff + (target - self.smoothed_cutoff) * cutoff_coeff;
            let cutoff = smoothed.clamp(20.0, self.sample_rate * 0.45);
            self.smoothed_cutoff = cutoff;
            self.redesign(channel, cutoff);

            // ── Drive, oversampled ──
            if use_drive {
                let grain = drive_gain;
                let comp = drive_comp;
                let written = if self.oversamplers[channel].is_prepared() && scratch_len > 0 {
                    let scratch = &mut self.scratch[..scratch_len];
                    self.oversamplers[channel].process_channel(
                        &self.dry[..frames],
                        &mut self.wet_buf[..frames],
                        scratch,
                        |x| shape(x * grain) * comp,
                    )
                } else {
                    0
                };
                if written != frames {
                    // The oversampler refused the block (too large, or not
                    // prepared). Fall back to the base-rate shaper rather than
                    // emitting a short or silent block.
                    for index in 0..frames {
                        let sample = self.dry[index];
                        self.wet_buf[index] = shape(sample * grain) * comp;
                    }
                }
            } else {
                self.wet_buf[..frames].copy_from_slice(&self.dry[..frames]);
            }

            // ── Filter ──
            for sample in self.wet_buf[..frames].iter_mut() {
                *sample = self.channels[channel].filter.process(*sample);
            }

            // ── DC block ──
            let dc_coeff = DcBlocker::coefficient(self.sample_rate);
            let dc = &mut self.channels[channel].dc;
            for sample in self.wet_buf[..frames].iter_mut() {
                *sample = dc.process(channel, *sample, dc_coeff);
            }

            // ── Wet/dry ──
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
        for state in self.channels.iter_mut() {
            state.filter.reset();
            state.envelope = 0.0;
            state.dc.reset();
        }
        for oversampler in self.oversamplers.iter_mut() {
            oversampler.reset();
        }
        self.smoothed_cutoff = self.cutoff_hz;
    }

    fn latency_samples(&self) -> usize {
        // The drive stage is oversampled, so the wet path carries that delay.
        // Reporting zero would misalign the channel against every other track.
        if self.drive_db > 0.01 {
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
            PARAM_MODE => {
                if let Some(mode) = FilterMode::from_u32(value as u32) {
                    self.mode = mode;
                }
            }
            PARAM_CUTOFF => self.cutoff_hz = value,
            PARAM_RESONANCE => self.resonance = value,
            PARAM_DRIVE => self.drive_db = value,
            PARAM_ENV_AMOUNT => self.env_amount = value,
            PARAM_ENV_ATTACK => self.env_attack_ms = value,
            PARAM_ENV_RELEASE => self.env_release_ms = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_MODE => Some(self.mode.as_u32() as f32),
            PARAM_CUTOFF => Some(self.cutoff_hz),
            PARAM_RESONANCE => Some(self.resonance),
            PARAM_DRIVE => Some(self.drive_db),
            PARAM_ENV_AMOUNT => Some(self.env_amount),
            PARAM_ENV_ATTACK => Some(self.env_attack_ms),
            PARAM_ENV_RELEASE => Some(self.env_release_ms),
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
    use crate::effects::util::dsp::sin_poly;
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn make() -> MultimodeFilter {
        let mut effect = MultimodeFilter::new(ParameterAddress::effect(0, 0, PARAM_CUTOFF));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Runs `frames` samples of a sine through a freshly prepared effect and
    /// returns the settled peak of the output.
    fn measure(effect: &mut MultimodeFilter, hz: f32, frames: usize) -> f32 {
        let chunk = 256;
        let mut peak = 0.0_f32;
        let mut produced = 0;
        while produced < frames {
            let mut channel: alloc::vec::Vec<f32> = (0..chunk)
                .map(|n| sin_poly(2.0 * PI * hz * (produced + n) as f32 / SR))
                .collect();
            {
                let mut views = [&mut channel[..]];
                let mut buf = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, produced as i64, 120.0, 960);
                effect.process(&mut buf, &ctx);
            }
            if produced + chunk > chunk * 4 {
                for sample in &channel {
                    peak = peak.max(sample.abs());
                }
            }
            produced += chunk;
        }
        peak
    }

    /// A block of constant DC, to settle smoothing before a measurement.
    fn settle(effect: &mut MultimodeFilter) {
        for _ in 0..40 {
            let mut channel = alloc::vec![0.0_f32; 64];
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            let ctx = RenderContext::new(SR, 64, 0, 120.0, 960);
            effect.process(&mut buf, &ctx);
        }
        effect.reset();
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.key, "multimode_filter");
        assert_eq!(d.label, "Multimode Filter");
        assert_eq!(d.category, EffectCategory::Filter);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..8);
        assert!(!d.is_analysis_only);
    }

    #[test]
    fn the_parameter_table_is_ordinal_and_complete() {
        let effect = make();
        let table = effect.parameters();
        assert_eq!(table.len(), PARAM_COUNT as usize);
        for (ordinal, spec) in table.iter().enumerate() {
            assert_eq!(
                spec.address.sub & 0x00FF,
                ordinal as u16,
                "parameter {ordinal} has a mismatched address"
            );
            assert!(
                spec.min_value <= spec.default_value && spec.default_value <= spec.max_value,
                "parameter {ordinal} default is outside its range"
            );
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
            if sub == PARAM_MODE {
                assert_eq!(read, midpoint.floor(), "mode snaps to an integer");
            } else {
                assert!(
                    (read - midpoint).abs() < 1e-3,
                    "parameter {sub} read back {read}, expected {midpoint}"
                );
            }
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_not_rejected() {
        let mut effect = make();
        effect.set_parameter(PARAM_CUTOFF, 1e9);
        assert_eq!(effect.get_parameter(PARAM_CUTOFF), Some(20_000.0));
        effect.set_parameter(PARAM_CUTOFF, -1e9);
        assert_eq!(effect.get_parameter(PARAM_CUTOFF), Some(20.0));
        // A NaN must fall back to the default rather than poison the filter.
        effect.set_parameter(PARAM_CUTOFF, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_CUTOFF), Some(1_000.0));
    }

    #[test]
    fn unknown_parameter_ordinals_are_ignored() {
        let mut effect = make();
        let before = effect.get_parameter(PARAM_CUTOFF);
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(PARAM_CUTOFF), before);
        assert_eq!(effect.get_parameter(999), None);
    }

    #[test]
    fn a_low_pass_attenuates_high_frequencies() {
        let mut effect = make();
        effect.set_parameter(PARAM_MODE, FilterMode::LowPass.as_u32() as f32);
        effect.set_parameter(PARAM_CUTOFF, 500.0);
        effect.set_parameter(PARAM_RESONANCE, 0.707);
        effect.set_parameter(PARAM_MIX, 100.0);

        settle(&mut effect);
        let high = measure(&mut effect, 10_000.0, 8_192);
        assert!(high < 0.1, "10 kHz through a 500 Hz low-pass came out at {high}");

        effect.reset();
        settle(&mut effect);
        let low = measure(&mut effect, 100.0, 8_192);
        assert!(low > 0.8, "100 Hz should pass, got {low}");
    }

    #[test]
    fn a_high_pass_blocks_low_frequencies() {
        let mut effect = make();
        effect.set_parameter(PARAM_MODE, FilterMode::HighPass.as_u32() as f32);
        effect.set_parameter(PARAM_CUTOFF, 2_000.0);
        settle(&mut effect);
        let low = measure(&mut effect, 100.0, 8_192);
        assert!(low < 0.1, "100 Hz through a 2 kHz high-pass came out at {low}");
    }

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut channel = alloc::vec![0.75_f32; 256];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::default());
        }
        assert_eq!(channel, expected);
    }

    #[test]
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_CUTOFF, 100.0);
        effect.set_parameter(PARAM_MIX, 0.0);
        let mut channel = alloc::vec![0.5_f32; 512];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, 512, 0, 120.0, 960));
        }
        for (i, (got, want)) in channel.iter().zip(expected.iter()).enumerate() {
            assert!((got - want).abs() < 1e-6, "sample {i}: {got} vs {want}");
        }
    }

    #[test]
    fn output_stays_finite_and_bounded_under_extreme_parameters() {
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, 36.0);
        effect.set_parameter(PARAM_RESONANCE, 20.0);
        effect.set_parameter(PARAM_ENV_AMOUNT, 100.0);
        effect.set_parameter(PARAM_CUTOFF, 200.0);

        let chunk = 256;
        for round in 0..16 {
            let mut channel = alloc::vec![0.9_f32; chunk];
            {
                let mut views = [&mut channel[..]];
                let mut buf = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, (round * chunk) as i64, 120.0, 960);
                effect.process(&mut buf, &ctx);
            }
            for (i, sample) in channel.iter().enumerate() {
                assert!(sample.is_finite(), "round {round} sample {i} is {sample}");
                assert!(
                    sample.abs() <= 4.0,
                    "round {round} sample {i} exploded to {sample}"
                );
            }
        }
    }

    #[test]
    fn a_self_oscillating_resonance_does_not_blow_up() {
        // Q = 20 with a hot input is the worst case for a two-pole filter; the
        // output must stay finite even with no input at all afterwards.
        let mut effect = make();
        effect.set_parameter(PARAM_RESONANCE, 20.0);
        effect.set_parameter(PARAM_CUTOFF, 1_000.0);
        let chunk = 256;
        for round in 0..40 {
            let mut channel = if round < 4 {
                alloc::vec![1.0_f32; chunk]
            } else {
                alloc::vec![0.0_f32; chunk]
            };
            {
                let mut views = [&mut channel[..]];
                let mut buf = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, (round * chunk) as i64, 120.0, 960);
                effect.process(&mut buf, &ctx);
            }
            for sample in &channel {
                assert!(sample.is_finite(), "resonance blew up: {sample}");
            }
        }
    }

    #[test]
    fn reset_clears_filter_and_envelope_state() {
        let mut effect = make();
        effect.set_parameter(PARAM_ENV_AMOUNT, 50.0);
        let mut channel = alloc::vec![1.0_f32; 256];
        {
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert!(effect.channels[0].envelope > 0.0, "follower should have moved");
        effect.reset();
        assert_eq!(effect.channels[0].envelope, 0.0);
        assert_eq!(effect.channels[0].filter.x1, 0.0);
    }

    #[test]
    fn reported_latency_matches_the_drive_setting() {
        let mut effect = make();
        assert_eq!(effect.latency_samples(), 0, "no drive means no oversampling");
        effect.set_parameter(PARAM_DRIVE, 12.0);
        assert_eq!(
            effect.latency_samples(),
            Oversampler::new(DRIVE_OVERSAMPLING).latency_samples(),
            "a driven filter must report its oversampling delay for PDC"
        );
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_rather_than_overrunning() {
        let mut effect = make();
        // Prepare sized for 256; a 512-frame block must pass through untouched
        // instead of indexing past the scratch.
        let mut channel = alloc::vec![0.5_f32; 512];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, 512, 0, 120.0, 960));
        }
        assert_eq!(channel, expected, "oversized block must be a safe no-op");
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
    fn the_envelope_follower_tracks_a_loud_passage_and_decays() {
        let mut effect = make();
        effect.set_parameter(PARAM_ENV_AMOUNT, 50.0);
        effect.set_parameter(PARAM_ENV_ATTACK, 1.0);
        effect.set_parameter(PARAM_ENV_RELEASE, 5.0);

        let chunk = 256;
        // Loud passage.
        for _ in 0..20 {
            let mut channel = alloc::vec![0.8_f32; chunk];
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, chunk, 0, 120.0, 960));
        }
        let loud = effect.channels[0].envelope;
        assert!(loud > 0.3, "follower only reached {loud} on a loud passage");

        // Silence: it must decay, not latch.
        for _ in 0..80 {
            let mut channel = alloc::vec![0.0_f32; chunk];
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, chunk, 0, 120.0, 960));
        }
        let quiet = effect.channels[0].envelope;
        assert!(quiet < loud * 0.5, "follower latched at {quiet} after silence");
    }
}

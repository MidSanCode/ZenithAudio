//! The unified effect interface (PLAN 搂3.S5).
//!
//! Every built-in effect implements [`EffectProcessor`]. The trait is
//! deliberately narrow: it knows about audio blocks, parameters and latency,
//! and nothing about channels, slots or the mixer. That separation is what
//! lets the same processor be a channel insert, a send effect or the contents
//! of a plugin slot without a second implementation.
//!
//! # The prepare/process split
//!
//! [`EffectProcessor::prepare`] runs on the **control thread** and is the only
//! place an effect may allocate: every buffer, delay line, FFT scratch and
//! impulse response is sized there. [`EffectProcessor::process`] runs on the
//! **audio thread** and must not allocate, lock, or perform IO (ABI principle
//! P5). The `assert_no_alloc` test module enforces that rather than trusting
//! it.
//!
//! # Parameters
//!
//! An effect publishes a `'static` table of parameter descriptors. The same
//! table drives three consumers: the audio path's parameter reads, the
//! automation system (S2), and the UI, which generates a panel from the
//! descriptors alone (PLAN 搂3.S5 鈥?no hand-written Dart class per effect).

pub mod buffer;
pub mod dynamics;
pub mod registry;
pub mod util;

pub use buffer::{
    AudioBuffer as EffectBuffer, RenderContext as EffectContext, SampleStorage as EffectScratch,
};
pub use registry::{
    create as create_effect, describe as describe_effect, is_known as is_known_effect,
};
pub use util::{DcBlocker, Oversampler, OversamplerBank, OversamplingFactor};

use crate::automation::parameter::{ParameterDescriptor, ParameterUnit};
use buffer::{AudioBuffer, RenderContext, SampleStorage};

/// Which family an effect belongs to, for UI grouping and meter colouring.
///
/// The discriminant is **not** part of the C ABI yet: `zenith_effect_describe`
/// publishes a `u32` built from [`EffectCategory::as_u32`], and new categories
/// are appended rather than renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffectCategory {
    /// Filters and equalisers.
    Equalizer = 0,
    /// Compressors, limiters and gates.
    Dynamics = 1,
    /// Reverberation and room simulation.
    Reverb = 2,
    /// Delays and echoes.
    Delay = 3,
    /// Chorus, flanger and phaser.
    Modulation = 4,
    /// Saturation, distortion and bit reduction.
    Distortion = 5,
    /// Signal-shaping filters.
    Filter = 6,
    /// Analysis that produces no audio (spectrum).
    Analysis = 7,
    /// Utility processors (gain, oversampling helpers).
    Utility = 8,
}

impl EffectCategory {
    /// The published discriminant.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// The stable machine-readable name.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Equalizer => "equalizer",
            Self::Dynamics => "dynamics",
            Self::Reverb => "reverb",
            Self::Delay => "delay",
            Self::Modulation => "modulation",
            Self::Distortion => "distortion",
            Self::Filter => "filter",
            Self::Analysis => "analysis",
            Self::Utility => "utility",
        }
    }
}

/// Everything the engine needs to know about an effect before creating it.
///
/// Returned by the registry as a `'static` value, so publishing it to Dart
/// costs no allocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectDescriptor {
    /// Kind id, as stored in a mixer effect slot.
    pub kind: u32,
    /// Stable machine-readable key, e.g. `"compressor"`.
    pub key: &'static str,
    /// Human-readable label; the UI localizes as it sees fit.
    pub label: &'static str,
    /// Family, for UI grouping.
    pub category: EffectCategory,
    /// Lower bound of the kind's parameter ordinals.
    pub first_param: u16,
    /// How many parameters the effect publishes.
    pub param_count: u16,
    /// Whether the effect adds latency that PDC must compensate.
    ///
    /// A look-ahead compressor and a convolution reverb report `true`; a
    /// stateless gain stage reports `false`.
    pub has_latency: bool,
    /// Whether this effect is an analyser that passes audio through untouched.
    pub is_analysis_only: bool,
}

impl EffectDescriptor {
    /// Parameter ordinals this effect occupies, as a range.
    #[must_use]
    pub const fn param_range(&self) -> core::ops::Range<u16> {
        self.first_param..self.first_param + self.param_count
    }
}

/// A real-time audio effect.
///
/// Implementors must be `Send` because the control thread prepares them and
/// the audio thread runs them; they are **not** required to be `Sync`, because
/// the engine guarantees a single audio thread touches a given instance.
pub trait EffectProcessor: Send {
    /// Returns the effect's static description.
    fn descriptor(&self) -> &'static EffectDescriptor;

    /// Allocates every buffer this effect will ever need.
    ///
    /// Called on the control thread when the engine starts, when the audio
    /// device changes, or when the block size grows. After this returns,
    /// [`Self::process`] must not allocate for any block up to `max_block`
    /// frames and `channels` channels.
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);

    /// Processes one block in place.
    ///
    /// Real-time safe: no allocation, no locks, no IO.
    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext);

    /// Clears internal state (delay lines, filter history, envelopes).
    ///
    /// Called on seek and on transport stop so a stale reverb tail cannot
    /// bleed into a new playback position.
    fn reset(&mut self);

    /// Latency introduced by this effect, in samples, for PDC.
    ///
    /// Must be accurate: S4 compensates other channels by this amount, and a
    /// wrong value misaligns every track in the project rather than only this
    /// one. Returns `0` for a zero-latency effect.
    fn latency_samples(&self) -> usize;

    /// The effect's own parameter table, in ordinal order.
    ///
    /// Borrowed from the instance rather than `'static` because descriptors
    /// embed an [`crate::automation::parameter::ParameterAddress`], and the
    /// address depends on which slot this instance occupies. A `'static` table
    /// would have to either lie about the slot or force every instance of an
    /// effect to share one address, which would make two compressors on
    /// different channels fight over the same automation lane.
    fn parameters(&self) -> &[ParameterDescriptor];

    /// Sets parameter `sub` to `value`, clamping rather than rejecting.
    ///
    /// Out-of-range values are clamped because they arrive from UI drags,
    /// project files and automation, where "a bit past the end" is normal and
    /// must not be an error. Unknown ordinals are ignored.
    fn set_parameter(&mut self, sub: u16, value: f32);

    /// Reads parameter `sub` back, for UI feedback and automation latch.
    fn get_parameter(&self, sub: u16) -> Option<f32>;

    /// Whether the effect currently passes audio untouched.
    ///
    /// A bypassed effect still exists and keeps its state, so bypass is a
    /// non-destructive A/B rather than a removal.
    fn is_bypassed(&self) -> bool {
        false
    }

    /// Sets the bypass state.
    fn set_bypassed(&mut self, bypassed: bool);

    /// Current wet/dry balance, `0.0` = dry, `1.0` = wet.
    fn wet(&self) -> f32 {
        1.0
    }

    /// Sets the wet/dry balance, clamped to `0.0..=1.0`.
    fn set_wet(&mut self, wet: f32);

    /// A human-readable tail length in seconds, for offline export.
    ///
    /// S4 uses this to know how much extra audio to render past the end of the
    /// project so a reverb tail is not truncated. `0.0` for effects with no tail.
    fn tail_seconds(&self) -> f32 {
        0.0
    }

    /// Mixes `buffer` back toward `dry` according to the wet/dry balance.
    ///
    /// Provided so every effect shades wet/dry identically: an effect whose
    /// wet path is in `buffer` and whose dry path is in `dry` calls this once
    /// at the end of `process` instead of reimplementing the crossfade (and
    /// reimplementing it slightly differently) each time.
    fn mix_wet_dry(&self, buffer: &mut AudioBuffer<'_>, dry: &AudioBuffer<'_>) {
        let wet = self.wet().clamp(0.0, 1.0);
        if wet >= 1.0 {
            return;
        }
        if wet <= 0.0 {
            // Fully dry: restore the original signal exactly.
            let _ = buffer.copy_from(dry);
            return;
        }
        let dry_gain = 1.0 - wet;
        for (dst, dry_ch) in buffer.iter_mut().zip(dry.iter()) {
            for (out, dry_sample) in dst.iter_mut().zip(dry_ch.iter()) {
                *out = *out * wet + *dry_sample * dry_gain;
            }
        }
    }
}

/// Shared clamping helper for effect parameter setters.
///
/// Centralising it keeps the NaN policy in one place: a `NaN` from a corrupt
/// project file must not propagate into filter state, where it would poison
/// the feedback path and silence the bus several blocks later.
#[must_use]
pub fn clamp_parameter(spec: &ParameterDescriptor, value: f32) -> f32 {
    spec.clamp(value)
}

/// Converts a wet/dry value to `0.0..=1.0`, failing safe to fully wet.
#[must_use]
pub fn sanitize_wet(wet: f32) -> f32 {
    if wet.is_nan() {
        1.0
    } else {
        wet.clamp(0.0, 1.0)
    }
}

/// Convenience alias so effect modules can store scratch without repeating the
/// import of [`SampleStorage`].
pub type Scratch = SampleStorage;

/// Unit constant re-exported for effect parameter tables.
pub const UNIT_LINEAR: ParameterUnit = ParameterUnit::Linear;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::parameter::parameter_flags;

    fn descriptor() -> ParameterDescriptor {
        ParameterDescriptor {
            address: crate::automation::parameter::ParameterAddress::effect(0, 0, 0),
            key: "mix",
            label: "Mix",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 100.0,
            smoothing_ms: 10.0,
        }
    }

    #[test]
    fn categories_publish_stable_discriminants() {
        // These values cross the ABI once effects are described to Dart; a
        // renumbering would silently recategorise every panel in the UI.
        assert_eq!(EffectCategory::Equalizer.as_u32(), 0);
        assert_eq!(EffectCategory::Dynamics.as_u32(), 1);
        assert_eq!(EffectCategory::Reverb.as_u32(), 2);
        assert_eq!(EffectCategory::Delay.as_u32(), 3);
        assert_eq!(EffectCategory::Modulation.as_u32(), 4);
        assert_eq!(EffectCategory::Distortion.as_u32(), 5);
        assert_eq!(EffectCategory::Filter.as_u32(), 6);
        assert_eq!(EffectCategory::Analysis.as_u32(), 7);
        assert_eq!(EffectCategory::Utility.as_u32(), 8);
    }

    #[test]
    fn category_keys_are_lowercase_and_distinct() {
        let all = [
            EffectCategory::Equalizer,
            EffectCategory::Dynamics,
            EffectCategory::Reverb,
            EffectCategory::Delay,
            EffectCategory::Modulation,
            EffectCategory::Distortion,
            EffectCategory::Filter,
            EffectCategory::Analysis,
            EffectCategory::Utility,
        ];
        for (i, a) in all.iter().enumerate() {
            assert!(
                a.key().chars().all(|c| c.is_ascii_lowercase()),
                "{:?} key is not lowercase",
                a
            );
            for b in &all[i + 1..] {
                assert_ne!(a.key(), b.key(), "duplicate category key");
            }
        }
    }

    #[test]
    fn a_parameter_range_covers_exactly_the_published_count() {
        let d = EffectDescriptor {
            kind: 1,
            key: "k",
            label: "L",
            category: EffectCategory::Filter,
            first_param: 10,
            param_count: 3,
            has_latency: false,
            is_analysis_only: false,
        };
        assert_eq!(d.param_range(), 10..13);
        assert_eq!(d.param_range().count(), 3);
    }

    #[test]
    fn clamp_parameter_bounds_and_neutralizes_nan() {
        let spec = descriptor();
        assert_eq!(clamp_parameter(&spec, 200.0), 100.0);
        assert_eq!(clamp_parameter(&spec, -5.0), 0.0);
        assert_eq!(clamp_parameter(&spec, 50.0), 50.0);
        // NaN must fall back to the default, never reach DSP state.
        assert_eq!(clamp_parameter(&spec, f32::NAN), 100.0);
    }

    #[test]
    fn sanitize_wet_clamps_and_fails_safe() {
        assert_eq!(sanitize_wet(2.0), 1.0);
        assert_eq!(sanitize_wet(-1.0), 0.0);
        assert_eq!(sanitize_wet(0.25), 0.25);
        assert_eq!(sanitize_wet(f32::NAN), 1.0);
    }
}



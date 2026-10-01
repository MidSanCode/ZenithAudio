//! The effect suite, mirroring `effects/mod.rs` and `effects/registry.rs`.

#[path = "../../../native/zenith_core/src/effects/buffer.rs"]
pub mod buffer;
#[path = "../../../native/zenith_core/src/effects/util/mod.rs"]
pub mod util;
#[path = "../../../native/zenith_core/src/effects/delay/mod.rs"]
pub mod delay;
#[path = "../../../native/zenith_core/src/effects/modulation/mod.rs"]
pub mod modulation;

/// Stub registry with just the kind ids this harness needs.
pub mod registry {
    /// Tempo-synced delay.
    pub const KIND_DELAY_SYNC: u32 = 0x0000_0400;
    /// Chorus.
    pub const KIND_CHORUS: u32 = 0x0000_0500;
    /// Flanger.
    pub const KIND_FLANGER: u32 = 0x0000_0501;
    /// Phaser.
    pub const KIND_PHASER: u32 = 0x0000_0502;
}

use crate::automation::parameter::{ParameterDescriptor, ParameterUnit};

/// Which family an effect belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectCategory {
    /// Filters and equalisers.
    Equalizer = 0,
    /// Compressors, limiters and gates.
    Dynamics = 1,
    /// Reverberation.
    Reverb = 2,
    /// Delays and echoes.
    Delay = 3,
    /// Chorus, flanger and phaser.
    Modulation = 4,
    /// Saturation and distortion.
    Distortion = 5,
    /// Signal-shaping filters.
    Filter = 6,
    /// Analysis.
    Analysis = 7,
    /// Utility processors.
    Utility = 8,
}

/// Everything the engine needs to know about an effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectDescriptor {
    /// Kind id.
    pub kind: u32,
    /// Stable machine-readable key.
    pub key: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// Family.
    pub category: EffectCategory,
    /// First parameter ordinal.
    pub first_param: u16,
    /// How many parameters.
    pub param_count: u16,
    /// Whether PDC applies.
    pub has_latency: bool,
    /// Whether it is analysis-only.
    pub is_analysis_only: bool,
}

impl EffectDescriptor {
    /// The ordinals this effect occupies.
    #[must_use]
    pub const fn param_range(&self) -> core::ops::Range<u16> {
        self.first_param..self.first_param + self.param_count
    }
}

/// A real-time audio effect.
pub trait EffectProcessor: Send {
    /// The static description.
    fn descriptor(&self) -> &'static EffectDescriptor;
    /// Allocates every buffer.
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize);
    /// Processes one block in place.
    fn process(&mut self, buffer: &mut buffer::AudioBuffer<'_>, ctx: &buffer::RenderContext);
    /// Clears internal state.
    fn reset(&mut self);
    /// Latency in samples, for PDC.
    fn latency_samples(&self) -> usize;
    /// The parameter table.
    fn parameters(&self) -> &[ParameterDescriptor];
    /// Sets a parameter, clamping.
    fn set_parameter(&mut self, sub: u16, value: f32);
    /// Reads a parameter back.
    fn get_parameter(&self, sub: u16) -> Option<f32>;
    /// Whether it is bypassed.
    fn is_bypassed(&self) -> bool {
        false
    }
    /// Sets the bypass state.
    fn set_bypassed(&mut self, bypassed: bool);
    /// The wet/dry balance.
    fn wet(&self) -> f32 {
        1.0
    }
    /// Sets the wet/dry balance.
    fn set_wet(&mut self, wet: f32);
    /// The tail length in seconds.
    fn tail_seconds(&self) -> f32 {
        0.0
    }
}

/// Clamps `value` to `spec`'s bounds, mapping `NaN` to the default.
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

/// Re-export so effect modules can name the unit constant.
pub const UNIT_LINEAR: ParameterUnit = ParameterUnit::Linear;

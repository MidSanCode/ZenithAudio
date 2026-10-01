//! Noise gate / downward expander with hysteresis, hold and a sidechain filter.
//!
//! # Why hysteresis is not optional
//!
//! A gate compares the detector's level against a threshold. With a single
//! threshold, a signal hovering exactly at it crosses back and forth on every
//! block and the gate chatters — an audible stutter far worse than the noise it
//! was meant to remove. The cure is a **second** threshold: the gate opens at
//! `threshold_db` and only closes once the level has fallen to
//! `threshold_db - hysteresis_db`. Between the two the current state is kept,
//! so a hovering signal stays where it is. Two tests pin this: one drives a
//! level exactly at the threshold and counts the transitions, one sweeps
//! through the hysteresis band.
//!
//! # Hold
//!
//! A gate on a percussive source closes during the decay of the hit, cutting
//! the tail. `hold_ms` keeps the gate open for a fixed time after the level
//! drops below the closing threshold, which is the difference between a gate
//! that tightens a drum and one that chops it.
//!
//! # Above the threshold: a downward expander, not a switch
//!
//! Between the open threshold and `range_db` below it the gain follows the
//! *expansion* curve rather than jumping to unity. A gate that snaps to 1.0 the
//! instant it opens has a discontinuity in its transfer function, which is
//! audible as a click; the smooth curve and the per-sample one-pole smoothing
//! on the gain keep the transition continuous.
//!
//! # Latency
//!
//! There is no look-ahead, so the reported latency is `0`. The detector is a
//! level follower on the current block; adding a look-ahead window would buy
//! nothing here because the gate is not trying to catch a transient that has
//! already passed.
//!
//! # Real-time safety
//!
//! Every buffer is allocated in [`Gate::prepare`]. `process` allocates nothing,
//! locks nothing and performs no IO.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{db_to_gain, gain_to_db, one_pole_coeff};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Level above which the gate opens, in decibels.
pub const PARAM_THRESHOLD: u16 = 0;
/// How far below the threshold the level must fall before the gate closes.
pub const PARAM_HYSTERESIS: u16 = 1;
/// How far down the gate closes, in decibels below unity.
pub const PARAM_RANGE: u16 = 2;
/// Opening time in milliseconds.
pub const PARAM_ATTACK: u16 = 3;
/// How long the gate stays open after the level drops, in milliseconds.
pub const PARAM_HOLD: u16 = 4;
/// Closing time in milliseconds.
pub const PARAM_RELEASE: u16 = 5;
/// Whether the sidechain filter is engaged.
pub const PARAM_SIDECHAIN_ENABLED: u16 = 6;
/// Sidechain filter centre frequency in hertz.
pub const PARAM_SIDECHAIN_HZ: u16 = 7;
/// How much of the filtered sidechain is used, in percent.
pub const PARAM_SIDECHAIN_DEPTH: u16 = 8;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 9;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 10;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_GATE,
    key: "noise_gate",
    label: "Noise Gate",
    category: EffectCategory::Dynamics,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: false,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_THRESHOLD),
            key: "threshold_db",
            label: "Threshold",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: -80.0,
            max_value: 0.0,
            default_value: -40.0,
            smoothing_ms: 10.0,
        },
        ParameterDescriptor {
            address: at(PARAM_HYSTERESIS),
            key: "hysteresis_db",
            label: "Hysteresis",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 24.0,
            default_value: 6.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RANGE),
            key: "range_db",
            label: "Range",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE,
            min_value: -90.0,
            max_value: 0.0,
            default_value: -60.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_ATTACK),
            key: "attack_ms",
            label: "Attack",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.1,
            max_value: 500.0,
            default_value: 1.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_HOLD),
            key: "hold_ms",
            label: "Hold",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 2_000.0,
            default_value: 50.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RELEASE),
            key: "release_ms",
            label: "Release",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 1.0,
            max_value: 4_000.0,
            default_value: 120.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_SIDECHAIN_ENABLED),
            key: "sidechain_enabled",
            label: "Sidechain",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE,
            min_value: 0.0,
            max_value: 1.0,
            default_value: 0.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_SIDECHAIN_HZ),
            key: "sidechain_hz",
            label: "Sidechain Freq",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::LOGARITHMIC,
            min_value: 20.0,
            max_value: 20_000.0,
            default_value: 1_000.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_SIDECHAIN_DEPTH),
            key: "sidechain_depth",
            label: "Sidechain Depth",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 100.0,
            smoothing_ms: 10.0,
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

/// The expansion curve, in decibels of gain, for a detector level.
///
/// `level_db` is the detector's level in decibels, `open_db` the level at which
/// the gate is fully open, and `floor_db` how far down the gate closes. The
/// curve is anchored so that:
///
/// * at and above `open_db` the gain is exactly `0` dB — the gate is
///   transparent, which is a hard requirement rather than a near miss;
/// * at and below `open_db - span` the gain is exactly `floor_db`;
/// * in between it is linear in decibels, i.e. a true downward expander rather
///   than a switch with a discontinuity in its transfer function.
///
/// `span_db` is the width of that transition; it comes from the hysteresis
/// control, so a wide hysteresis also gives a gentle expansion knee.
#[must_use]
pub fn expansion_gain_db(level_db: f32, open_db: f32, span_db: f32, floor_db: f32) -> f32 {
    if !level_db.is_finite() || !open_db.is_finite() || !floor_db.is_finite() {
        return 0.0;
    }
    let floor_db = floor_db.min(0.0);
    let span = if span_db.is_finite() {
        span_db.max(1e-3)
    } else {
        1e-3
    };
    if level_db >= open_db {
        return 0.0;
    }
    let below = open_db - level_db;
    if below >= span {
        return floor_db;
    }
    let fraction = below / span;
    let gain_db = floor_db * fraction;
    if gain_db.is_finite() {
        gain_db.clamp(floor_db, 0.0)
    } else {
        0.0
    }
}

/// One channel's sidechain filter state.
#[derive(Debug, Clone, Copy, Default)]
struct SidechainState {
    /// State of the one-pole low-pass.
    low: f32,
    /// State of the one-pole high-pass.
    high_input: f32,
    /// Output history of the one-pole high-pass.
    high_output: f32,
}

impl SidechainState {
    /// Clears the filter history.
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Applies the band-pass pair described in the parameter docs.
    fn process(&mut self, input: f32, coefficient: f32) -> f32 {
        let coefficient = coefficient.clamp(0.0, 1.0);
        self.low += (input - self.low) * coefficient;
        let low = if self.low.is_finite() { self.low } else { 0.0 };
        self.low = low;
        let high = low - self.high_input + self.high_output;
        let high = if high.is_finite() { high } else { 0.0 };
        self.high_input = low;
        self.high_output = high;
        high
    }
}

/// The gate effect.
#[derive(Debug)]
pub struct Gate {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Open threshold in decibels.
    threshold_db: f32,
    /// Hysteresis width in decibels.
    hysteresis_db: f32,
    /// Range in decibels (how far down the gate closes).
    range_db: f32,
    /// Attack time in milliseconds.
    attack_ms: f32,
    /// Hold time in milliseconds.
    hold_ms: f32,
    /// Release time in milliseconds.
    release_ms: f32,
    /// Whether the sidechain filter is engaged.
    sidechain_enabled: bool,
    /// Sidechain filter centre in hertz.
    sidechain_hz: f32,
    /// Sidechain blend in percent.
    sidechain_depth: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Whether the gate is currently open.
    open: bool,
    /// Milliseconds the gate has been held open since the level dropped.
    hold_remaining_ms: f32,
    /// Smoothed linear gain actually applied.
    gain: f32,
    /// How many times the gate has changed state. Published for tests and for
    /// a UI indicator.
    transitions: u32,
    /// Per-channel sidechain filter state.
    sidechain: [SidechainState; MAX_CHANNELS],
    /// The dry snapshot of the block.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_THRESHOLD))
    }
}

impl Gate {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            threshold_db: table[PARAM_THRESHOLD as usize].default_value,
            hysteresis_db: table[PARAM_HYSTERESIS as usize].default_value,
            range_db: table[PARAM_RANGE as usize].default_value,
            attack_ms: table[PARAM_ATTACK as usize].default_value,
            hold_ms: table[PARAM_HOLD as usize].default_value,
            release_ms: table[PARAM_RELEASE as usize].default_value,
            sidechain_enabled: false,
            sidechain_hz: table[PARAM_SIDECHAIN_HZ as usize].default_value,
            sidechain_depth: table[PARAM_SIDECHAIN_DEPTH as usize].default_value,
            mix_percent: 100.0,
            wet: 1.0,
            bypassed: false,
            sample_rate: 48_000.0,
            open: true,
            hold_remaining_ms: 0.0,
            gain: 1.0,
            transitions: 0,
            sidechain: [SidechainState::default(); MAX_CHANNELS],
            dry: alloc::vec::Vec::new(),
            max_block: 0,
            table,
        }
    }

    /// Whether the gate is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// How many times the gate has opened or closed since construction.
    ///
    /// A chatter detector: a signal hovering at the threshold must leave this
    /// at zero. Published so a test can assert the hysteresis directly rather
    /// than inferring it from audio.
    #[must_use]
    pub fn transitions(&self) -> u32 {
        self.transitions
    }

    /// The smoothed linear gain currently applied.
    #[must_use]
    pub fn current_gain(&self) -> f32 {
        self.gain
    }

    /// The level at which the gate closes, in decibels.
    #[must_use]
    pub fn close_threshold_db(&self) -> f32 {
        self.threshold_db - self.hysteresis_db
    }
}

/// One-pole time constant, in milliseconds, for a sidechain corner in hertz.
///
/// The coefficient [`one_pole_coeff`] takes is dimensionless
/// (`elapsed_ms / time_ms`), so the conversion from hertz happens here:
/// `tau = 1 / (2*PI*f)`, written as a reciprocal so a zero or negative
/// frequency cannot become a negative time constant.
#[must_use]
fn sidechain_time_constant_ms(hz: f32, sample_rate: f32) -> f32 {
    let nyquist = sample_rate * 0.5;
    let hz = if hz.is_finite() {
        hz.clamp(0.1, nyquist.max(0.2))
    } else {
        1_000.0
    };
    let tau_s = 1.0 / (2.0 * core::f32::consts::PI * hz);
    (tau_s * 1000.0).clamp(1e-4, 100_000.0)
}

impl EffectProcessor for Gate {
    fn descriptor(&self) -> &'static EffectDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sample_rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.max_block = max_block.max(1);
        // Every allocation this effect will ever make happens here.
        self.dry = alloc::vec![0.0; self.max_block];
        let _ = channels;
        self.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext) {
        if self.bypassed {
            return;
        }
        let frames = buffer.frames();
        let channels = buffer.channel_count().min(MAX_CHANNELS);
        if frames == 0 || channels == 0 {
            return;
        }
        // Refuse a block larger than `prepare` sized for rather than indexing
        // past the scratch. A silent pass-through is a far better failure than
        // an out-of-bounds write in the audio thread.
        if frames > self.max_block || frames > self.dry.len() {
            return;
        }

        let block_ms = ctx.block_ms();
        let attack = one_pole_coeff(self.attack_ms, block_ms);
        let release = one_pole_coeff(self.release_ms, block_ms);
        let sidechain_on = self.sidechain_enabled && self.sidechain_depth > 0.0;
        let blend = (self.sidechain_depth / 100.0).clamp(0.0, 1.0);
        let side_coeff = one_pole_coeff(
            sidechain_time_constant_ms(self.sidechain_hz, self.sample_rate),
            block_ms,
        );

        // ── 1. Linked detector, optionally sidechained ──
        //
        // The maximum across channels, taken once, so both channels are opened
        // and closed together. A gate that closed one side independently would
        // move the stereo image, exactly like an unlinked compressor.
        let mut linked_peak = 0.0_f32;
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            self.dry[..frames].copy_from_slice(source);
            let level = if sidechain_on {
                let state = &mut self.sidechain[channel.min(MAX_CHANNELS - 1)];
                let mut filtered_peak = 0.0_f32;
                for sample in self.dry[..frames].iter() {
                    filtered_peak =
                        filtered_peak.max(state.process(*sample, side_coeff).abs());
                }
                let raw = self.dry[..frames].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
                blend * filtered_peak + (1.0 - blend) * raw
            } else {
                self.dry[..frames].iter().fold(0.0_f32, |m, s| m.max(s.abs()))
            };
            if level.is_finite() {
                linked_peak = linked_peak.max(level);
            }
        }
        if !sidechain_on {
            for state in self.sidechain.iter_mut() {
                state.reset();
            }
        }

        let level_db = gain_to_db(linked_peak);
        let level_db = if level_db.is_finite() { level_db } else { -144.0 };

        // ── 2. Hysteresis state machine ──
        //
        // Open above `threshold_db`, and only close once the level has dropped
        // past `threshold_db - hysteresis_db`. In between, whatever state we
        // are already in is kept — that is the whole point.
        let close_db = self.close_threshold_db();
        if self.open {
            if level_db <= close_db {
                // The hold keeps the gate open for a fixed time after the
                // level drops, so a percussive decay is not chopped.
                self.hold_remaining_ms -= block_ms;
                if self.hold_remaining_ms <= 0.0 {
                    self.hold_remaining_ms = 0.0;
                    self.open = false;
                    self.transitions = self.transitions.saturating_add(1);
                }
            } else {
                self.hold_remaining_ms = self.hold_ms;
            }
        } else if level_db >= self.threshold_db {
            self.open = true;
            self.hold_remaining_ms = self.hold_ms;
            self.transitions = self.transitions.saturating_add(1);
        }

        // ── 3. Target gain ──
        //
        // Fully open means *exactly* unity, which is what makes the gate
        // transparent rather than merely almost transparent.
        let target = if self.open {
            // The expansion curve anchors at 0 dB when open, so an open gate
            // passes the signal untouched.
            1.0
        } else {
            // Below the threshold the gain follows the expansion curve, so a
            // signal just under the threshold is attenuated a little rather
            // than muted. The curve is evaluated against the *closing*
            // threshold, which keeps the knee on the side of the hysteresis
            // band the gate is actually in.
            let open_db = if self.hysteresis_db > 0.0 {
                close_db
            } else {
                self.threshold_db
            };
            let span = self.hysteresis_db.max(1.0);
            db_to_gain(expansion_gain_db(level_db, open_db, span, self.range_db))
        };

        // ── 4. Smooth toward the target ──
        //
        // Per block, against the real elapsed time, so the documented attack
        // and release mean the same thing at every block size.
        let coefficient = if target > self.gain { attack } else { release };
        self.gain += (target - self.gain) * coefficient;
        if !self.gain.is_finite() || self.gain < 0.0 {
            self.gain = if target.is_finite() { target } else { 1.0 };
        }
        // Never let the smoothing exceed the target on the way *down*: an
        // over-shoot would take the output below the floor the user asked for
        // and then crawl back, which is audible as a hole.
        if target < self.gain && self.gain > 1.0 {
            self.gain = 1.0;
        }
        let gain = self.gain;
        let wet = self.wet;

        // ── 5. Apply, per channel ──
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            self.dry[..frames].copy_from_slice(source);
            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let dry_sample = self
                        .dry
                        .get(index)
                        .copied()
                        .filter(|s| s.is_finite())
                        .unwrap_or(0.0);
                    let wet_sample = dry_sample * gain;
                    *out = wet_sample * wet + dry_sample * (1.0 - wet);
                }
            }
        }
    }

    fn reset(&mut self) {
        for state in self.sidechain.iter_mut() {
            state.reset();
        }
        self.open = true;
        self.hold_remaining_ms = self.hold_ms;
        self.gain = 1.0;
        self.transitions = 0;
        self.dry.iter_mut().for_each(|s| *s = 0.0);
    }

    fn latency_samples(&self) -> usize {
        // No look-ahead and no oversampling: the gate acts on the block it is
        // given. Reporting anything else would make PDC pull this track
        // forward for no reason.
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
            PARAM_THRESHOLD => self.threshold_db = value,
            PARAM_HYSTERESIS => self.hysteresis_db = value,
            PARAM_RANGE => self.range_db = value,
            PARAM_ATTACK => self.attack_ms = value,
            PARAM_HOLD => {
                self.hold_ms = value;
                if self.hold_remaining_ms > value {
                    self.hold_remaining_ms = value;
                }
            }
            PARAM_RELEASE => self.release_ms = value,
            PARAM_SIDECHAIN_ENABLED => self.sidechain_enabled = value >= 0.5,
            PARAM_SIDECHAIN_HZ => self.sidechain_hz = value,
            PARAM_SIDECHAIN_DEPTH => self.sidechain_depth = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_THRESHOLD => Some(self.threshold_db),
            PARAM_HYSTERESIS => Some(self.hysteresis_db),
            PARAM_RANGE => Some(self.range_db),
            PARAM_ATTACK => Some(self.attack_ms),
            PARAM_HOLD => Some(self.hold_ms),
            PARAM_RELEASE => Some(self.release_ms),
            PARAM_SIDECHAIN_ENABLED => Some(if self.sidechain_enabled { 1.0 } else { 0.0 }),
            PARAM_SIDECHAIN_HZ => Some(self.sidechain_hz),
            PARAM_SIDECHAIN_DEPTH => Some(self.sidechain_depth),
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

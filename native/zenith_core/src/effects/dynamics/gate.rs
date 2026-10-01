//! Noise gate / downward expander with hysteresis, hold and a sidechain filter.
//!
//! # Why hysteresis is not optional
//!
//! A gate compares the detector's level against a threshold. With a single
//! threshold, a signal hovering exactly at it crosses back and forth on every
//! block and the gate chatters - an audible stutter far worse than the noise it
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
/// * at and above `open_db` the gain is exactly `0` dB - the gate is
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
///
/// The sidechain is a **band-pass**: a one-pole high-pass at the control's
/// frequency, cascaded with a one-pole low-pass a decade above it. A single
/// corner would be a tilt rather than a band, and "ignore the rumble" is the
/// use case the control exists for - a high-pass alone with no upper bound
/// would let a cymbal wash open the gate as readily as a kick drum.
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

    /// Applies the band-pass pair described above, one **sample** at a time.
    ///
    /// `high_pole` is the pole position of the high-pass and `low_pole` that of
    /// the low-pass. The coefficients are per-sample, not per-block: a filter
    /// corner is a property of the sample rate, and folding a 5 ms block into
    /// one step would put the "150 Hz" corner wherever the block size happened
    /// to fall. The block-rate form of [`one_pole_coeff`] belongs to the gate's
    /// attack, hold and release, which really are evaluated once per block.
    fn process(&mut self, input: f32, high_pole: f32, low_pole: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        // High-pass first: rumble below the corner must not reach the
        // low-pass's state, where it would linger. `y = x - x1 + p*y1` is the
        // DC-blocker form `dsp::DcBlocker` uses.
        let high = input - self.high_input + high_pole.clamp(0.0, 1.0) * self.high_output;
        let high = if high.is_finite() { high } else { 0.0 };
        self.high_input = input;
        self.high_output = high;

        // The low-pass wants the *input* coefficient, so it is `1 - pole`.
        self.low += (high - self.low) * (1.0 - low_pole.clamp(0.0, 1.0));
        if self.low.is_finite() {
            self.low
        } else {
            self.low = 0.0;
            0.0
        }
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
    /// The linked detector level from the most recent block, as a linear peak.
    detector_peak: f32,
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
            detector_peak: 0.0,
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

    /// The detector level used for the most recent block, as a linear peak.
    ///
    /// This is the linked (and, when enabled, sidechain-filtered) level the
    /// open/close decision was made from, before it is compared with the
    /// threshold. Published so a test can assert what the detector saw rather
    /// than inferring it from the gate's decision.
    #[must_use]
    pub fn detector_peak(&self) -> f32 {
        self.detector_peak
    }
}

/// How far above the sidechain's high-pass corner its low-pass corner sits.
///
/// A decade is wide enough that the two sections do not fight each other around
/// the control frequency (which would make the band a notch) and narrow enough
/// that the filter still reads as a band rather than a tilt.
const SIDECHAIN_BAND_RATIO: f32 = 10.0;

/// A one-pole **pole position** for a corner frequency in hertz.
///
/// This is the per-sample form (`exp(-2*PI*f/fs)`), not the block-rate
/// [`one_pole_coeff`]: a filter corner is a property of the sample rate, so
/// deriving it from the block duration would move the corner every time the
/// engine changed its buffer size. `exp2` keeps the crate free of `exp`.
///
/// The value returned is the **pole** (`p`), used directly by a high-pass
/// (`y = x - x1 + p*y1`); a low-pass wants `1 - p`. A non-finite or
/// non-positive frequency fails safe to `0.0`, which makes a high-pass a
/// pass-through and a low-pass wide open - the transparent degenerate case
/// rather than a silent sidechain.
#[must_use]
fn sample_pole(hz: f32, sample_rate: f32) -> f32 {
    if !hz.is_finite() || hz <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0.0;
    }
    let nyquist = sample_rate * 0.5;
    let hz = hz.min(nyquist * 0.99).max(0.01);
    let exponent = -2.0 * core::f32::consts::PI * hz / sample_rate;
    let decay = crate::effects::util::dsp::exp2(exponent * core::f32::consts::LOG2_E);
    if decay.is_finite() {
        decay.clamp(0.0, 1.0)
    } else {
        0.0
    }
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
        // High-pass at the control's frequency, low-pass a decade above it.
        // Both are per-sample pole positions, because they describe a filter
        // corner rather than a smoothing time.
        let high_pole = sample_pole(self.sidechain_hz, self.sample_rate);
        let low_pole = sample_pole(self.sidechain_hz * SIDECHAIN_BAND_RATIO, self.sample_rate);

        // -- 1. Linked detector, optionally sidechained --
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
                        filtered_peak.max(state.process(*sample, high_pole, low_pole).abs());
                }
                let raw = self.dry[..frames]
                    .iter()
                    .fold(0.0_f32, |m, s| m.max(s.abs()));
                blend * filtered_peak + (1.0 - blend) * raw
            } else {
                self.dry[..frames]
                    .iter()
                    .fold(0.0_f32, |m, s| m.max(s.abs()))
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
        let level_db = if level_db.is_finite() {
            level_db
        } else {
            -144.0
        };
        self.detector_peak = if linked_peak.is_finite() {
            linked_peak
        } else {
            0.0
        };

        // -- 2. Hysteresis state machine --
        //
        // Open above `threshold_db`, and only close once the level has dropped
        // past `threshold_db - hysteresis_db`. In between, whatever state we
        // are already in is kept - that is the whole point.
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

        // -- 3. Target gain --
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

        // -- 4. Smooth toward the target --
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

        // -- 5. Apply, per channel --
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
        self.detector_peak = 0.0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;
    const CHUNK: usize = 256;

    fn make() -> Gate {
        let mut effect = Gate::new(ParameterAddress::effect(0, 0, PARAM_THRESHOLD));
        effect.prepare(SR, CHUNK, 2);
        effect
    }

    /// Processes one stereo block through the real `process`.
    fn block(effect: &mut Gate, left: &mut [f32], right: &mut [f32], frame: i64) {
        let mut views: [&mut [f32]; 2] = [left, right];
        let mut buffer = AudioBuffer::new(&mut views);
        let frames = buffer.frames();
        let ctx = RenderContext::new(SR, frames, frame, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }

    /// Processes one mono block.
    fn block_mono(effect: &mut Gate, channel: &mut [f32], frame: i64) {
        let mut views: [&mut [f32]; 1] = [channel];
        let mut buffer = AudioBuffer::new(&mut views);
        let frames = buffer.frames();
        let ctx = RenderContext::new(SR, frames, frame, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }

    /// Drives `blocks` blocks of a constant level and returns the settled
    /// output level (measured over the second half of the run).
    fn measure_level(effect: &mut Gate, level: f32, blocks: usize) -> f32 {
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        let mut peak = 0.0_f32;
        for round in 0..blocks {
            left.iter_mut().for_each(|s| *s = level);
            right.iter_mut().for_each(|s| *s = level);
            block(effect, &mut left, &mut right, (round * CHUNK) as i64);
            if round >= blocks / 2 {
                for sample in left.iter() {
                    peak = peak.max(sample.abs());
                }
            }
        }
        peak
    }

    // -- Identity and table --

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, crate::effects::registry::KIND_GATE);
        assert_eq!(d.kind, 0x0000_0202);
        assert_eq!(d.key, "noise_gate");
        assert_eq!(d.label, "Noise Gate");
        assert_eq!(d.category, EffectCategory::Dynamics);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(
            !d.has_latency,
            "the gate has no look-ahead, so it must advertise no latency"
        );
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
            assert!(!spec.key.is_empty());
            assert!(
                spec.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "key {} is not a stable machine key",
                spec.key
            );
        }
    }

    #[test]
    fn parameter_keys_are_unique_and_addresses_follow_the_slot() {
        let table = parameter_table(ParameterAddress::effect(4, 6, 0));
        for (i, a) in table.iter().enumerate() {
            assert_eq!(a.address.effect_slot(), 6);
            assert_eq!(a.address.index, 4);
            for b in &table[i + 1..] {
                assert_ne!(a.key, b.key, "duplicate parameter key {}", a.key);
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
            if sub == PARAM_SIDECHAIN_ENABLED {
                continue;
            }
            assert!(
                (read - midpoint).abs() < 1e-3,
                "parameter {sub} read back {read}, expected {midpoint}"
            );
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_not_rejected() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, 1e9);
        assert_eq!(effect.get_parameter(PARAM_THRESHOLD), Some(0.0));
        effect.set_parameter(PARAM_THRESHOLD, -1e9);
        assert_eq!(effect.get_parameter(PARAM_THRESHOLD), Some(-80.0));
        effect.set_parameter(PARAM_RANGE, f32::NAN);
        assert_eq!(
            effect.get_parameter(PARAM_RANGE),
            Some(effect.table[PARAM_RANGE as usize].default_value)
        );
    }

    #[test]
    fn unknown_parameter_ordinals_are_ignored() {
        let mut effect = make();
        let before = effect.get_parameter(PARAM_THRESHOLD);
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(PARAM_THRESHOLD), before);
        assert_eq!(effect.get_parameter(999), None);
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

    // -- Required contract behaviours --

    #[test]
    fn bypass_returns_the_input_untouched() {
        let mut effect = make();
        effect.set_bypassed(true);
        let mut left = alloc::vec![0.75_f32; CHUNK];
        let mut right = alloc::vec![-0.25_f32; CHUNK];
        let expected_left = left.clone();
        let expected_right = right.clone();
        block(&mut effect, &mut left, &mut right, 0);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_rather_than_overrunning() {
        let mut effect = make();
        let mut left = alloc::vec![1.0_f32; 4_800];
        let mut right = alloc::vec![1.0_f32; 4_800];
        let expected = left.clone();
        block(&mut effect, &mut left, &mut right, 0);
        assert_eq!(left, expected, "an oversized block must be a safe no-op");
    }

    #[test]
    fn non_finite_input_never_reaches_the_output() {
        let mut effect = make();
        let mut left = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        let mut right = alloc::vec![f32::NAN; 4];
        block(&mut effect, &mut left, &mut right, 0);
        for (i, sample) in left.iter().chain(right.iter()).enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
    }

    #[test]
    fn output_stays_finite_under_every_extreme_parameter_at_once() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            let spec = effect.table[sub as usize];
            effect.set_parameter(sub, spec.max_value);
        }
        for round in 0..48 {
            let mut left = alloc::vec![1.0_f32; CHUNK];
            let mut right = alloc::vec![-1.0_f32; CHUNK];
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(
                    sample.is_finite(),
                    "round {round} sample {i} is {sample} with every parameter maxed"
                );
            }
        }
    }

    #[test]
    fn reset_clears_the_state_and_the_sidechain_filters() {
        let mut effect = make();
        effect.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
        let mut left = alloc::vec![1.0_f32; CHUNK];
        let mut right = alloc::vec![1.0_f32; CHUNK];
        block(&mut effect, &mut left, &mut right, 0);
        effect.reset();
        assert_eq!(effect.current_gain(), 1.0);
        assert_eq!(effect.transitions(), 0);
        assert_eq!(effect.sidechain[0].low, 0.0);
        assert_eq!(effect.sidechain[0].high_output, 0.0);
    }

    #[test]
    fn an_empty_block_is_a_no_op() {
        let mut effect = make();
        let mut empty: [&mut [f32]; 0] = [];
        let mut buffer = AudioBuffer::new(&mut empty);
        let ctx = RenderContext::new(SR, 0, 0, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }

    #[test]
    fn the_gate_has_no_tail() {
        let effect = make();
        assert_eq!(effect.tail_seconds(), 0.0);
    }

    #[test]
    fn the_gate_reports_no_latency() {
        // There is no look-ahead and no oversampling, so PDC has nothing to
        // compensate. Anything else would pull this track forward for no
        // reason.
        let effect = make();
        assert_eq!(effect.latency_samples(), 0);
    }

    // -- Open / close behaviour --

    #[test]
    fn a_signal_below_the_threshold_is_closed_down() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_RANGE, -60.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        // -60 dBFS is far below a -30 dB threshold, and below the expansion
        // knee, so the gate should be at its floor.
        let level = measure_level(&mut effect, db_to_gain(-60.0), 80);
        let floor = db_to_gain(-60.0) * db_to_gain(-60.0);
        assert!(
            level <= floor * 1.5,
            "the gate did not close: {level} against a floor of {floor}"
        );
        assert!(!effect.is_open());
    }

    #[test]
    fn a_signal_above_the_threshold_is_passed_through() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_RANGE, -60.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        let level = measure_level(&mut effect, db_to_gain(-6.0), 40);
        assert!(
            (level - db_to_gain(-6.0)).abs() < 1e-4,
            "an open gate must be transparent: {level} vs {}",
            db_to_gain(-6.0)
        );
        assert!(effect.is_open());
    }

    #[test]
    fn an_open_gate_is_exactly_transparent() {
        // Not "within a fraction of a dB" - exactly unity. A gate that applies
        // even a 0.01 dB trim colours every signal that passes through it.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -60.0);
        effect.set_parameter(PARAM_RANGE, -90.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for n in 0..CHUNK {
            let sample = 0.7 * sin_poly(2.0 * PI * 220.0 * n as f32 / SR);
            left[n] = sample;
            right[n] = sample;
        }
        let expected_left = left.clone();
        for round in 0..20 {
            if round > 0 {
                for n in 0..CHUNK {
                    let sample = 0.7 * sin_poly(2.0 * PI * 220.0 * n as f32 / SR);
                    left[n] = sample;
                    right[n] = sample;
                }
            }
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        for (got, want) in left.iter().zip(expected_left.iter()) {
            assert_eq!(got, want, "an open gate altered the signal");
        }
    }

    #[test]
    fn the_gate_opens_when_the_level_rises_above_the_threshold() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        // Start closed on a quiet level, then go loud.
        let quiet = db_to_gain(-70.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = quiet);
            right.iter_mut().for_each(|s| *s = quiet);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(!effect.is_open(), "the gate should have closed");
        let before = effect.transitions();

        let loud = db_to_gain(-6.0);
        for round in 0..8 {
            left.iter_mut().for_each(|s| *s = loud);
            right.iter_mut().for_each(|s| *s = loud);
            block(
                &mut effect,
                &mut left,
                &mut right,
                ((40 + round) * CHUNK) as i64,
            );
        }
        assert!(effect.is_open(), "the gate did not open on a loud signal");
        assert_eq!(effect.transitions(), before + 1);
    }

    #[test]
    fn the_gate_closes_when_the_level_falls_below_the_threshold() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        let loud = db_to_gain(-6.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..20 {
            left.iter_mut().for_each(|s| *s = loud);
            right.iter_mut().for_each(|s| *s = loud);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(effect.is_open());
        let quiet = db_to_gain(-70.0);
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = quiet);
            right.iter_mut().for_each(|s| *s = quiet);
            block(
                &mut effect,
                &mut left,
                &mut right,
                ((20 + round) * CHUNK) as i64,
            );
        }
        assert!(
            !effect.is_open(),
            "the gate did not close on a quiet signal"
        );
    }

    #[test]
    fn the_range_control_sets_how_far_down_the_gate_closes() {
        let mut shallow = make();
        let mut deep = make();
        for (effect, range) in [(&mut shallow, -12.0_f32), (&mut deep, -80.0)] {
            effect.set_parameter(PARAM_THRESHOLD, -30.0);
            effect.set_parameter(PARAM_RANGE, range);
            effect.set_parameter(PARAM_HOLD, 0.0);
            effect.set_parameter(PARAM_RELEASE, 5.0);
            effect.set_parameter(PARAM_ATTACK, 0.1);
        }
        let level = db_to_gain(-60.0);
        let shallow_out = measure_level(&mut shallow, level, 80);
        let deep_out = measure_level(&mut deep, level, 80);
        assert!(
            deep_out < shallow_out,
            "a -80 dB range ({deep_out}) did not close further than -12 dB ({shallow_out})"
        );
        assert!(
            (shallow_out - level * db_to_gain(-12.0)).abs() < level * 0.05,
            "the -12 dB range did not land at -12 dB: {shallow_out}"
        );
    }

    #[test]
    fn an_inverted_range_cannot_become_a_gain_boost() {
        // `range_db` is a floor, so a positive value (or one clamped to 0)
        // means "no attenuation" rather than amplification.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -10.0);
        effect.set_parameter(PARAM_RANGE, 0.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        let level = db_to_gain(-60.0);
        let out = measure_level(&mut effect, level, 60);
        assert!(
            out <= level * 1.001,
            "the gate boosted a quiet signal to {out} from {level}"
        );
    }

    // -- Hysteresis: the core requirement --

    #[test]
    fn a_signal_sitting_exactly_at_the_threshold_does_not_chatter() {
        // The failure this prevents: with a single threshold, a level sitting
        // on it crosses back and forth on every block and the gate stutters.
        // The hysteresis band means the state is kept once it is entered.
        let mut effect = make();
        let threshold_db = -30.0_f32;
        effect.set_parameter(PARAM_THRESHOLD, threshold_db);
        effect.set_parameter(PARAM_HYSTERESIS, 6.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 20.0);

        // Dither the level a hair either side of the threshold, which is what
        // real programme material hovering at the threshold does.
        let level = db_to_gain(threshold_db);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..200 {
            let wiggle = if round % 2 == 0 { 1.0001 } else { 0.9999 };
            left.iter_mut().for_each(|s| *s = level * wiggle);
            right.iter_mut().for_each(|s| *s = level * wiggle);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(
            effect.transitions() <= 1,
            "the gate chattered {} times on a signal at the threshold",
            effect.transitions()
        );
    }

    #[test]
    fn hysteresis_holds_the_state_inside_the_band() {
        // Open above the threshold, then drop into the band between the open
        // and close thresholds: the gate must stay where it was.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_HYSTERESIS, 12.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        assert_eq!(effect.close_threshold_db(), -42.0);

        // Drive it open.
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        let loud = db_to_gain(-6.0);
        for round in 0..10 {
            left.iter_mut().for_each(|s| *s = loud);
            right.iter_mut().for_each(|s| *s = loud);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(effect.is_open());
        let opened_at = effect.transitions();

        // Now sit at -36 dB, which is below the open threshold (-30) but above
        // the close threshold (-42). Inside the band: no change.
        let inside = db_to_gain(-36.0);
        for round in 0..60 {
            left.iter_mut().for_each(|s| *s = inside);
            right.iter_mut().for_each(|s| *s = inside);
            block(
                &mut effect,
                &mut left,
                &mut right,
                ((10 + round) * CHUNK) as i64,
            );
        }
        assert!(
            effect.is_open(),
            "the gate closed inside the hysteresis band"
        );
        assert_eq!(
            effect.transitions(),
            opened_at,
            "the gate changed state inside the hysteresis band"
        );
    }

    #[test]
    fn a_start_closed_gate_stays_closed_inside_the_band() {
        // The other direction: if the gate is already closed, a level inside
        // the band must not open it. Together with the test above this proves
        // the band is a genuine two-threshold state machine.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_HYSTERESIS, 12.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        // Close it.
        let quiet = db_to_gain(-70.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = quiet);
            right.iter_mut().for_each(|s| *s = quiet);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(!effect.is_open());

        // -36 dB is inside the band, so the gate must stay closed.
        let inside = db_to_gain(-36.0);
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = inside);
            right.iter_mut().for_each(|s| *s = inside);
            block(
                &mut effect,
                &mut left,
                &mut right,
                ((40 + round) * CHUNK) as i64,
            );
        }
        assert!(
            !effect.is_open(),
            "a closed gate opened inside the hysteresis band"
        );
    }

    #[test]
    fn a_wider_hysteresis_band_tolerates_more_wobble() {
        // The band's width is what buys the immunity, so a narrow setting must
        // chatter where a wide one does not.
        let chatter = |hysteresis: f32| -> u32 {
            let mut effect = make();
            effect.set_parameter(PARAM_THRESHOLD, -30.0);
            effect.set_parameter(PARAM_HYSTERESIS, hysteresis);
            effect.set_parameter(PARAM_HOLD, 0.0);
            effect.set_parameter(PARAM_ATTACK, 0.1);
            effect.set_parameter(PARAM_RELEASE, 2.0);
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            // A slow swell that crosses the threshold repeatedly.
            for round in 0..120 {
                let phase = round as f32 / 12.0;
                let sway = 1.0 + 0.35 * sin_poly(phase);
                let level = db_to_gain(-30.0) * sway;
                left.iter_mut().for_each(|s| *s = level);
                right.iter_mut().for_each(|s| *s = level);
                block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            effect.transitions()
        };
        let narrow = chatter(0.0);
        let wide = chatter(18.0);
        assert!(
            wide < narrow,
            "an 18 dB band ({wide} transitions) was not calmer than 0 dB ({narrow})"
        );
        assert!(
            wide <= 4,
            "an 18 dB hysteresis band still chattered {wide} times"
        );
    }

    #[test]
    fn zero_hysteresis_still_works_as_a_plain_gate() {
        // Hysteresis at its minimum must degrade to an ordinary single
        // threshold rather than doing something degenerate.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_HYSTERESIS, 0.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        assert_eq!(effect.close_threshold_db(), -30.0);
        let loud = measure_level(&mut effect, db_to_gain(-6.0), 40);
        assert!((loud - db_to_gain(-6.0)).abs() < 1e-4);
        assert!(effect.is_open());
    }

    // -- Hold --

    #[test]
    fn the_hold_keeps_the_gate_open_after_the_level_drops() {
        let run = |hold_ms: f32| -> bool {
            let mut effect = make();
            effect.set_parameter(PARAM_THRESHOLD, -30.0);
            effect.set_parameter(PARAM_HOLD, hold_ms);
            effect.set_parameter(PARAM_RELEASE, 5.0);
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            let loud = db_to_gain(-6.0);
            for round in 0..10 {
                left.iter_mut().for_each(|s| *s = loud);
                right.iter_mut().for_each(|s| *s = loud);
                block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            // A short silence, deliberately shorter than the hold.
            let quiet = db_to_gain(-80.0);
            for round in 0..4 {
                left.iter_mut().for_each(|s| *s = quiet);
                right.iter_mut().for_each(|s| *s = quiet);
                block(
                    &mut effect,
                    &mut left,
                    &mut right,
                    ((10 + round) * CHUNK) as i64,
                );
            }
            effect.is_open()
        };
        let no_hold = run(0.0);
        let with_hold = run(2_000.0);
        assert!(!no_hold, "a 0 ms hold should have closed the gate");
        assert!(
            with_hold,
            "a 2000 ms hold should still be holding after ~21 ms of silence"
        );
    }

    // -- Gain smoothing --

    #[test]
    fn the_attack_and_release_are_block_size_independent() {
        // `one_pole_coeff` is evaluated against the real block duration, so a
        // transition of a given length in milliseconds must take the same
        // number of milliseconds at any block size.
        let _ = CHUNK;
        let run = |frames: usize| -> f32 {
            let mut effect = Gate::new(ParameterAddress::effect(0, 0, 0));
            effect.prepare(SR, frames, 1);
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_HOLD, 0.0);
            effect.set_parameter(PARAM_ATTACK, 10.0);
            effect.set_parameter(PARAM_RELEASE, 10.0);
            let total_ms = 20.0_f32;
            let blocks = ((total_ms / 1000.0) * SR / frames as f32).ceil() as usize;
            let mut channel = alloc::vec![0.0_f32; frames];
            for round in 0..blocks {
                channel.iter_mut().for_each(|s| *s = db_to_gain(-60.0));
                block_mono(&mut effect, &mut channel, (round * frames) as i64);
            }
            effect.current_gain()
        };
        let small = run(64);
        let large = run(1024);
        assert!(
            (small - large).abs() < 0.05,
            "gain after 20 ms differs by block size: {small} vs {large}"
        );
    }

    #[test]
    fn a_slow_attack_opens_more_gently_than_a_fast_one() {
        let after = |attack_ms: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_HOLD, 0.0);
            effect.set_parameter(PARAM_ATTACK, attack_ms);
            effect.set_parameter(PARAM_RELEASE, 5.0);
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            for round in 0..6 {
                left.iter_mut().for_each(|s| *s = db_to_gain(-60.0));
                right.iter_mut().for_each(|s| *s = db_to_gain(-60.0));
                block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            let loud = db_to_gain(-6.0);
            for round in 0..4 {
                left.iter_mut().for_each(|s| *s = loud);
                right.iter_mut().for_each(|s| *s = loud);
                block(
                    &mut effect,
                    &mut left,
                    &mut right,
                    ((6 + round) * CHUNK) as i64,
                );
            }
            effect.current_gain()
        };
        let fast = after(0.1);
        let slow = after(300.0);
        assert!(
            slow < fast,
            "a 300 ms attack ({slow}) opened as fast as a 0.1 ms one ({fast})"
        );
    }

    // -- Stereo link --

    #[test]
    fn the_gate_is_stereo_linked() {
        // A loud left and a quiet right: the link means the right is opened by
        // the left's level, so it is not gated independently.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = db_to_gain(-3.0));
            // The right side is well below the threshold on its own.
            right.iter_mut().for_each(|s| *s = db_to_gain(-50.0));
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(
            effect.is_open(),
            "the gate ignored the loud left channel when deciding to open"
        );
        // Both channels get the same gain, so the level ratio is preserved.
        let ratio = left[0] / right[0];
        assert!(
            (ratio - db_to_gain(-3.0) / db_to_gain(-50.0)).abs() < 1e-3,
            "the stereo link altered the balance: ratio {ratio}"
        );
    }

    #[test]
    fn an_identical_stereo_signal_stays_identical() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..60 {
            for n in 0..CHUNK {
                let level = if round % 2 == 0 {
                    db_to_gain(-6.0)
                } else {
                    db_to_gain(-60.0)
                };
                let sample = level * sin_poly(2.0 * PI * 200.0 * n as f32 / SR);
                left[n] = sample;
                right[n] = sample;
            }
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            for (l, r) in left.iter().zip(right.iter()) {
                assert_eq!(l, r, "linked channels diverged");
            }
        }
    }

    // -- Sidechain --

    #[test]
    fn the_sidechain_filter_changes_when_the_gate_opens() {
        // The sidechain's job is to change *what the detector sees*, and that
        // is what this measures: the same tone, the same threshold, only the
        // sidechain differing. The detector level is compared directly rather
        // than through the gate's decision, so the assertion does not depend on
        // where the threshold happens to fall.
        let detector = |enabled: bool, hz: f32, corner: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_HOLD, 0.0);
            effect.set_parameter(PARAM_SIDECHAIN_ENABLED, if enabled { 1.0 } else { 0.0 });
            effect.set_parameter(PARAM_SIDECHAIN_HZ, corner);
            effect.set_parameter(PARAM_SIDECHAIN_DEPTH, 100.0);
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            for round in 0..80 {
                for n in 0..CHUNK {
                    let sample = 0.5 * sin_poly(2.0 * PI * hz * n as f32 / SR);
                    left[n] = sample;
                    right[n] = sample;
                }
                block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            effect.detector_peak()
        };

        // A tone below the sidechain's corner is attenuated. A one-pole
        // high-pass falls at 6 dB per octave, so 68 Hz against a 300 Hz corner
        // is a couple of octaves down and worth roughly -4 dB - a clear
        // reduction, though nothing like a brick wall.
        let raw = detector(false, 68.0, 300.0);
        let filtered = detector(true, 68.0, 300.0);
        assert!(
            (raw - 0.5).abs() < 0.02,
            "the unfiltered detector should track the tone peak: {raw}"
        );
        assert!(
            filtered < raw * 0.75,
            "the sidechain high-pass only took {raw} down to {filtered}"
        );
        // The attenuation has to grow as the tone moves further below the
        // corner, which is what distinguishes a high-pass from a shelf.
        let lower = detector(true, 34.0, 300.0);
        assert!(
            lower < filtered,
            "34 Hz ({lower}) was not cut harder than 68 Hz ({filtered})"
        );

        // A tone inside the band passes, which is what keeps the sidechain a
        // filter rather than a gate-killer.
        let in_band = detector(true, 800.0, 300.0);
        assert!(
            in_band > 0.3,
            "a tone in the middle of the band came out at {in_band}"
        );
        assert!(
            in_band > filtered,
            "the band did not favour its centre: {in_band} vs {filtered}"
        );
    }

    #[test]
    fn the_sidechain_depth_fades_between_the_filtered_and_raw_detector() {
        // Depth is a crossfade, not a switch: 50% must land between the two
        // extremes rather than snapping to either. Measured on the detector, so
        // the result is the blend itself.
        let detector = |depth: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_HOLD, 0.0);
            effect.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
            effect.set_parameter(PARAM_SIDECHAIN_HZ, 300.0);
            effect.set_parameter(PARAM_SIDECHAIN_DEPTH, depth);
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            for round in 0..80 {
                for n in 0..CHUNK {
                    let sample = 0.5 * sin_poly(2.0 * PI * 68.0 * n as f32 / SR);
                    left[n] = sample;
                    right[n] = sample;
                }
                block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            effect.detector_peak()
        };
        let raw = detector(0.0);
        let full = detector(100.0);
        let half = detector(50.0);
        assert!(full < raw, "full depth ({full}) was not below raw ({raw})");
        assert!(
            half > full && half < raw,
            "50% depth ({half}) is not between 100% ({full}) and 0% ({raw})"
        );
        // A crossfade, not a curve: halfway must be about halfway.
        let expected = (raw + full) * 0.5;
        assert!(
            (half - expected).abs() < (raw - full).abs() * 0.15,
            "50% depth gave {half}, not the midpoint {expected}"
        );
    }

    #[test]
    fn the_sidechain_band_passes_the_programme_that_matters() {
        // The complement: a tone inside the band must still open the gate, or
        // the sidechain would be a mute rather than a filter.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
        effect.set_parameter(PARAM_SIDECHAIN_HZ, 100.0);
        effect.set_parameter(PARAM_SIDECHAIN_DEPTH, 100.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..60 {
            for n in 0..CHUNK {
                let sample = 0.5 * sin_poly(2.0 * PI * 400.0 * n as f32 / SR);
                left[n] = sample;
                right[n] = sample;
            }
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(
            effect.is_open(),
            "a 400 Hz tone inside the sidechain band did not open the gate"
        );
    }

    #[test]
    fn sidechain_depth_zero_is_the_same_as_sidechain_off() {
        let capture = |enabled: bool, depth: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_SIDECHAIN_ENABLED, if enabled { 1.0 } else { 0.0 });
            effect.set_parameter(PARAM_SIDECHAIN_HZ, 150.0);
            effect.set_parameter(PARAM_SIDECHAIN_DEPTH, depth);
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            for round in 0..60 {
                for n in 0..CHUNK {
                    let sample = 0.5 * sin_poly(2.0 * PI * 30.0 * n as f32 / SR);
                    left[n] = sample;
                    right[n] = sample;
                }
                block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            effect.current_gain()
        };
        let off = capture(false, 100.0);
        let zero_depth = capture(true, 0.0);
        assert!((off - zero_depth).abs() < 1e-6, "{off} vs {zero_depth}");
    }

    #[test]
    fn the_sidechain_filter_is_per_channel() {
        let mut effect = make();
        effect.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
        effect.set_parameter(PARAM_SIDECHAIN_HZ, 200.0);
        let mut left: alloc::vec::Vec<f32> = (0..CHUNK)
            .map(|n| sin_poly(2.0 * PI * 600.0 * n as f32 / SR))
            .collect();
        let mut right = alloc::vec![0.0_f32; CHUNK];
        block(&mut effect, &mut left, &mut right, 0);
        assert!(
            effect.sidechain[0].low.abs() > 1e-3,
            "the left filter should have moved"
        );
        assert_eq!(
            effect.sidechain[1].low, 0.0,
            "the right channel's sidechain filter picked up the left channel's state"
        );
    }

    #[test]
    fn the_sidechain_corner_pole_is_sample_rate_based() {
        // A 150 Hz corner at 48 kHz: exp(-2*PI*150/48000) ~ 0.9806.
        let pole = sample_pole(150.0, SR);
        assert!((pole - 0.9806).abs() < 1e-3, "pole is {pole}");
        // A lower corner means a pole nearer 1 (a longer memory).
        assert!(sample_pole(50.0, SR) > pole);
        // A higher corner means a pole nearer 0.
        assert!(sample_pole(1_500.0, SR) < pole);
        // A higher sample rate moves the pole toward 1 for the same corner.
        assert!(sample_pole(150.0, 96_000.0) > pole);

        // Degenerate inputs fail safe to a pass-through (pole 0) rather than
        // to a filter that cannot be opened.
        assert_eq!(sample_pole(0.0, SR), 0.0);
        assert_eq!(sample_pole(-100.0, SR), 0.0);
        assert_eq!(sample_pole(f32::NAN, SR), 0.0);
        assert_eq!(sample_pole(1_000.0, 0.0), 0.0);
        // Above Nyquist it saturates rather than wrapping.
        let above = sample_pole(1e9, SR);
        assert!(
            above.is_finite() && (0.0..1.0).contains(&above),
            "got {above}"
        );
    }

    #[test]
    fn the_sidechain_band_is_a_band_not_a_mute() {
        // Feed the filter directly: a tone inside the band must come out close
        // to its input level, and a tone below the high-pass corner must come
        // out much smaller. Without this, a filter that crushed everything
        // would still pass the "sidechain changes the response" test.
        let mut state = SidechainState::default();
        let high_pole = sample_pole(200.0, SR);
        let low_pole = sample_pole(2_000.0, SR);

        let peak_at = |state: &mut SidechainState, hz: f32| -> f32 {
            state.reset();
            let mut peak = 0.0_f32;
            for n in 0..8_192 {
                let x = sin_poly(2.0 * PI * hz * n as f32 / SR);
                let y = state.process(x, high_pole, low_pole);
                if n > 4_096 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };

        let centre = peak_at(&mut state, 700.0);
        let below = peak_at(&mut state, 20.0);
        assert!(
            centre > 0.8,
            "a tone in the middle of the band came out at {centre}"
        );
        assert!(
            below < 0.2,
            "a tone below the high-pass corner came out at {below}"
        );
    }

    // -- The expansion curve --

    #[test]
    fn the_expansion_curve_is_continuous_and_anchored() {
        // Open: exactly unity, which is what makes the gate transparent.
        assert_eq!(expansion_gain_db(-10.0, -10.0, 6.0, -60.0), 0.0);
        assert_eq!(expansion_gain_db(0.0, -10.0, 6.0, -60.0), 0.0);
        // At the floor: exactly the requested range.
        assert_eq!(expansion_gain_db(-30.0, -10.0, 6.0, -60.0), -60.0);
        // Halfway through the knee: half the floor.
        let mid = expansion_gain_db(-13.0, -10.0, 6.0, -60.0);
        assert!((mid + 30.0).abs() < 1e-3, "midpoint was {mid}");
        // The curve never boosts and never falls below the floor.
        for level in [
            -120.0_f32, -60.0, -30.0, -14.0, -13.0, -11.0, -9.0, 0.0, 12.0,
        ] {
            let gain = expansion_gain_db(level, -12.0, 8.0, -40.0);
            assert!(gain <= 0.0, "level {level} produced a boost of {gain}");
            assert!(gain >= -40.0, "level {level} fell below the floor: {gain}");
        }
        // Non-finite input must not become a NaN gain.
        assert_eq!(expansion_gain_db(f32::NAN, -12.0, 6.0, -60.0), 0.0);
        assert_eq!(expansion_gain_db(-30.0, f32::NAN, 6.0, -60.0), 0.0);
        assert_eq!(expansion_gain_db(-30.0, -12.0, 6.0, f32::NAN), 0.0);
    }

    #[test]
    fn a_mono_block_is_processed_correctly() {
        let mut effect = Gate::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, CHUNK, 1);
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_HOLD, 0.0);
        effect.set_parameter(PARAM_RANGE, -80.0);
        effect.set_parameter(PARAM_RELEASE, 5.0);
        let mut channel = alloc::vec![0.0_f32; CHUNK];
        let quiet = db_to_gain(-70.0);
        for round in 0..60 {
            channel.iter_mut().for_each(|s| *s = quiet);
            block_mono(&mut effect, &mut channel, (round * CHUNK) as i64);
        }
        assert!(!effect.is_open(), "the mono path did not close");
        assert!(
            channel[0].abs() <= quiet,
            "the mono path boosted the signal: {} from {quiet}",
            channel[0]
        );
    }
}

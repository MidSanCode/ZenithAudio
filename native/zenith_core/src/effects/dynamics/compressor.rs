//! Feed-forward compressor with look-ahead and a stereo-linked detector.
//!
//! # The gain computer
//!
//! The static curve is the standard one and is implemented in dB directly,
//! because that is where it is defined: convert the level to decibels, apply
//! the soft knee, measure the overshoot above the threshold, divide the
//! overshoot by the ratio, and add the result back onto the input level. Doing
//! the arithmetic in the linear domain instead has no way to express "the
//! first `knee/2` dB below the threshold is compressed by a fraction of the
//! ratio" without re-deriving the same logarithm.
//!
//! ```text
//!   d = level_db - threshold_db
//!   overshoot = 0                        d < -knee/2
//!             = (d + knee/2)^2/(2*knee)  |d| <= knee/2
//!             = d                        d > knee/2
//!   output_db = level_db - overshoot * (1 - 1/ratio)
//!   gain      = db_to_gain(output_db - level_db)
//! ```
//!
//! The same function is used by [`super::limiter`] with the knee forced to
//! zero, so a limiter is exactly a compressor at an extreme ratio rather than a
//! second, subtly different curve.
//!
//! # Detection, ratio and the stereo link
//!
//! This is a **feed-forward** design: the detector is the maximum of every
//! channel's peak, and the resulting gain reduction is applied to every
//! channel. That link is not a convenience - reducing channels independently
//! modulates the difference signal, so a hard hit on one side pulls the whole
//! stereo image toward the other. A test asserts the link directly.
//!
//! The detector runs at **block rate**, like the envelope follower in
//! [`crate::effects::filter::multimode`], and the attack/release coefficients
//! come from [`one_pole_coeff`] against `ctx.block_ms()` so a documented time
//! constant means the same thing at every block size.
//!
//! # Look-ahead
//!
//! The detector runs `lookahead_ms` *ahead* of the audio it controls: it reads
//! the block in the delay line, which is that much older than the block being
//! emitted. The gain therefore starts moving before the transient arrives, and
//! the residual overshoot of a finite attack time is removed instead of being
//! clipped. The delay is reported from [`EffectProcessor::latency_samples`]; a
//! wrong value here misaligns every other track in the project through PDC, so
//! a test pins the exact number.
//!
//! # Real-time safety
//!
//! The delay line, the dry snapshot and the wet scratch are all allocated in
//! [`Compressor::prepare`]. `process` allocates nothing and performs no IO.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{db_to_gain, gain_to_db, one_pole_coeff};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Level above which gain reduction starts, in decibels.
pub const PARAM_THRESHOLD: u16 = 0;
/// Compression ratio, `1.0` = no compression.
pub const PARAM_RATIO: u16 = 1;
/// Attack time in milliseconds.
pub const PARAM_ATTACK: u16 = 2;
/// Release time in milliseconds.
pub const PARAM_RELEASE: u16 = 3;
/// Soft-knee width in decibels; `0.0` is a hard knee.
pub const PARAM_KNEE: u16 = 4;
/// Output makeup gain in decibels.
pub const PARAM_MAKEUP: u16 = 5;
/// Look-ahead window in milliseconds.
pub const PARAM_LOOKAHEAD: u16 = 6;
/// Whether the sidechain filter is engaged.
pub const PARAM_SIDECHAIN_ENABLED: u16 = 7;
/// Sidechain filter centre frequency in hertz.
pub const PARAM_SIDECHAIN_HZ: u16 = 8;
/// How much of the filtered sidechain is used, in percent.
pub const PARAM_SIDECHAIN_DEPTH: u16 = 9;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 10;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 11;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_COMPRESSOR,
    key: "compressor",
    label: "Compressor",
    category: EffectCategory::Dynamics,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: true,
    is_analysis_only: false,
};

/// The static gain curve, shared by the compressor and the limiter.
///
/// `level_db` is the detector's level in decibels and `threshold_db` is where
/// reduction starts. `knee_db >= 0` is the total width of the soft knee
/// centred on the threshold; `0.0` gives a hard knee. `ratio >= 1` is the
/// compression ratio.
///
/// Returned in decibels, relative to the input level: `0.0` means "no gain
/// reduction". The result is always non-positive for a valid ratio, and is
/// never `NaN` - a non-finite level reads as "no reduction" rather than as
/// silence, because a detector that has gone non-finite must not mute the bus.
#[must_use]
pub fn gain_computer(level_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    if !level_db.is_finite() || !threshold_db.is_finite() {
        return 0.0;
    }
    let ratio = if ratio.is_finite() {
        ratio.max(1.0)
    } else {
        1.0
    };
    let knee = if knee_db.is_finite() {
        knee_db.max(0.0)
    } else {
        0.0
    };
    let delta = level_db - threshold_db;
    let half = knee * 0.5;
    let overshoot = if knee <= 1e-6 || delta >= half {
        delta.max(0.0)
    } else if delta <= -half {
        0.0
    } else {
        // Quadratic knee: continuous in value *and* in slope with the two
        // straight segments either side of it, which is what stops a soft knee
        // from producing an audible kink as the programme crosses it.
        let shifted = delta + half;
        shifted * shifted / (2.0 * knee)
    };
    let slope = 1.0 - 1.0 / ratio;
    let reduction = -overshoot * slope;
    if reduction.is_finite() {
        reduction
    } else {
        0.0
    }
}

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
            min_value: -60.0,
            max_value: 0.0,
            default_value: -18.0,
            smoothing_ms: 10.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RATIO),
            key: "ratio",
            label: "Ratio",
            unit: ParameterUnit::Linear,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::LOGARITHMIC,
            min_value: 1.0,
            max_value: 20.0,
            default_value: 4.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_ATTACK),
            key: "attack_ms",
            label: "Attack",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.1,
            max_value: 500.0,
            default_value: 10.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RELEASE),
            key: "release_ms",
            label: "Release",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 5.0,
            max_value: 4_000.0,
            default_value: 120.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_KNEE),
            key: "knee_db",
            label: "Knee",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 24.0,
            default_value: 6.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_MAKEUP),
            key: "makeup_db",
            label: "Makeup",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 24.0,
            default_value: 0.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_LOOKAHEAD),
            key: "lookahead_ms",
            label: "Look-ahead",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 20.0,
            default_value: 5.0,
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

/// The longest look-ahead the gain line is sized for, in seconds.
///
/// Kept in step with `PARAM_LOOKAHEAD`'s `max_value`; a test pins the two
/// together so enlarging the parameter cannot silently start clamping.
const MAX_LOOKAHEAD_SECONDS: f32 = 0.02;

/// How many past blocks the look-ahead gain line can span.
///
/// The 20 ms maximum window is at most 16 blocks at 48 kHz with the engine's
/// documented 64-frame minimum block (PLAN section 3.S1); 64 entries covers every
/// supported rate and block size and never needs to be resized.
const HISTORY_BLOCKS: usize = 64;

/// One channel's sidechain filter state.
///
/// The sidechain is a **band-pass**: a one-pole high-pass at the control's
/// frequency, cascaded with a one-pole low-pass a decade above it. A single
/// corner would be a tilt rather than a band, and "ignore the rumble" is the
/// use case the control exists for - a high-pass alone with no upper bound
/// would let a cymbal wash open the detector as readily as a kick drum.
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
    /// to fall. The block-rate form of [`one_pole_coeff`] belongs to the
    /// detector's attack and release, which really are evaluated once per
    /// block.
    fn process(&mut self, input: f32, high_pole: f32, low_pole: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        // High-pass first: rumble below the corner must not reach the
        // low-pass's state, where it would linger. `y = x - x1 + p*y1` is the
        // same DC-blocker form `dsp::DcBlocker` uses.
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

/// The compressor effect.
#[derive(Debug)]
pub struct Compressor {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Threshold in decibels.
    threshold_db: f32,
    /// Ratio.
    ratio: f32,
    /// Attack time in milliseconds.
    attack_ms: f32,
    /// Release time in milliseconds.
    release_ms: f32,
    /// Knee width in decibels.
    knee_db: f32,
    /// Makeup gain in decibels.
    makeup_db: f32,
    /// Requested look-ahead in milliseconds.
    lookahead_ms: f32,
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
    /// The audio delay line that puts the detector ahead of the output.
    line: LookaheadLine,
    /// The largest look-ahead the delay line is sized for, in samples.
    max_lookahead_samples: usize,
    /// Detector level in decibels, the shared (linked) value.
    detector_db: f32,
    /// The look-ahead gain delay line, in **blocks**.
    ///
    /// Holds one gain value per processed block, and the gain read for the
    /// block being emitted is the one `ceil(delay / block)` blocks old. Primed
    /// with unity, so the first blocks after `prepare` are not silently ducked.
    gain_line: alloc::vec::Vec<f32>,
    /// Write position in `gain_line`.
    gain_write: usize,
    /// Gain reduction actually applied to the block just processed, in
    /// decibels. Published for meters.
    last_reduction_db: f32,
    /// Linear gain applied to the block just processed, including makeup.
    last_gain: f32,
    /// Per-channel sidechain filter state.
    sidechain: [SidechainState; MAX_CHANNELS],
    /// Preallocated dry snapshot, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer, `max_block`.
    wet_buf: alloc::vec::Vec<f32>,
    /// Preallocated delayed-signal buffer, `max_block`.
    delayed: alloc::vec::Vec<f32>,
    /// Preallocated sidechain scratch, `max_block`.
    side_buf: alloc::vec::Vec<f32>,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_THRESHOLD))
    }
}

impl Compressor {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            threshold_db: table[PARAM_THRESHOLD as usize].default_value,
            ratio: table[PARAM_RATIO as usize].default_value,
            attack_ms: table[PARAM_ATTACK as usize].default_value,
            release_ms: table[PARAM_RELEASE as usize].default_value,
            knee_db: table[PARAM_KNEE as usize].default_value,
            makeup_db: table[PARAM_MAKEUP as usize].default_value,
            lookahead_ms: table[PARAM_LOOKAHEAD as usize].default_value,
            sidechain_enabled: false,
            sidechain_hz: table[PARAM_SIDECHAIN_HZ as usize].default_value,
            sidechain_depth: table[PARAM_SIDECHAIN_DEPTH as usize].default_value,
            mix_percent: 100.0,
            wet: 1.0,
            bypassed: false,
            sample_rate: 48_000.0,
            max_lookahead_samples: 0,
            line: LookaheadLine::new(1),
            detector_db: -144.0,
            last_reduction_db: 0.0,
            last_gain: 1.0,
            gain_line: alloc::vec![1.0; HISTORY_BLOCKS],
            gain_write: 0,
            sidechain: [SidechainState::default(); MAX_CHANNELS],
            table,
            dry: alloc::vec::Vec::new(),
            wet_buf: alloc::vec::Vec::new(),
            delayed: alloc::vec::Vec::new(),
            side_buf: alloc::vec::Vec::new(),
            max_block: 0,
        }
    }

    /// The configured look-ahead, in samples.
    ///
    /// This is the window the gain is computed over, and the figure PDC must
    /// compensate. Clamped to the delay the gain line can actually represent so
    /// a large block size cannot silently ask for more look-ahead than the line
    /// holds.
    #[must_use]
    pub fn lookahead_samples(&self) -> usize {
        let seconds = if self.lookahead_ms > 0.0 {
            self.lookahead_ms / 1000.0
        } else {
            0.0
        };
        let requested = (seconds * self.sample_rate).round();
        let requested = if requested > 0.0 {
            requested as usize
        } else {
            0
        };
        requested.min(self.max_lookahead_samples)
    }

    /// The gain reduction the current detector state asks for, in decibels.
    ///
    /// The last value the gain computer produced, which is what a meter should
    /// display and what the tests read back.
    #[must_use]
    pub fn current_reduction_db(&self) -> f32 {
        self.last_reduction_db
    }

    /// The linear gain the block just processed was scaled by.
    #[must_use]
    pub fn current_gain(&self) -> f32 {
        self.last_gain
    }
}

impl EffectProcessor for Compressor {
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
        // Every allocation this effect will ever make happens here.
        //
        // The gain line stages one value per block, so the look-ahead it can
        // represent is bounded by the block size. A block of 64 frames at
        // 48 kHz is 1.33 ms, so the 20 ms maximum window needs 15 stages; the
        // line is sized for the larger of that and the block-rate floor below,
        // capped by `HISTORY_BLOCKS`.
        let samples_per_block = max_block.max(1);
        self.max_lookahead_samples = (self.sample_rate * MAX_LOOKAHEAD_SECONDS).ceil() as usize;
        self.line = LookaheadLine::new(self.max_lookahead_samples + 1);
        self.dry = alloc::vec![0.0; max_block];
        self.wet_buf = alloc::vec![0.0; max_block];
        self.delayed = alloc::vec![0.0; max_block];
        self.side_buf = alloc::vec![0.0; max_block];
        let _ = samples_per_block;
        self.gain_line = alloc::vec![1.0; HISTORY_BLOCKS];
        self.gain_write = 0;
        let _ = channels;
        self.detector_db = -144.0;
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
        let make_gain = db_to_gain(self.makeup_db);
        let delay = self.lookahead_samples();
        let wet = self.wet;
        let sidechain_on = self.sidechain_enabled && self.sidechain_depth > 0.0;
        let blend = (self.sidechain_depth / 100.0).clamp(0.0, 1.0);
        // High-pass at the control's frequency, low-pass a decade above it.
        // Both are per-sample pole positions, because they describe a filter
        // corner rather than a smoothing time.
        let high_pole = sample_pole(self.sidechain_hz, self.sample_rate);
        let low_pole = sample_pole(self.sidechain_hz * SIDECHAIN_BAND_RATIO, self.sample_rate);

        // -- 1. Linked detector, read ahead of the audio --
        //
        // Each channel is optionally filtered first, then the maximum across
        // channels is taken. Filtering per channel and linking afterwards is
        // the only order that keeps the filter consistent with the signal it is
        // derived from; linking raw channels and filtering the result would
        // need a second, different filter state.
        let mut linked_peak = 0.0_f32;
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            self.dry[..frames].copy_from_slice(source);
            let level = if sidechain_on {
                let state = &mut self.sidechain[channel.min(MAX_CHANNELS - 1)];
                let mut filtered_peak = 0.0_f32;
                for (index, sample) in self.dry[..frames].iter().enumerate() {
                    let filtered = state.process(*sample, high_pole, low_pole);
                    self.side_buf[index] = filtered;
                    filtered_peak = filtered_peak.max(filtered.abs());
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

        // The sidechain depth crossfades detector sources *per channel* above;
        // at 0% the filtered path is not consulted at all, which is why the
        // blend factor below is applied to the linked peak and not to the raw
        // samples.
        let mut peak_db = gain_to_db(linked_peak);
        if sidechain_on && blend < 1.0 {
            // Partial depth: blend the filtered detector back toward the raw
            // one so a small amount of sidechain is a tonal change rather than
            // a different response.
            let raw_db = gain_to_db(
                self.dry[..frames]
                    .iter()
                    .fold(0.0_f32, |m, s| m.max(s.abs())),
            );
            peak_db = blend * peak_db + (1.0 - blend) * raw_db;
        }

        let detector = if !peak_db.is_finite() {
            -144.0
        } else {
            peak_db
        };
        // Look-ahead changes *what* the attack time smooths. Without it, the
        // attack smooths the level of the block being emitted, which is the
        // only information available. With it, the reduction has to be complete
        // by the time the transient leaves the delay line, so the attack is
        // evaluated over the look-ahead window rather than over the block: the
        // coefficient is the same, but it is applied `span` times per block.
        //
        // That is what makes a slow attack still catch the transient it was
        // given a window to see. It is deliberately *not* an instant attack -
        // the attack control still shapes the onset - it is simply allowed to
        // begin `delay` samples earlier.
        let span = if delay > 0 {
            ((delay + frames - 1) / frames.max(1)).max(1)
        } else {
            1
        };
        for _ in 0..span {
            let coefficient = if detector > self.detector_db {
                attack
            } else {
                release
            };
            self.detector_db += (detector - self.detector_db) * coefficient;
        }
        if !self.detector_db.is_finite() {
            self.detector_db = -144.0;
        }

        // -- 1b. Look-ahead: the detector leads the audio by one window --
        //
        // The detector above reads the input as it arrives, and the audio is
        // delayed by `delay` samples below, so the detector is genuinely ahead
        // of the samples it controls. Two things have to follow from that or
        // the delay buys nothing:
        //
        // 1. The detector must react to a transient the instant it enters the
        //    line, not when the attack curve has finished travelling. It is
        //    therefore driven by the running *peak* of the window, which the
        //    attack/release smoothing below converts into gain; a target above
        //    the smoothed value uses the attack coefficient, so the reduction
        //    is under way while the transient is still crossing the line.
        // 2. The gain applied to the block emerging now must be the *most
        //    severe* gain the window has asked for, not the gain of the block
        //    that happens to be leaving the line. That is the sliding minimum
        //    of the per-block gains below.
        //
        // Together those mean the reduction is waiting for the transient when
        // it emerges, which is the whole purpose of the look-ahead.
        let reduction_db = gain_computer(
            self.detector_db,
            self.threshold_db,
            self.ratio,
            self.knee_db,
        );
        let instantaneous_gain = db_to_gain(reduction_db).clamp(0.0, 1.0);

        // Push this block's gain and take the minimum over the window.
        let slots = self.gain_line.len().max(1);
        let span = ((delay + frames - 1) / frames.max(1)).clamp(1, slots);
        self.gain_line[self.gain_write % slots] = instantaneous_gain;
        self.gain_write = self.gain_write.wrapping_add(1);
        let mut gain = 1.0_f32;
        for offset in 0..span {
            let index = (self.gain_write + slots - 1 - offset) % slots;
            let value = self.gain_line[index];
            if value.is_finite() {
                gain = gain.min(value);
            }
        }
        gain = gain.clamp(0.0, 1.0);
        self.last_reduction_db = gain_to_db(gain);
        self.last_gain = gain * make_gain;

        // -- 2. Delay the audio, then apply the held (linked) gain --
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            self.dry[..frames].copy_from_slice(source);
            self.line
                .process(&self.dry[..frames], &mut self.delayed[..frames], delay);
            for (index, sample) in self.delayed[..frames].iter().enumerate() {
                self.wet_buf[index] = *sample * gain * make_gain;
            }

            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let wet_sample = self.wet_buf.get(index).copied().unwrap_or(0.0);
                    // The dry term is added only when the mix actually wants
                    // it: `NaN * 0.0` is `NaN`, so folding a non-finite input
                    // through a fully wet mix would poison the output.
                    *out = if wet >= 1.0 {
                        wet_sample
                    } else if wet <= 0.0 {
                        self.dry.get(index).copied().unwrap_or(0.0)
                    } else {
                        let dry_sample = self.dry.get(index).copied().unwrap_or(0.0);
                        wet_sample * wet + dry_sample * (1.0 - wet)
                    };
                }
            }
        }
    }

    fn reset(&mut self) {
        self.line.reset();
        for state in self.sidechain.iter_mut() {
            state.reset();
        }
        self.detector_db = -144.0;
        self.last_reduction_db = 0.0;
        self.last_gain = 1.0;
        self.gain_line.iter_mut().for_each(|g| *g = 1.0);
        self.gain_write = 0;
        for sample in self.wet_buf.iter_mut() {
            *sample = 0.0;
        }
        for sample in self.delayed.iter_mut() {
            *sample = 0.0;
        }
    }

    fn latency_samples(&self) -> usize {
        // The dry path is delayed by the look-ahead window, so PDC must
        // compensate exactly that. Reporting zero would pull this track
        // forward against every other one by the window length.
        self.lookahead_samples()
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
            PARAM_RATIO => self.ratio = value.max(1.0),
            PARAM_ATTACK => self.attack_ms = value,
            PARAM_RELEASE => self.release_ms = value,
            PARAM_KNEE => self.knee_db = value,
            PARAM_MAKEUP => self.makeup_db = value,
            PARAM_LOOKAHEAD => self.lookahead_ms = value,
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
            PARAM_RATIO => Some(self.ratio),
            PARAM_ATTACK => Some(self.attack_ms),
            PARAM_RELEASE => Some(self.release_ms),
            PARAM_KNEE => Some(self.knee_db),
            PARAM_MAKEUP => Some(self.makeup_db),
            PARAM_LOOKAHEAD => Some(self.lookahead_ms),
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

/// The look-ahead delay line for the audio path.
///
/// A ring rather than a shifting buffer, so a 20 ms window at 96 kHz costs no
/// per-sample move. The read pointer trails the write pointer by exactly
/// `delay`, which is what puts the detector ahead of the audio it controls.
#[derive(Debug)]
struct LookaheadLine {
    /// Samples, `max_delay + 1` long.
    data: alloc::vec::Vec<f32>,
    /// Write position.
    write: usize,
}

impl LookaheadLine {
    /// Sizes the line for `capacity` samples; allocates exactly once.
    fn new(capacity: usize) -> Self {
        Self {
            data: alloc::vec![0.0; capacity.max(1)],
            write: 0,
        }
    }

    /// Pushes `input` and yields the sample `delay` positions behind it.
    fn process(&mut self, input: &[f32], output: &mut [f32], delay: usize) {
        let len = self.data.len();
        if len == 0 {
            return;
        }
        let delay = delay.min(len.saturating_sub(1));
        let mut write = self.write % len;
        for (index, sample) in input.iter().enumerate() {
            if index >= output.len() {
                break;
            }
            let value = if sample.is_finite() { *sample } else { 0.0 };
            let read = (write + len - delay) % len;
            output[index] = self.data[read];
            self.data[write] = value;
            write = if write + 1 == len { 0 } else { write + 1 };
        }
        self.write = write;
    }

    /// Clears the line.
    fn reset(&mut self) {
        self.data.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
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
/// (`y = x - x1 + p*y1`, the same form as
/// [`DcBlocker`](crate::effects::util::dsp::DcBlocker)); a low-pass wants
/// `1 - p`. A non-finite or non-positive frequency fails safe to `0.0`, which
/// makes a high-pass a pass-through and a low-pass wide open - the transparent
/// degenerate case rather than a silent sidechain.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;
    const CHUNK: usize = 256;

    fn make() -> Compressor {
        let mut effect = Compressor::new(ParameterAddress::effect(0, 0, PARAM_THRESHOLD));
        effect.prepare(SR, CHUNK, 2);
        effect
    }

    /// Processes one stereo block through the real `process`.
    fn block(effect: &mut Compressor, left: &mut [f32], right: &mut [f32], frame: i64) {
        let mut views: [&mut [f32]; 2] = [left, right];
        let mut buffer = AudioBuffer::new(&mut views);
        let ctx = RenderContext::new(SR, views_frames(&buffer), frame, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }

    fn views_frames(buffer: &AudioBuffer<'_>) -> usize {
        buffer.frames()
    }

    /// Runs `blocks` blocks of a sine through a stereo pair and returns the
    /// settled peak of each channel.
    ///
    /// The peak is measured over the *second half* of the run so the
    /// measurement is of the steady state rather than of the onset.
    fn measure_sine(effect: &mut Compressor, amplitude: f32, hz: f32, blocks: usize) -> (f32, f32) {
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        let mut peaks = (0.0_f32, 0.0_f32);
        let mut produced = 0usize;
        for round in 0..blocks {
            for n in 0..CHUNK {
                let phase = 2.0 * PI * hz * (produced + n) as f32 / SR;
                let sample = sin_poly(phase) * amplitude;
                left[n] = sample;
                right[n] = sample;
            }
            block(effect, &mut left, &mut right, produced as i64);
            if round >= blocks / 2 {
                for (index, sample) in left.iter().enumerate() {
                    let _ = index;
                    peaks.0 = peaks.0.max(sample.abs());
                }
                for sample in right.iter() {
                    peaks.1 = peaks.1.max(sample.abs());
                }
            }
            produced += CHUNK;
        }
        peaks
    }

    /// A DC-ish signal (constant level) for detector measurements.
    fn measure_dc(effect: &mut Compressor, level: f32, blocks: usize) -> (f32, f32) {
        let mut left = alloc::vec![level; CHUNK];
        let mut right = alloc::vec![level; CHUNK];
        let mut peaks = (0.0_f32, 0.0_f32);
        for round in 0..blocks {
            left.iter_mut().for_each(|s| *s = level);
            right.iter_mut().for_each(|s| *s = level);
            block(effect, &mut left, &mut right, (round * CHUNK) as i64);
            if round >= blocks / 2 {
                for sample in left.iter() {
                    peaks.0 = peaks.0.max(sample.abs());
                }
                for sample in right.iter() {
                    peaks.1 = peaks.1.max(sample.abs());
                }
            }
        }
        peaks
    }

    // -- Identity and table --

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, crate::effects::registry::KIND_COMPRESSOR);
        assert_eq!(d.kind, 0x0000_0200);
        assert_eq!(d.key, "compressor");
        assert_eq!(d.label, "Compressor");
        assert_eq!(d.category, EffectCategory::Dynamics);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(
            d.has_latency,
            "a look-ahead compressor must declare its latency"
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
            assert_eq!(spec.address.effect_slot(), 0);
            assert_eq!(spec.address.index, 0);
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
    fn parameter_keys_are_unique_and_the_addresses_follow_the_slot() {
        let table = parameter_table(ParameterAddress::effect(5, 3, 0));
        for (i, a) in table.iter().enumerate() {
            assert_eq!(a.address.effect_slot(), 3);
            assert_eq!(a.address.index, 5);
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
        assert_eq!(effect.get_parameter(PARAM_THRESHOLD), Some(-60.0));
        effect.set_parameter(PARAM_RATIO, f32::NAN);
        assert_eq!(
            effect.get_parameter(PARAM_RATIO),
            Some(effect.table[PARAM_RATIO as usize].default_value)
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
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -40.0);
        effect.set_parameter(PARAM_MIX, 0.0);
        let mut left = alloc::vec![0.5_f32; CHUNK];
        let mut right = alloc::vec![-0.5_f32; CHUNK];
        let expected_left = left.clone();
        let expected_right = right.clone();
        block(&mut effect, &mut left, &mut right, 0);
        for (i, (got, want)) in left.iter().zip(expected_left.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-6,
                "left sample {i}: {got} vs {want}"
            );
        }
        for (i, (got, want)) in right.iter().zip(expected_right.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-6,
                "right sample {i}: {got} vs {want}"
            );
        }
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_rather_than_overrunning() {
        let mut effect = make();
        let mut left = alloc::vec![0.5_f32; 512];
        let mut right = alloc::vec![0.5_f32; 512];
        let expected = left.clone();
        block(&mut effect, &mut left, &mut right, 0);
        assert_eq!(left, expected, "oversized block must be a safe no-op");
    }

    #[test]
    fn non_finite_input_never_reaches_the_output() {
        let mut effect = make();
        effect.set_parameter(PARAM_LOOKAHEAD, 2.0);
        let mut left = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        let mut right = alloc::vec![f32::NAN; 4];
        block(&mut effect, &mut left, &mut right, 0);
        for (i, sample) in left.iter().enumerate() {
            assert!(sample.is_finite(), "left {i} is {sample}");
        }
        for (i, sample) in right.iter().enumerate() {
            assert!(sample.is_finite(), "right {i} is {sample}");
        }
        // And the state must be clean: a following normal block is sane.
        let mut left = alloc::vec![0.4_f32; CHUNK];
        let mut right = alloc::vec![0.4_f32; CHUNK];
        block(&mut effect, &mut left, &mut right, 4);
        for sample in left.iter().chain(right.iter()) {
            assert!(sample.is_finite(), "poisoned state: {sample}");
        }
    }

    #[test]
    fn output_stays_finite_under_every_extreme_parameter_at_once() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            let spec = effect.table[sub as usize];
            effect.set_parameter(sub, spec.max_value);
        }
        for round in 0..32 {
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
    fn reset_clears_the_gain_line_and_the_detector() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -40.0);
        effect.set_parameter(PARAM_RATIO, 20.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        let mut left = alloc::vec![1.0_f32; CHUNK];
        let mut right = alloc::vec![1.0_f32; CHUNK];
        for round in 0..8 {
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(
            effect.gain_line.iter().any(|g| *g < 1.0),
            "the gain line should hold a reduction"
        );
        effect.reset();
        assert!(
            effect.gain_line.iter().all(|g| *g == 1.0),
            "reset must return the gain line to unity"
        );
        assert_eq!(effect.detector_db, -144.0);
        assert_eq!(effect.last_reduction_db, 0.0);
    }

    // -- Latency --

    #[test]
    fn reported_latency_is_exactly_the_lookahead_window() {
        let mut effect = make();
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        assert_eq!(effect.latency_samples(), 240, "5 ms at 48 kHz");
        effect.set_parameter(PARAM_LOOKAHEAD, 1.0);
        assert_eq!(effect.latency_samples(), 48);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        assert_eq!(
            effect.latency_samples(),
            0,
            "no look-ahead means no latency to report"
        );
    }

    #[test]
    fn the_delay_line_is_sized_for_the_largest_advertised_lookahead() {
        let spec = parameter_table(ParameterAddress::effect(0, 0, 0))[PARAM_LOOKAHEAD as usize];
        let needed = (SR * spec.max_value / 1000.0).ceil() as usize;
        assert!(
            (SR * MAX_LOOKAHEAD_SECONDS).ceil() as usize >= needed,
            "MAX_LOOKAHEAD_SECONDS is smaller than the advertised maximum"
        );
        // And the parameter setter saturates at that same bound.
        let mut effect = make();
        effect.set_parameter(PARAM_LOOKAHEAD, 1_000.0);
        assert_eq!(effect.lookahead_samples(), needed);
    }

    #[test]
    fn the_lookahead_actually_delays_the_audio() {
        let mut effect = make();
        effect.set_parameter(PARAM_LOOKAHEAD, 2.0);
        effect.set_parameter(PARAM_THRESHOLD, 0.0);
        effect.set_parameter(PARAM_RATIO, 1.0);
        // With ratio 1 and a threshold of 0 dB the gain computer never reduces,
        // so the output is a pure delay of the input.
        let delay = effect.latency_samples();
        assert_eq!(delay, 96);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        block(&mut effect, &mut left, &mut right, 0);
        for (i, sample) in left.iter().enumerate().take(delay) {
            assert_eq!(*sample, 0.0, "sample {i} should still be in the look-ahead");
        }
    }

    // -- Gain reduction --

    #[test]
    fn a_signal_below_the_threshold_is_left_alone() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -6.0);
        effect.set_parameter(PARAM_RATIO, 8.0);
        effect.set_parameter(PARAM_KNEE, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_MAKEUP, 0.0);
        // -20 dBFS is well below a -6 dB threshold.
        let peak = measure_dc(&mut effect, 0.1, 40).0;
        assert!(
            (peak - 0.1).abs() < 1e-3,
            "a quiet signal was reduced to {peak}, expected ~0.1"
        );
        assert!(effect.current_reduction_db().abs() < 1e-3);
    }

    #[test]
    fn a_signal_above_the_threshold_is_reduced() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_RATIO, 4.0);
        effect.set_parameter(PARAM_KNEE, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_MAKEUP, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 50.0);
        // 0 dBFS is 20 dB above the threshold; at 4:1 that is 15 dB of
        // reduction, i.e. a settled output near -15 dBFS = 0.178.
        let peak = measure_dc(&mut effect, 1.0, 80).0;
        let expected = db_to_gain(-15.0);
        assert!(
            (peak - expected).abs() < 0.02,
            "expected ~{expected} (-15 dBFS), measured {peak}"
        );
    }

    #[test]
    fn a_higher_ratio_reduces_more() {
        let mut gentle = make();
        let mut firm = make();
        for effect in [&mut gentle, &mut firm] {
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_KNEE, 0.0);
            effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
            effect.set_parameter(PARAM_ATTACK, 0.1);
            effect.set_parameter(PARAM_RELEASE, 50.0);
        }
        gentle.set_parameter(PARAM_RATIO, 2.0);
        firm.set_parameter(PARAM_RATIO, 12.0);
        let low = measure_dc(&mut gentle, 1.0, 60).0;
        let high = measure_dc(&mut firm, 1.0, 60).0;
        assert!(
            high < low,
            "12:1 gave {high} but 2:1 gave {low}; the ratio is not acting"
        );
    }

    #[test]
    fn a_soft_knee_reduces_less_just_below_the_threshold_than_above_it() {
        // The knee's job is to make the transition continuous. Below the
        // threshold by less than half the knee there must be *some* reduction,
        // and further below there must be none.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_RATIO, 4.0);
        effect.set_parameter(PARAM_KNEE, 12.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 50.0);

        let near = measure_dc(&mut effect, db_to_gain(-18.0), 60).0;
        effect.reset();
        let input = db_to_gain(-18.0);
        assert!(
            near < input,
            "inside the knee ({near}) there should be some reduction of {input}"
        );

        effect.reset();
        let far = measure_dc(&mut effect, db_to_gain(-40.0), 60).0;
        assert!(
            (far - db_to_gain(-40.0)).abs() < 1e-3,
            "far below the knee there must be no reduction, got {far}"
        );
    }

    #[test]
    fn the_gain_computer_matches_the_published_curve() {
        // Hard knee: 10 dB over a 4:1 threshold is 7.5 dB of reduction.
        let hard = gain_computer(0.0, -10.0, 4.0, 0.0);
        assert!((hard + 7.5).abs() < 1e-3, "hard knee gave {hard}");

        // Below the threshold there is nothing to do.
        assert_eq!(gain_computer(-30.0, -10.0, 4.0, 0.0), 0.0);

        // A ratio of 1 must be exactly transparent everywhere.
        for level in [-60.0_f32, -10.0, 0.0, 24.0] {
            assert_eq!(gain_computer(level, -20.0, 1.0, 6.0), 0.0);
        }

        // A degenerate knee of zero is a hard knee, and the knee curve is
        // continuous with both straight segments.
        let at_edge = gain_computer(-10.0 + 3.0, -10.0, 4.0, 6.0);
        let below = gain_computer(-10.0 + 3.0 - 1e-3, -10.0, 4.0, 6.0);
        assert!((at_edge - below).abs() < 1e-3);
        // NaNs must not propagate into a gain.
        assert_eq!(gain_computer(f32::NAN, -10.0, 4.0, 0.0), 0.0);
        assert_eq!(gain_computer(0.0, f32::NAN, 4.0, 0.0), 0.0);
    }

    // -- Stereo link --

    #[test]
    fn a_loud_left_channel_reduces_the_silent_right_channel_too() {
        // The core property of a linked compressor. An independent design
        // would leave the right channel untouched, which is what shifts the
        // stereo image.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_RATIO, 8.0);
        effect.set_parameter(PARAM_KNEE, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 100.0);

        let mut left = alloc::vec![1.0_f32; CHUNK];
        let mut right = alloc::vec![0.2_f32; CHUNK];
        let mut right_out = 0.0_f32;
        let mut left_out = 0.0_f32;
        for round in 0..40 {
            // Both channels carry the same programme; only the level differs,
            // so the *gain* on each channel must come out identical.
            left.iter_mut().for_each(|s| *s = 1.0);
            right.iter_mut().for_each(|s| *s = 0.2);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            if round >= 20 {
                left_out = left[0];
                right_out = right[0];
            }
        }
        assert!(left_out.abs() < 1.0, "the loud channel was not reduced");
        let ratio = left_out / right_out;
        assert!(
            (ratio - 5.0).abs() < 0.2,
            "channels were not reduced by the same gain: ratio {ratio}, expected 5.0"
        );
    }

    #[test]
    fn the_link_is_taken_from_the_loudest_channel_not_the_first() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_RATIO, 8.0);
        effect.set_parameter(PARAM_KNEE, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 100.0);

        // The loud side is the *right* one this time.
        let mut left = alloc::vec![0.2_f32; CHUNK];
        let mut right = alloc::vec![1.0_f32; CHUNK];
        let mut gain_quiet = 0.0_f32;
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = 0.2);
            right.iter_mut().for_each(|s| *s = 1.0);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            if round >= 20 {
                gain_quiet = left[0] / 0.2;
            }
        }
        assert!(
            gain_quiet < 0.2,
            "the quiet channel's gain is {gain_quiet}; the detector ignored the right side"
        );
    }

    #[test]
    fn an_identical_stereo_signal_keeps_its_balance() {
        // A signal that is equal in both channels must come out equal: the
        // link is what makes that true by construction.
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -24.0);
        effect.set_parameter(PARAM_RATIO, 6.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 3.0);
        let (left_peak, right_peak) = measure_sine(&mut effect, 0.9, 220.0, 60);
        assert!(
            (left_peak - right_peak).abs() < 1e-6,
            "linked channels diverged: {left_peak} vs {right_peak}"
        );
    }

    #[test]
    fn stereo_state_is_per_channel_so_the_sidechain_filter_does_not_leak() {
        let mut effect = make();
        effect.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
        effect.set_parameter(PARAM_SIDECHAIN_HZ, 200.0);
        // Drive only the left side with a tone inside the sidechain band. A DC
        // signal would be removed by the high-pass, leaving the state at zero
        // and making the test vacuous.
        let mut left: alloc::vec::Vec<f32> = (0..CHUNK)
            .map(|n| sin_poly(2.0 * PI * 400.0 * n as f32 / SR))
            .collect();
        let mut right = alloc::vec![0.0_f32; CHUNK];
        block(&mut effect, &mut left, &mut right, 0);
        assert!(
            effect.sidechain[0].low.abs() > 1e-3,
            "the left filter should have moved, got {}",
            effect.sidechain[0].low
        );
        assert_eq!(
            effect.sidechain[1].low, 0.0,
            "the right channel's filter picked up the left channel's state"
        );
    }

    // -- Sidechain --

    #[test]
    fn the_sidechain_filter_changes_how_the_detector_responds() {
        let mut wide = make();
        let mut filtered = make();
        for effect in [&mut wide, &mut filtered] {
            effect.set_parameter(PARAM_THRESHOLD, -30.0);
            effect.set_parameter(PARAM_RATIO, 8.0);
            effect.set_parameter(PARAM_KNEE, 0.0);
            effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
            effect.set_parameter(PARAM_ATTACK, 0.1);
            effect.set_parameter(PARAM_RELEASE, 100.0);
        }
        filtered.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
        filtered.set_parameter(PARAM_SIDECHAIN_HZ, 150.0);
        filtered.set_parameter(PARAM_SIDECHAIN_DEPTH, 100.0);

        // The sidechain's high-pass sits at 150 Hz, so a 30 Hz tone is cut
        // before it reaches the detector and the gate reduces less.
        let plain = measure_sine(&mut wide, 0.9, 30.0, 80).0;
        let sidechained = measure_sine(&mut filtered, 0.9, 30.0, 80).0;
        assert!(
            sidechained > plain,
            "the sidechain filter had no effect: {sidechained} vs {plain}"
        );
    }

    #[test]
    fn the_sidechain_band_passes_the_programme_that_matters() {
        // The complement of the test above: a tone well inside the band must
        // drive the detector and produce the same reduction the unfiltered
        // signal would. Without this, "the sidechain reduces less" would also
        // pass if the sidechain were simply broken and always at zero.
        let configure = |effect: &mut Compressor, filtered: bool| {
            effect.set_parameter(PARAM_THRESHOLD, -30.0);
            effect.set_parameter(PARAM_RATIO, 8.0);
            effect.set_parameter(PARAM_KNEE, 0.0);
            effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
            effect.set_parameter(PARAM_ATTACK, 0.1);
            effect.set_parameter(PARAM_RELEASE, 100.0);
            effect.set_parameter(PARAM_SIDECHAIN_HZ, 100.0);
            effect.set_parameter(PARAM_SIDECHAIN_ENABLED, if filtered { 1.0 } else { 0.0 });
        };

        // 300 Hz sits comfortably inside the 100 Hz..1 kHz band, where both
        // sections are close to unity.
        let mut filtered = make();
        configure(&mut filtered, true);
        let filtered_peak = measure_sine(&mut filtered, 0.9, 300.0, 80).0;

        let mut plain = make();
        configure(&mut plain, false);
        let unfiltered_peak = measure_sine(&mut plain, 0.9, 300.0, 80).0;

        let difference = (filtered_peak - unfiltered_peak).abs();
        assert!(
            difference < 0.05,
            "a 300 Hz tone should pass the sidechain band: {filtered_peak} vs {unfiltered_peak}"
        );
    }

    #[test]
    fn sidechain_depth_zero_is_the_same_as_sidechain_off() {
        let mut a = make();
        let mut b = make();
        for effect in [&mut a, &mut b] {
            effect.set_parameter(PARAM_THRESHOLD, -20.0);
            effect.set_parameter(PARAM_RATIO, 4.0);
            effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
            effect.set_parameter(PARAM_ATTACK, 0.1);
        }
        a.set_parameter(PARAM_SIDECHAIN_ENABLED, 0.0);
        b.set_parameter(PARAM_SIDECHAIN_ENABLED, 1.0);
        b.set_parameter(PARAM_SIDECHAIN_DEPTH, 0.0);
        let plain = measure_sine(&mut a, 0.9, 500.0, 40).0;
        let zero_depth = measure_sine(&mut b, 0.9, 500.0, 40).0;
        assert!((plain - zero_depth).abs() < 1e-6);
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

    // -- Look-ahead behaviour --

    #[test]
    fn lookahead_reduces_the_emerging_transient() {
        // The mechanism, stated directly: the detector reads the input as it
        // arrives while the audio path is delayed, so the reduction is already
        // in place by the time the loud samples leave the line. Without
        // look-ahead the loud samples emerge in the block that first asks for
        // reduction, so they get through at (almost) the old gain.
        //
        // Both compressors are settled on a quiet level first. Without that
        // settling the comparison would be dominated by the detector travelling
        // up from silence rather than by the look-ahead.
        let mut without = make();
        let mut with = make();
        for effect in [&mut without, &mut with] {
            effect.set_parameter(PARAM_THRESHOLD, -30.0);
            effect.set_parameter(PARAM_RATIO, 20.0);
            effect.set_parameter(PARAM_KNEE, 0.0);
            effect.set_parameter(PARAM_RELEASE, 200.0);
            effect.set_parameter(PARAM_ATTACK, 40.0);
        }
        without.set_parameter(PARAM_LOOKAHEAD, 0.0);
        with.set_parameter(PARAM_LOOKAHEAD, 15.0);

        let settle_level = db_to_gain(-40.0);
        let settle = |effect: &mut Compressor| {
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            for round in 0..96 {
                left.iter_mut().for_each(|s| *s = settle_level);
                right.iter_mut().for_each(|s| *s = settle_level);
                block(effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            assert!(
                effect.detector_db > -41.0,
                "the detector settled at {} dB, not near -40",
                effect.detector_db
            );
        };
        settle(&mut without);
        settle(&mut with);

        // The loud burst. The peak over the whole run is the quantity that
        // matters: a limiter-shaped overshoot at the onset is exactly what
        // look-ahead exists to remove.
        let burst = 0.9_f32;
        let peak_of_burst = |effect: &mut Compressor| -> f32 {
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            let mut peak = 0.0_f32;
            for round in 0..24 {
                left.iter_mut().for_each(|s| *s = burst);
                right.iter_mut().for_each(|s| *s = burst);
                block(effect, &mut left, &mut right, ((96 + round) * CHUNK) as i64);
                for sample in left.iter() {
                    peak = peak.max(sample.abs());
                }
            }
            peak
        };

        let fast_peak = peak_of_burst(&mut without);
        let slow_peak = peak_of_burst(&mut with);

        assert!(
            slow_peak < fast_peak,
            "look-ahead did not reduce the emerging transient: {slow_peak} vs {fast_peak}"
        );
        // The look-ahead version must actually catch the burst, not merely
        // squeeze it by a hair: with a full window of warning the reduction is
        // well under way before the samples leave the line.
        assert!(
            slow_peak < fast_peak * 0.8,
            "look-ahead only reduced the transient from {fast_peak} to {slow_peak}"
        );
    }

    #[test]
    fn a_block_larger_than_prepared_is_refused_and_a_repared_one_is_not() {
        // `prepare` sizes the scratch; the effect must honour that bound rather
        // than index past it.
        let mut effect = make();
        let mut left = alloc::vec![1.0_f32; 4_800];
        let mut right = alloc::vec![1.0_f32; 4_800];
        let expected = left.clone();
        block(&mut effect, &mut left, &mut right, 0);
        assert_eq!(left, expected, "a 4800-frame block was not refused");

        // Re-preparing for the larger block must make it process normally.
        effect.prepare(SR, 4_800, 2);
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_RATIO, 4.0);
        effect.set_parameter(PARAM_ATTACK, 10.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        left.iter_mut().for_each(|s| *s = 1.0);
        right.iter_mut().for_each(|s| *s = 1.0);
        block(&mut effect, &mut left, &mut right, 0);
        assert!(
            left[0] < 1.0,
            "a re-prepared 4800-frame block should be compressed, got {}",
            left[0]
        );
    }

    #[test]
    fn a_dry_wet_mix_between_zero_and_one_interpolates_both_paths() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -30.0);
        effect.set_parameter(PARAM_RATIO, 20.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 50.0);
        effect.set_parameter(PARAM_MIX, 50.0);
        let (left, right) = measure_sine(&mut effect, 0.9, 300.0, 60);
        // 20:1 on a 0.9 peak (-0.9 dB) against a -30 dB threshold is almost
        // full limiting; a 50% blend must land strictly between the two.
        let fully_wet = 0.9 * db_to_gain(-29.1);
        assert!(left < 0.9, "the wet path was not applied: {left}");
        assert!(left > fully_wet * 0.9, "the dry path was lost: {left}");
        assert!((left - right).abs() < 1e-6);
    }

    #[test]
    fn makeup_gain_is_applied_after_the_reduction() {
        let mut effect = make();
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_RATIO, 4.0);
        effect.set_parameter(PARAM_KNEE, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 50.0);

        let without = measure_dc(&mut effect, 1.0, 40).0;
        effect.reset();
        effect.set_parameter(PARAM_MAKEUP, 6.0);
        let with = measure_dc(&mut effect, 1.0, 40).0;
        let ratio = with / without;
        assert!(
            (ratio - db_to_gain(6.0)).abs() < 0.05,
            "makeup should multiply by 2, got {ratio}"
        );
    }

    #[test]
    fn the_compressor_has_no_tail() {
        // Compression is instantaneous once the detector has settled; there is
        // nothing for an offline render to extend.
        let effect = make();
        assert_eq!(effect.tail_seconds(), 0.0);
    }

    #[test]
    fn a_single_channel_block_is_processed_correctly() {
        let mut effect = Compressor::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, CHUNK, 1);
        effect.set_parameter(PARAM_THRESHOLD, -20.0);
        effect.set_parameter(PARAM_RATIO, 4.0);
        effect.set_parameter(PARAM_KNEE, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        effect.set_parameter(PARAM_ATTACK, 0.1);
        effect.set_parameter(PARAM_RELEASE, 50.0);

        let mut channel = alloc::vec![1.0_f32; CHUNK];
        let mut peak = 0.0_f32;
        for round in 0..40 {
            channel.iter_mut().for_each(|s| *s = 1.0);
            let mut views: [&mut [f32]; 1] = [&mut channel];
            let mut buffer = AudioBuffer::new(&mut views);
            let ctx = RenderContext::new(SR, CHUNK, (round * CHUNK) as i64, 120.0, 960);
            effect.process(&mut buffer, &ctx);
            if round >= 20 {
                peak = peak.max(channel[0].abs());
            }
        }
        let expected = db_to_gain(-15.0);
        assert!(
            (peak - expected).abs() < 0.05,
            "mono compression gave {peak}, expected ~{expected}"
        );
    }

    #[test]
    fn an_empty_block_is_a_no_op() {
        let mut effect = make();
        let mut empty: [&mut [f32]; 0] = [];
        let mut buffer = AudioBuffer::new(&mut empty);
        let ctx = RenderContext::new(SR, 0, 0, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }
}

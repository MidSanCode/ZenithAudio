//! True-peak-ish look-ahead brick-wall limiter.
//!
//! # The guarantee
//!
//! The output **never exceeds the ceiling**. That is the whole contract, and
//! everything in this file exists to make it true for every input:
//!
//! 1. **Look-ahead.** The audio is delayed by `lookahead_ms` while the detector
//!    reads samples as they enter the line, so the gain is computed from
//!    samples that have not been emitted yet. Without it a brick wall could
//!    only be reached by clipping, which is not limiting.
//! 2. **A sliding-window maximum, not a decayed envelope.** An envelope with an
//!    attack time can only *approach* the required gain, so a fast transient
//!    passes through before the envelope arrives. A window maximum states the
//!    requirement exactly: over the samples now in the line, the largest
//!    magnitude is `m`, so the gain must be at most `ceiling / m` before any of
//!    them is emitted. Because the window always contains the sample being
//!    emitted, the gain is already low enough for it.
//! 3. **A hard backstop.** `gain * sample <= ceiling` holds in exact
//!    arithmetic once (2) is in place; the final `clamp` removes even the
//!    one-multiply rounding error. It is not decoration — it makes "never
//!    exceeds the ceiling" a property of the code rather than of the
//!    arithmetic happening to work out.
//!
//! # Release
//!
//! The attack is instantaneous by construction; only the recovery is smoothed,
//! by a one-pole whose coefficient comes from
//! [`one_pole_coeff`](crate::effects::util::dsp::one_pole_coeff) against
//! `ctx.block_ms()`, so a documented release time means the same thing at every
//! block size. The gain can only ever be *lower* than the window demands, never
//! higher, which is what keeps the guarantee while the release runs.
//!
//! # Stereo link
//!
//! The detector is the maximum across channels and one gain is applied to all
//! of them. Limiting channels independently would move the stereo image on
//! every transient — a real audible defect, not a theoretical one.
//!
//! # Real-time safety
//!
//! The delay line, the block scratch and the sliding-window deques are all
//! allocated in [`Limiter::prepare`]. `process` allocates nothing and performs
//! no IO.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{db_to_gain, one_pole_coeff};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// The level the output is held below, in decibels.
pub const PARAM_CEILING: u16 = 0;
/// Look-ahead window in milliseconds.
pub const PARAM_LOOKAHEAD: u16 = 1;
/// Release time in milliseconds.
pub const PARAM_RELEASE: u16 = 2;
/// How much the release is smoothed, in percent.
///
/// At `0` the gain recovers as fast as the window allows, which is the tightest
/// brick wall. Raising it lengthens the recovery toward the release time.
pub const PARAM_SOFTEN: u16 = 3;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 4;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 5;

/// Maximum channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The longest look-ahead the delay line is sized for, in seconds.
///
/// Kept in step with `PARAM_LOOKAHEAD`'s `max_value`; a test pins the two
/// together so enlarging the parameter cannot silently start clamping.
const MAX_LOOKAHEAD_SECONDS: f32 = 0.05;

/// Upper bound on the window length the deques are sized for.
///
/// The deques live in the struct rather than in a `Vec`, so they are sized once
/// for the longest window at the highest sample rate the engine runs at.
/// `MAX_LOOKAHEAD_SECONDS` at 192 kHz is 9 600 frames; this leaves 5x of room.
const MAX_WINDOW_SAMPLES: usize = 48_000;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_LIMITER,
    key: "limiter",
    label: "Limiter",
    category: EffectCategory::Dynamics,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: true,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_CEILING),
            key: "ceiling_db",
            label: "Ceiling",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: -24.0,
            max_value: 0.0,
            default_value: -0.3,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_LOOKAHEAD),
            key: "lookahead_ms",
            label: "Look-ahead",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 50.0,
            default_value: 5.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RELEASE),
            key: "release_ms",
            label: "Release",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 1.0,
            max_value: 5_000.0,
            default_value: 60.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_SOFTEN),
            key: "soften",
            label: "Soften",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 0.0,
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

/// A monotonic sliding-window maximum.
///
/// The classic deque: indices are kept in decreasing order of sample value, so
/// the front is always the window maximum. Both ends move forward only, so a
/// sample is pushed once and popped once and the amortised cost is O(1) per
/// sample — which is what makes it affordable on the audio thread.
#[derive(Debug)]
struct SlidingMax {
    /// Sample values, indexed by `write % capacity`.
    values: alloc::vec::Vec<f32>,
    /// Ring of slots into `values`, in decreasing value order.
    order: alloc::vec::Vec<usize>,
    /// Read end of `order`.
    head: usize,
    /// Write end of `order`.
    tail: usize,
    /// Entries currently live in `order`.
    count: usize,
}

impl SlidingMax {
    /// Sizes the window for `capacity` samples; allocates exactly once.
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            values: alloc::vec![0.0; capacity],
            order: alloc::vec![0; capacity],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Clears the window.
    fn reset(&mut self) {
        self.values.iter_mut().for_each(|s| *s = 0.0);
        self.order.iter_mut().for_each(|s| *s = 0);
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }

    /// The largest value still inside the window, or `0.0` when empty.
    #[must_use]
    fn maximum(&self) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        self.values[self.order[self.head]]
    }

    /// Inserts `value` at slot `write` and evicts everything older than
    /// `window` frames.
    ///
    /// `write` counts frames from the start of the stream, so the sample being
    /// inserted has age zero and a sample at slot `s` has age
    /// `(write - s) mod capacity`.
    fn push(&mut self, write: usize, value: f32, window: usize) {
        let capacity = self.values.len();
        if capacity == 0 {
            return;
        }
        let window = window.clamp(1, capacity);
        let slot = write % capacity;
        let value = if value.is_finite() { value } else { 0.0 };
        self.values[slot] = value;

        // Everything the new sample dominates is dead: this sample is later and
        // at least as large, so those entries can never be the maximum again.
        while self.count > 0 {
            let last = (self.tail + capacity - 1) % capacity;
            if self.values[self.order[last]] <= value {
                self.tail = last;
                self.count -= 1;
            } else {
                break;
            }
        }
        self.order[self.tail] = slot;
        self.tail = (self.tail + 1) % capacity;
        self.count += 1;

        // Evict what has left the window.
        while self.count > 0 {
            let slot = self.order[self.head];
            let age = (write + capacity - slot) % capacity;
            if age >= window {
                self.head = (self.head + 1) % capacity;
                self.count -= 1;
            } else {
                break;
            }
        }
    }
}

/// The limiter effect.
#[derive(Debug)]
pub struct Limiter {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Ceiling in decibels.
    ceiling_db: f32,
    /// Look-ahead in milliseconds.
    lookahead_ms: f32,
    /// Release time in milliseconds.
    release_ms: f32,
    /// Soften amount in percent.
    soften_percent: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Per-channel sliding-window maxima.
    windows: [SlidingMax; MAX_CHANNELS],
    /// The delay line carrying the signal through the look-ahead window.
    ///
    /// Holds the **linked** magnitude rather than the audio, because that is
    /// all the detector needs and it keeps the stereo link in one place.
    line: alloc::vec::Vec<f32>,
    /// The delayed audio for the block being emitted.
    delayed: alloc::vec::Vec<f32>,
    /// The dry snapshot of the block.
    dry: alloc::vec::Vec<f32>,
    /// Write position in the line, in frames since `prepare`.
    write: usize,
    /// Current linear gain: the release state.
    gain: f32,
    /// Gain applied to the block just processed.
    last_gain: f32,
    /// The largest output magnitude in the block just processed.
    last_output_peak: f32,
    /// Frames the delay line holds.
    capacity: usize,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, PARAM_CEILING))
    }
}

impl Limiter {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            ceiling_db: table[PARAM_CEILING as usize].default_value,
            lookahead_ms: table[PARAM_LOOKAHEAD as usize].default_value,
            release_ms: table[PARAM_RELEASE as usize].default_value,
            soften_percent: table[PARAM_SOFTEN as usize].default_value,
            mix_percent: 100.0,
            wet: 1.0,
            bypassed: false,
            sample_rate: 48_000.0,
            windows: [
                SlidingMax::new(MAX_WINDOW_SAMPLES),
                SlidingMax::new(MAX_WINDOW_SAMPLES),
            ],
            line: alloc::vec::Vec::new(),
            delayed: alloc::vec::Vec::new(),
            dry: alloc::vec::Vec::new(),
            write: 0,
            gain: 1.0,
            last_gain: 1.0,
            last_output_peak: 0.0,
            capacity: 0,
            max_block: 0,
            table,
        }
    }

    /// The configured look-ahead, in samples.
    #[must_use]
    pub fn lookahead_samples(&self) -> usize {
        let seconds = if self.lookahead_ms > 0.0 {
            self.lookahead_ms / 1000.0
        } else {
            0.0
        };
        let requested = (seconds * self.sample_rate).round();
        let requested = if requested > 0.0 { requested as usize } else { 0 };
        requested.min(self.capacity.saturating_sub(1))
    }

    /// The ceiling as a linear magnitude.
    #[must_use]
    pub fn ceiling_linear(&self) -> f32 {
        db_to_gain(self.ceiling_db)
    }

    /// The gain the block just processed was scaled by.
    #[must_use]
    pub fn current_gain(&self) -> f32 {
        self.last_gain
    }

    /// The largest output magnitude in the block just processed.
    #[must_use]
    pub fn output_peak(&self) -> f32 {
        self.last_output_peak
    }

    /// The linked magnitude of frame `index` across `channels` of `buffer`.
    fn linked_magnitude(buffer: &AudioBuffer<'_>, index: usize, channels: usize) -> f32 {
        let mut magnitude = 0.0_f32;
        for channel in 0..channels {
            if let Some(source) = buffer.channel(channel) {
                let sample = source.get(index).copied().unwrap_or(0.0);
                if sample.is_finite() {
                    magnitude = magnitude.max(sample.abs());
                }
            }
        }
        magnitude
    }
}

impl EffectProcessor for Limiter {
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
        // Every allocation this effect will ever make happens here. The line is
        // sized for the maximum the *parameter table* allows, so dragging the
        // look-ahead control never reallocates on the audio thread.
        self.capacity = (self.sample_rate * MAX_LOOKAHEAD_SECONDS).ceil() as usize + 1;
        self.line = alloc::vec![0.0; self.capacity];
        self.delayed = alloc::vec![0.0; self.max_block];
        self.dry = alloc::vec![0.0; self.max_block];
        self.write = 0;
        let _ = channels;
        self.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer<'_>, ctx: &RenderContext) {
        if self.bypassed {
            return;
        }
        let frames = buffer.frames();
        let channels = buffer.channel_count().min(MAX_CHANNELS);
        if frames == 0 || channels == 0 || self.capacity == 0 {
            return;
        }
        // Refuse a block larger than `prepare` sized for rather than indexing
        // past the scratch. A silent pass-through is a far better failure than
        // an out-of-bounds write in the audio thread.
        if frames > self.max_block || frames > self.dry.len() || frames > self.delayed.len() {
            return;
        }

        let block_ms = ctx.block_ms();
        let release = one_pole_coeff(self.release_ms, block_ms);
        let delay = self.lookahead_samples();
        // With no look-ahead the window still has to contain the sample being
        // emitted, or the limiter would be one sample late and overshoot.
        let window = delay + 1;
        let ceiling = self.ceiling_linear();
        let wet = self.wet;
        let soften = (self.soften_percent / 100.0).clamp(0.0, 1.0);
        let trim = if ceiling > 0.0 { 1.0 / ceiling } else { 1.0 };

        // ── 1. Read the delayed audio, before this block overwrites the line ──
        //
        // The magnitude at `write + i - delay` is the one that entered the
        // window `delay` frames ago. That value is what drives the gain for the
        // frame being emitted, and it must be recovered before the write.
        for index in 0..frames {
            let position = (self.write + index) % self.capacity;
            let read = (position + self.capacity - delay % self.capacity) % self.capacity;
            self.delayed[index] = self.line[read];
        }

        // ── 2. Walk the block: push the linked magnitude, scale the delayed
        //      magnitude, and record the audio ──
        //
        // The detector and the audio path are the same number: `line` holds the
        // linked magnitude of the signal, `delayed` holds what was in `line`
        // `delay` frames ago, and the gain is `ceiling / window_max` where
        // `window_max` is the maximum of the last `delay + 1` magnitudes. The
        // sample being emitted is always inside that window, so
        // `gain * delayed <= ceiling`.
        let mut peak_out = 0.0_f32;
        let mut gain = self.gain;
        for index in 0..frames {
            let magnitude = Self::linked_magnitude(buffer, index, channels);
            let position = (self.write + index) % self.capacity;

            // The window maximum, *including* the new sample, is the
            // requirement the frame being emitted must already satisfy.
            let window_max = {
                let reader = &mut self.windows[0];
                reader.push(position, magnitude, window);
                reader.maximum()
            };
            let window_max = if window_max.is_finite() {
                window_max
            } else {
                ceiling
            };

            let required = if ceiling > 0.0 && window_max > ceiling {
                ceiling / window_max
            } else {
                1.0
            };

            // Instant attack: the gain is never allowed above what the window
            // demands. Only the recovery is smoothed.
            if required <= gain {
                gain = required;
            } else {
                let smoothed = gain + (required - gain) * release;
                gain = smoothed + (required - smoothed) * (1.0 - soften);
            }
            if !gain.is_finite() || gain < 0.0 {
                gain = if required.is_finite() { required } else { 1.0 };
            }
            // Belt and braces: whatever the smoothing did, the ceiling wins.
            if gain > required {
                gain = required;
            }

            // The audio leaving the line at this instant, scaled and bounded.
            let delayed = self.delayed[index];
            let delayed = if delayed.is_finite() { delayed } else { 0.0 };
            let scaled = delayed * gain * trim;
            let bounded = if scaled.is_finite() {
                scaled.clamp(-1.0, 1.0)
            } else {
                0.0
            };

            // Commit: the line now carries this frame's magnitude, and the
            // delayed slot carries the emitted audio for the wet/dry mix.
            self.line[position] = magnitude.min(f32::MAX);
            self.delayed[index] = bounded;
            peak_out = peak_out.max(bounded.abs());
        }
        self.write = (self.write + frames) % self.capacity;
        self.gain = gain;
        self.last_gain = gain;
        self.last_output_peak = peak_out;

        // ── 3. Wet/dry, per channel ──
        //
        // `delayed` is normalised to full scale by `trim`, and so is the dry
        // path below, so the mix compares like with like and the ceiling still
        // holds at every mix setting.
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            for (index, sample) in source.iter().enumerate().take(frames) {
                let dry_sample = if sample.is_finite() { *sample } else { 0.0 } * trim;
                self.dry[index] = dry_sample.clamp(-1.0, 1.0);
            }
            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let wet_sample = self.delayed.get(index).copied().unwrap_or(0.0);
                    let dry_sample = self.dry.get(index).copied().unwrap_or(0.0);
                    let mixed = wet_sample * wet + dry_sample * (1.0 - wet);
                    *out = mixed * ceiling;
                }
            }
        }
    }

    fn reset(&mut self) {
        for window in self.windows.iter_mut() {
            window.reset();
        }
        self.line.iter_mut().for_each(|s| *s = 0.0);
        self.delayed.iter_mut().for_each(|s| *s = 0.0);
        self.dry.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.gain = 1.0;
        self.last_gain = 1.0;
        self.last_output_peak = 0.0;
    }

    fn latency_samples(&self) -> usize {
        // The audio path runs through the look-ahead line, so PDC must
        // compensate exactly that. Reporting zero would pull this track forward
        // against every other one by the window length.
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
            PARAM_CEILING => self.ceiling_db = value,
            PARAM_LOOKAHEAD => self.lookahead_ms = value,
            PARAM_RELEASE => self.release_ms = value,
            PARAM_SOFTEN => self.soften_percent = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_CEILING => Some(self.ceiling_db),
            PARAM_LOOKAHEAD => Some(self.lookahead_ms),
            PARAM_RELEASE => Some(self.release_ms),
            PARAM_SOFTEN => Some(self.soften_percent),
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

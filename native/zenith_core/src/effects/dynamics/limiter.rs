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
//!    one-multiply rounding error. It is not decoration - it makes "never
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
//! every transient - a real audible defect, not a theoretical one.
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
/// sample - which is what makes it affordable on the audio thread.
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
    /// Per-channel delay lines carrying the audio through the look-ahead.
    ///
    /// Separate per channel because the audio path must stay untouched in
    /// inter-channel terms; only the detector is linked.
    lines: [alloc::vec::Vec<f32>; MAX_CHANNELS],
    /// The delayed audio for the block being emitted, channel-major with a
    /// `max_block` stride.
    delayed: alloc::vec::Vec<f32>,
    /// The dry snapshot of the block.
    dry: alloc::vec::Vec<f32>,
    /// The per-frame gain the detector asked for, applied in the second pass.
    gain_buf: alloc::vec::Vec<f32>,
    /// Scratch for the limited channel currently being mixed.
    wet_buf: alloc::vec::Vec<f32>,
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
            lines: [alloc::vec::Vec::new(), alloc::vec::Vec::new()],
            delayed: alloc::vec::Vec::new(),
            dry: alloc::vec::Vec::new(),
            gain_buf: alloc::vec::Vec::new(),
            wet_buf: alloc::vec::Vec::new(),
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
        let requested = if requested > 0.0 {
            requested as usize
        } else {
            0
        };
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
        self.lines = [
            alloc::vec![0.0; self.capacity],
            alloc::vec![0.0; self.capacity],
        ];
        self.delayed = alloc::vec![0.0; self.max_block * MAX_CHANNELS];
        self.dry = alloc::vec![0.0; self.max_block];
        self.gain_buf = alloc::vec![1.0; self.max_block];
        self.wet_buf = alloc::vec![0.0; self.max_block];
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

        // -- 1. Read each channel's delayed audio, before the line is
        //      overwritten --
        //
        // Separate lines per channel: the audio must come out unaltered in
        // inter-channel terms, while the *detector* is linked. Sharing one line
        // between the two would replace both channels with the linked
        // magnitude, which is the audio equivalent of collapsing the stereo
        // image to mono.
        for channel in 0..channels {
            let line = &self.lines[channel.min(MAX_CHANNELS - 1)];
            let destination_start = channel * self.max_block;
            for index in 0..frames {
                let position = (self.write + index) % self.capacity;
                let read = (position + self.capacity - delay % self.capacity) % self.capacity;
                self.delayed[destination_start + index] = line[read];
            }
        }

        // -- 2. Walk the block: drive the linked detector from the *input*, and
        //      write the delayed audio back while the gain applies to it --
        //
        // The detector sees the block as it arrives, ahead of the delayed
        // audio, which is what the look-ahead buys. The gain is the smaller of
        // what the window demands and what the release allows, so the ceiling
        // holds at every sample.
        let mut peak_out = 0.0_f32;
        let mut gain = self.gain;
        for index in 0..frames {
            let magnitude = Self::linked_magnitude(buffer, index, channels);
            let position = (self.write + index) % self.capacity;

            // The window maximum over the audio that is still inside the line,
            // which includes every sample that has not been emitted yet.
            let window_max = {
                let reader = &mut self.windows[0];
                reader.push(position, magnitude, window);
                reader.maximum()
            };
            let window_max = if window_max.is_finite() {
                window_max
            } else {
                ceiling.max(1e-6)
            };

            let required = if ceiling > 0.0 && window_max > ceiling {
                ceiling / window_max
            } else {
                1.0
            };

            // Instant attack: the gain is never allowed above what the window
            // demands. Only the recovery is smoothed, and only upward.
            if required <= gain {
                gain = required;
            } else {
                let smoothed = gain + (required - gain) * release;
                gain = smoothed + (required - smoothed) * (1.0 - soften);
            }
            if !gain.is_finite() || gain < 0.0 {
                gain = if required.is_finite() { required } else { 1.0 };
            }
            // Whatever the smoothing decided, the ceiling has the last word on
            // the value that will be applied.
            if gain > required {
                gain = required;
            }

            // Commit the input into every channel's line, and record the frame's
            // gain so the payload can be scaled in one pass below.
            for channel in 0..channels {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                let sample = source.get(index).copied().unwrap_or(0.0);
                let sample = if sample.is_finite() { sample } else { 0.0 };
                self.lines[channel.min(MAX_CHANNELS - 1)][position] = sample;
            }
            self.gain_buf[index] = gain;
        }
        self.write = (self.write + frames) % self.capacity;
        self.gain = gain;
        self.last_gain = gain;

        // -- 3. Scale the delayed payload and mix, per channel --
        //
        // The *wet* output is the delayed audio times the frame's gain, which
        // is at most `ceiling / window_max` with the emitted sample inside the
        // window, so `|wet| <= ceiling` by construction. The dry path is the
        // untouched input at every mix setting below 100%.
        for channel in 0..channels {
            let Some(source) = buffer.channel(channel) else {
                continue;
            };
            let payload_start = channel * self.max_block;
            for (index, sample) in source.iter().enumerate().take(frames) {
                let dry_sample = if sample.is_finite() { *sample } else { 0.0 };
                self.dry[index] = dry_sample;
                let delayed = self.delayed[payload_start + index];
                let delayed = if delayed.is_finite() { delayed } else { 0.0 };
                // The bound is a backstop for the one-multiply rounding error,
                // and it is exact: the wet path can never legitimately exceed
                // the ceiling.
                let wet_sample = if ceiling > 0.0 {
                    (delayed * self.gain_buf[index]).clamp(-ceiling, ceiling)
                } else {
                    delayed * self.gain_buf[index]
                };
                self.delayed[payload_start + index] = wet_sample;
                self.wet_buf[index] = wet_sample;
            }
            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let wet_sample = self.wet_buf.get(index).copied().unwrap_or(0.0);
                    let dry_sample = self.dry.get(index).copied().unwrap_or(0.0);
                    let mixed = if wet >= 1.0 {
                        wet_sample
                    } else if wet <= 0.0 {
                        dry_sample
                    } else {
                        wet_sample * wet + dry_sample * (1.0 - wet)
                    };
                    *out = mixed;
                    peak_out = peak_out.max(mixed.abs());
                }
            }
            let _ = payload_start;
        }

        self.last_output_peak = peak_out;
    }

    fn reset(&mut self) {
        for window in self.windows.iter_mut() {
            window.reset();
        }
        for line in self.lines.iter_mut() {
            line.iter_mut().for_each(|s| *s = 0.0);
        }
        self.delayed.iter_mut().for_each(|s| *s = 0.0);
        self.dry.iter_mut().for_each(|s| *s = 0.0);
        self.gain_buf.iter_mut().for_each(|g| *g = 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;
    const CHUNK: usize = 256;

    fn make() -> Limiter {
        let mut effect = Limiter::new(ParameterAddress::effect(0, 0, PARAM_CEILING));
        effect.prepare(SR, CHUNK, 2);
        effect
    }

    /// Processes one stereo block through the real `process`.
    fn block(effect: &mut Limiter, left: &mut [f32], right: &mut [f32], frame: i64) {
        let mut views: [&mut [f32]; 2] = [left, right];
        let mut buffer = AudioBuffer::new(&mut views);
        let frames = buffer.frames();
        let ctx = RenderContext::new(SR, frames, frame, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }

    /// Processes one mono block.
    fn block_mono(effect: &mut Limiter, channel: &mut [f32], frame: i64) {
        let mut views: [&mut [f32]; 1] = [channel];
        let mut buffer = AudioBuffer::new(&mut views);
        let frames = buffer.frames();
        let ctx = RenderContext::new(SR, frames, frame, 120.0, 960);
        effect.process(&mut buffer, &ctx);
    }

    /// Drives `blocks` blocks of a signal and returns the largest output
    /// magnitude seen across the whole run.
    ///
    /// The whole run, not a settled window: the ceiling is a promise about
    /// every sample, and the onset is exactly where a limiter is most likely
    /// to break it.
    fn peak_of(effect: &mut Limiter, source: &dyn Fn(usize) -> f32, blocks: usize) -> f32 {
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        let mut peak = 0.0_f32;
        for round in 0..blocks {
            for n in 0..CHUNK {
                let sample = source(round * CHUNK + n);
                left[n] = sample;
                right[n] = sample;
            }
            block(effect, &mut left, &mut right, (round * CHUNK) as i64);
            for sample in left.iter().chain(right.iter()) {
                peak = peak.max(sample.abs());
            }
        }
        peak
    }

    // -- Identity and table --

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, crate::effects::registry::KIND_LIMITER);
        assert_eq!(d.kind, 0x0000_0201);
        assert_eq!(d.key, "limiter");
        assert_eq!(d.label, "Limiter");
        assert_eq!(d.category, EffectCategory::Dynamics);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(
            d.has_latency,
            "a look-ahead limiter must declare its latency"
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
        let table = parameter_table(ParameterAddress::effect(7, 2, 0));
        for (i, a) in table.iter().enumerate() {
            assert_eq!(a.address.effect_slot(), 2);
            assert_eq!(a.address.index, 7);
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
            assert!(
                (read - midpoint).abs() < 1e-3,
                "parameter {sub} read back {read}, expected {midpoint}"
            );
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_not_rejected() {
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, 1e9);
        assert_eq!(effect.get_parameter(PARAM_CEILING), Some(0.0));
        effect.set_parameter(PARAM_CEILING, -1e9);
        assert_eq!(effect.get_parameter(PARAM_CEILING), Some(-24.0));
        effect.set_parameter(PARAM_LOOKAHEAD, f32::NAN);
        assert_eq!(
            effect.get_parameter(PARAM_LOOKAHEAD),
            Some(effect.table[PARAM_LOOKAHEAD as usize].default_value)
        );
    }

    #[test]
    fn unknown_parameter_ordinals_are_ignored() {
        let mut effect = make();
        let before = effect.get_parameter(PARAM_CEILING);
        effect.set_parameter(999, 1.0);
        assert_eq!(effect.get_parameter(PARAM_CEILING), before);
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
        let mut left = alloc::vec![2.0_f32; 4_800];
        let mut right = alloc::vec![2.0_f32; 4_800];
        let expected = left.clone();
        block(&mut effect, &mut left, &mut right, 0);
        assert_eq!(left, expected, "an oversized block must be a safe no-op");
    }

    #[test]
    fn non_finite_input_never_reaches_the_output() {
        let mut effect = make();
        effect.set_parameter(PARAM_LOOKAHEAD, 2.0);
        let mut left = alloc::vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 3.0, -4.0, 0.5];
        let mut right = alloc::vec![f32::NAN; 6];
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
    fn reset_returns_the_gain_to_unity_and_clears_the_line() {
        let mut effect = make();
        let mut left = alloc::vec![1.0_f32; CHUNK];
        let mut right = alloc::vec![1.0_f32; CHUNK];
        for round in 0..8 {
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        assert!(
            effect.lines[0].iter().any(|s| *s != 0.0),
            "the line should hold audio"
        );
        effect.reset();
        assert!(effect.lines[0].iter().all(|s| *s == 0.0));
        assert!(effect.lines[1].iter().all(|s| *s == 0.0));
        assert_eq!(effect.current_gain(), 1.0);
        assert_eq!(effect.output_peak(), 0.0);
        assert_eq!(effect.write, 0);
    }

    #[test]
    fn the_limiter_has_no_tail() {
        let effect = make();
        assert_eq!(effect.tail_seconds(), 0.0);
    }

    #[test]
    fn an_empty_block_is_a_no_op() {
        let mut effect = make();
        let mut empty: [&mut [f32]; 0] = [];
        let mut buffer = AudioBuffer::new(&mut empty);
        let ctx = RenderContext::new(SR, 0, 0, 120.0, 960);
        effect.process(&mut buffer, &ctx);
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
        assert_eq!(spec.max_value, 50.0);
        assert!(
            MAX_LOOKAHEAD_SECONDS * 1000.0 >= spec.max_value,
            "MAX_LOOKAHEAD_SECONDS is smaller than the advertised maximum"
        );
        let mut effect = make();
        effect.set_parameter(PARAM_LOOKAHEAD, 1_000.0);
        assert_eq!(effect.latency_samples(), effect.capacity - 1);
    }

    // -- The ceiling: the core requirement --

    #[test]
    fn the_ceiling_is_never_exceeded_on_a_hot_sine() {
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -1.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        let ceiling = effect.ceiling_linear();
        // Deliberately hot: 6 dB over full scale.
        let peak = peak_of(
            &mut effect,
            &|n| 2.0 * sin_poly(2.0 * PI * 220.0 * n as f32 / SR),
            200,
        );
        assert!(
            peak <= ceiling + 1e-4,
            "the output reached {peak}, above the ceiling {ceiling}"
        );
        assert!(
            peak > ceiling * 0.5,
            "the limiter squashed the signal to {peak} instead of limiting it to {ceiling}"
        );
    }

    #[test]
    fn the_ceiling_is_never_exceeded_on_a_square_wave_with_fast_edges() {
        // A square wave is the worst case for a sample-accurate detector: its
        // edges are instantaneous, so a limiter without look-ahead has to clip
        // them.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -3.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        let ceiling = effect.ceiling_linear();
        let peak = peak_of(
            &mut effect,
            &|n| {
                let phase = (n as f32 * 300.0 / SR).fract();
                if phase < 0.5 {
                    1.5
                } else {
                    -1.5
                }
            },
            200,
        );
        assert!(
            peak <= ceiling + 1e-4,
            "the output reached {peak}, above the ceiling {ceiling}"
        );
    }

    #[test]
    fn the_ceiling_is_never_exceeded_without_lookahead() {
        // With no window at all the limiter still has to hold the ceiling: the
        // window then covers exactly the sample being emitted.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -6.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 0.0);
        let ceiling = effect.ceiling_linear();
        let peak = peak_of(
            &mut effect,
            &|n| 3.0 * sin_poly(2.0 * PI * 90.0 * n as f32 / SR),
            120,
        );
        assert!(
            peak <= ceiling + 1e-4,
            "with no look-ahead the output reached {peak}, above {ceiling}"
        );
    }

    #[test]
    fn the_ceiling_is_never_exceeded_at_every_advertised_lookahead() {
        for lookahead in [0.0_f32, 0.5, 1.0, 5.0, 20.0, 50.0] {
            let mut effect = make();
            effect.set_parameter(PARAM_CEILING, -0.3);
            effect.set_parameter(PARAM_LOOKAHEAD, lookahead);
            let ceiling = effect.ceiling_linear();
            let peak = peak_of(
                &mut effect,
                &|n| 2.5 * sin_poly(2.0 * PI * 440.0 * n as f32 / SR),
                120,
            );
            assert!(
                peak <= ceiling + 1e-4,
                "look-ahead {lookahead} ms let {peak} through, above {ceiling}"
            );
        }
    }

    #[test]
    fn the_ceiling_is_never_exceeded_at_every_advertised_ceiling() {
        for ceiling_db in [-24.0_f32, -12.0, -6.0, -3.0, -0.3, 0.0] {
            let mut effect = make();
            effect.set_parameter(PARAM_CEILING, ceiling_db);
            effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
            let ceiling = effect.ceiling_linear();
            let peak = peak_of(
                &mut effect,
                &|n| 2.0 * sin_poly(2.0 * PI * 330.0 * n as f32 / SR),
                120,
            );
            assert!(
                peak <= ceiling + 1e-4,
                "ceiling {ceiling_db} dB let {peak} through, above {ceiling}"
            );
        }
    }

    #[test]
    fn a_signal_below_the_ceiling_passes_through_unharmed() {
        // The complement of the ceiling tests: a limiter that attenuated
        // everything would satisfy "never exceeds the ceiling" trivially, so
        // transparency below the threshold has to be asserted separately.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, 0.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        effect.set_parameter(PARAM_SOFTEN, 0.0);
        let amplitude = 0.5_f32;
        let peak = peak_of(
            &mut effect,
            &|n| amplitude * sin_poly(2.0 * PI * 440.0 * n as f32 / SR),
            60,
        );
        assert!(
            (peak - amplitude).abs() < 0.02,
            "a signal below the ceiling came out at {peak}, expected {amplitude}"
        );
    }

    #[test]
    fn the_limiter_survives_sustained_overload() {
        // Twenty seconds of a signal far over full scale. The limiter must not
        // ramp, latch, or drift into a non-finite state.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -1.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        effect.set_parameter(PARAM_RELEASE, 1_000.0);
        let ceiling = effect.ceiling_linear();
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        let blocks = 2_000;
        for round in 0..blocks {
            for n in 0..CHUNK {
                let sample = 8.0 * sin_poly(2.0 * PI * 1_000.0 * (round * CHUNK + n) as f32 / SR);
                left[n] = sample;
                right[n] = sample;
            }
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(sample.is_finite(), "round {round} sample {i} is {sample}");
                assert!(
                    sample.abs() <= ceiling + 1e-4,
                    "round {round} sample {i} reached {} above {ceiling}",
                    sample.abs()
                );
            }
        }
        assert!(effect.current_gain().is_finite());
        assert!(effect.output_peak() <= ceiling + 1e-4);
    }

    #[test]
    fn an_infinite_signal_does_not_poison_the_limiter() {
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -1.0);
        for round in 0..16 {
            let mut left = alloc::vec![f32::INFINITY; CHUNK];
            let mut right = alloc::vec![f32::INFINITY; CHUNK];
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(sample.is_finite(), "round {round} sample {i} is {sample}");
            }
        }
        // And the effect still works normally afterwards.
        let ceiling = effect.ceiling_linear();
        let peak = peak_of(
            &mut effect,
            &|n| 2.0 * sin_poly(2.0 * PI * 440.0 * n as f32 / SR),
            60,
        );
        assert!(peak.is_finite() && peak <= ceiling + 1e-4);
    }

    // -- Stereo link --

    #[test]
    fn both_channels_receive_the_same_gain() {
        // A loud left and a quiet right: the link means the right is attenuated
        // by exactly the same factor, so the ratio between them is preserved.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -6.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        effect.set_parameter(PARAM_RELEASE, 50.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = 2.0);
            right.iter_mut().for_each(|s| *s = 0.5);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        // The ratio must survive: 4:1 in, 4:1 out.
        let ratio = left[0] / right[0];
        assert!(
            (ratio - 4.0).abs() < 0.05,
            "the stereo link broke: ratio {ratio}, expected 4.0"
        );
        assert!(right[0] < 0.5, "the quiet channel was not attenuated");
    }

    #[test]
    fn an_identical_stereo_signal_stays_identical() {
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -3.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..60 {
            for n in 0..CHUNK {
                let sample = 2.0 * sin_poly(2.0 * PI * 300.0 * n as f32 / SR);
                left[n] = sample;
                right[n] = sample;
            }
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            for (l, r) in left.iter().zip(right.iter()) {
                assert_eq!(l, r, "linked channels diverged");
            }
        }
    }

    #[test]
    fn a_loud_channel_limits_the_quiet_one_too() {
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -6.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        let mut first_quiet = 0.0_f32;
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = 3.0);
            right.iter_mut().for_each(|s| *s = 0.4);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
            if round == 20 {
                first_quiet = right[0];
            }
        }
        let ceiling = effect.ceiling_linear();
        assert!(
            first_quiet < 0.4 * 0.99,
            "the quiet channel was not pulled down: {first_quiet}"
        );
        assert!(first_quiet < ceiling);
    }

    // -- Behaviour and mechanism --

    #[test]
    fn the_sliding_window_maximum_tracks_its_window() {
        // The detector is the ceiling's whole foundation, so its window is
        // asserted directly: a spike must hold the maximum for exactly the
        // window length and then vanish.
        let mut window = SlidingMax::new(16);
        let mut last = 0.0;
        for n in 0..4 {
            last = {
                window.push(n, if n == 1 { 1.0 } else { 0.1 }, 8);
                window.maximum()
            };
        }
        assert_eq!(last, 1.0, "the spike left the window too early");
        let mut value = 1.0;
        for n in 4..16 {
            window.push(n, 0.1, 8);
            value = window.maximum();
        }
        assert!(
            value < 0.5,
            "the spike is still the maximum {} pushes later",
            16 - 4
        );
    }

    #[test]
    fn a_larger_window_changes_nothing_about_the_ceiling() {
        // The window length is a smoothness control, not a correctness one.
        for lookahead in [1.0_f32, 10.0, 30.0] {
            let mut effect = make();
            effect.set_parameter(PARAM_CEILING, -2.0);
            effect.set_parameter(PARAM_LOOKAHEAD, lookahead);
            let ceiling = effect.ceiling_linear();
            let peak = peak_of(
                &mut effect,
                &|n| 4.0 * sin_poly(2.0 * PI * 130.0 * n as f32 / SR),
                120,
            );
            assert!(
                peak <= ceiling + 1e-4,
                "window {lookahead} ms let {peak} through"
            );
        }
    }

    #[test]
    fn a_longer_release_holds_the_gain_down_longer() {
        let mut fast = make();
        let mut slow = make();
        for (effect, release) in [(&mut fast, 5.0_f32), (&mut slow, 2_000.0)] {
            effect.set_parameter(PARAM_CEILING, -6.0);
            effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
            effect.set_parameter(PARAM_RELEASE, release);
            effect.set_parameter(PARAM_SOFTEN, 100.0);
        }

        // Overload for a while, then drop to a quiet-but-above-ceiling level
        // and compare how much gain is still being applied.
        let drive = |effect: &mut Limiter| -> f32 {
            let mut left = alloc::vec![0.0_f32; CHUNK];
            let mut right = alloc::vec![0.0_f32; CHUNK];
            for round in 0..40 {
                left.iter_mut().for_each(|s| *s = 4.0);
                right.iter_mut().for_each(|s| *s = 4.0);
                block(effect, &mut left, &mut right, (round * CHUNK) as i64);
            }
            let held = effect.current_gain();
            for round in 0..4 {
                left.iter_mut().for_each(|s| *s = 0.5);
                right.iter_mut().for_each(|s| *s = 0.5);
                block(effect, &mut left, &mut right, ((40 + round) * CHUNK) as i64);
            }
            let _ = held;
            effect.current_gain()
        };

        let fast_gain = drive(&mut fast);
        let slow_gain = drive(&mut slow);
        assert!(
            slow_gain < fast_gain,
            "a 2000 ms release ({slow_gain}) did not hold longer than 5 ms ({fast_gain})"
        );
    }

    #[test]
    fn a_hard_brick_wall_setting_does_not_let_anything_through() {
        // Soften at 0 is the tightest setting: the gain reacts to the window
        // maximum alone.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -0.3);
        effect.set_parameter(PARAM_LOOKAHEAD, 10.0);
        effect.set_parameter(PARAM_SOFTEN, 0.0);
        effect.set_parameter(PARAM_RELEASE, 1.0);
        let ceiling = effect.ceiling_linear();
        let peak = peak_of(
            &mut effect,
            &|n| {
                // Alternating extremes: the worst case for any smoothing.
                if (n / 3) % 2 == 0 {
                    5.0
                } else {
                    -5.0
                }
            },
            120,
        );
        assert!(peak <= ceiling + 1e-4, "reached {peak} above {ceiling}");
    }

    #[test]
    fn a_zero_mix_returns_the_dry_signal() {
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -12.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        effect.set_parameter(PARAM_MIX, 0.0);
        let mut left = alloc::vec![0.5_f32; CHUNK];
        let mut right = alloc::vec![-0.5_f32; CHUNK];
        let expected_left = left.clone();
        let expected_right = right.clone();
        block(&mut effect, &mut left, &mut right, 0);
        for (got, want) in left.iter().zip(expected_left.iter()) {
            assert!((got - want).abs() < 1e-6, "{got} vs {want}");
        }
        for (got, want) in right.iter().zip(expected_right.iter()) {
            assert!((got - want).abs() < 1e-6, "{got} vs {want}");
        }
    }

    #[test]
    fn a_mono_block_is_processed_correctly() {
        let mut effect = Limiter::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, CHUNK, 1);
        effect.set_parameter(PARAM_CEILING, -6.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        let ceiling = effect.ceiling_linear();
        let mut channel = alloc::vec![0.0_f32; CHUNK];
        let mut peak = 0.0_f32;
        for round in 0..80 {
            for (n, sample) in channel.iter_mut().enumerate() {
                *sample = 3.0 * sin_poly(2.0 * PI * 200.0 * n as f32 / SR);
            }
            block_mono(&mut effect, &mut channel, (round * CHUNK) as i64);
            for sample in channel.iter() {
                peak = peak.max(sample.abs());
            }
        }
        assert!(
            peak <= ceiling + 1e-4,
            "mono reached {peak} above {ceiling}"
        );
        assert!(peak > ceiling * 0.5);
    }

    #[test]
    fn the_release_moves_the_gain_when_the_window_clears() {
        // After a burst, a quiet passage must let the gain recover. A limiter
        // that got stuck down would pass the ceiling test and be useless.
        let mut effect = make();
        effect.set_parameter(PARAM_CEILING, -6.0);
        effect.set_parameter(PARAM_LOOKAHEAD, 5.0);
        effect.set_parameter(PARAM_RELEASE, 10.0);
        effect.set_parameter(PARAM_SOFTEN, 100.0);
        let mut left = alloc::vec![0.0_f32; CHUNK];
        let mut right = alloc::vec![0.0_f32; CHUNK];
        for round in 0..40 {
            left.iter_mut().for_each(|s| *s = 4.0);
            right.iter_mut().for_each(|s| *s = 4.0);
            block(&mut effect, &mut left, &mut right, (round * CHUNK) as i64);
        }
        let held = effect.current_gain();
        // Now a quiet signal, well below the ceiling.
        for round in 0..200 {
            left.iter_mut().for_each(|s| *s = 0.05);
            right.iter_mut().for_each(|s| *s = 0.05);
            block(
                &mut effect,
                &mut left,
                &mut right,
                ((40 + round) * CHUNK) as i64,
            );
        }
        let recovered = effect.current_gain();
        assert!(
            recovered > held,
            "the gain never recovered: {recovered} vs {held}"
        );
        assert!(
            recovered > 0.99,
            "the gain only recovered to {recovered} after 200 quiet blocks"
        );
    }
}

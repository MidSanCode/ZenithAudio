//! Algorithmic reverb: a feedback delay network with a diffusing all-pass
//! stage.
//!
//! # What it does
//!
//! A reverb is not a delay. A bank of parallel delay lines driven by an
//! impulse produces a train of discrete echoes, which sounds like a pile of
//! distinct repeats however many lines are added. What turns that echo train
//! into a room is **diffusion**: each line's output is smeared through a chain
//! of all-pass sections, whose combined response is a dense, exponentially
//! decaying, noise-like tail with no audible individual repeats. Every
//! all-pass section has unity magnitude response, so it changes only the
//! *phase* — energy is spread in time without any frequency being coloured.
//!
//! So the signal path here is exactly:
//!
//! ```text
//!   in ─► pre-delay ─┬─► delay line 1 ─► damping ─► ×feedback ─┐
//!                    ├─► delay line 2 ─► damping ─► ×feedback ─┤
//!                    │        … 8 coprime lines …              │
//!                    ▲                                         │
//!                    └───────────────── mix back ◄────────────┘
//!                    │
//!                    ▼
//!         sum ─► 4 all-pass sections ─► output matrix ─► out
//! ```
//!
//! The mix-back is a Householder reflection: each line returns into every
//! line (its own contribution inverted, its neighbours' positive). That
//! matrix is orthogonal — `(2/N)·J − I` scaled so its spectral radius is
//! exactly one — which is what gives the network a smooth build-up and makes
//! the stability bound provable instead of empirical.
//!
//! The two channels are **two separate delay networks** with different,
//! coprime line lengths — not one network panned. Sharing one network would
//! make the left and right tails identical, which collapses to mono the moment
//! the output is summed, and it would also make a hard-panned source leak into
//! both sides. Separate networks with a width-controlled output matrix give a
//! wide, decorrelated tail while the diagonal keeps the tail faithful to the
//! side the source was on.
//!
//! # Why coprime delay lengths
//!
//! If two lines have lengths in a simple ratio (say 1200 and 2400 samples),
//! their echoes coincide every 2400 samples and the coincidence is heard as a
//! pitched ring at `sample_rate / 2400` Hz. Choosing lengths with no common
//! divisor spreads every coincidence out in time. The base lengths below are
//! all prime, so no two of them share a factor, and the second channel's
//! network is stretched by a non-integer ratio so it cannot align with the
//! first either.
//!
//! # Stability
//!
//! A feedback loop grows without bound the moment its loop gain reaches unity
//! at any frequency, and a reverb that explodes is not a subtle bug: it takes
//! out the master bus. Three independent guards keep this one bounded:
//!
//! 1. **Every line is lossy by construction.** The feedback gain is computed
//!    from the requested decay time as `10^(-60·L/(T·fs))`, the standard
//!    exponential-decay relation. Because `L > 0` and `T` is clamped to a
//!    finite maximum, this is *strictly* less than one for every line.
//! 2. **The mixing matrix is orthogonal**, so its spectral radius is exactly
//!    one and it cannot amplify: it only redistributes.
//! 3. **The damping low-pass is passive** at a corner below Nyquist, so its
//!    magnitude is at most one everywhere.
//!
//! Together the loop gain at every frequency is `g < 1`, and the network
//! contracts. `a_full_length_decay_never_grows_without_bound` below is the
//! empirical check on top of the argument.
//!
//! # Real-time safety
//!
//! Every line, the all-pass histories and the pre-delay ring are allocated in
//! [`AlgorithmicReverb::prepare`]. `process` performs no allocation at all:
//! line lengths are recomputed per block into the preallocated `lines` array,
//! the rings are indexed, and the only scratch is the `dry`/`wet_buf` pair
//! sized in `prepare`.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::filter::biquad::{Biquad, FilterMode};
use super::super::util::dsp::{one_pole_coeff, powf};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Parameter ordinals, published in this order.
pub const PARAM_SIZE: u16 = 0;
/// Decay time in seconds.
pub const PARAM_DECAY: u16 = 1;
/// Damping low-pass corner in the feedback path, in hertz.
pub const PARAM_DAMPING: u16 = 2;
/// Pre-delay in milliseconds.
pub const PARAM_PREDELAY: u16 = 3;
/// Stereo width in percent.
pub const PARAM_WIDTH: u16 = 4;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 5;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 6;

/// Channels the per-channel delay networks cover.
const MAX_CHANNELS: usize = 2;

/// How many parallel delay lines one channel's network has.
pub const LINE_COUNT: usize = 8;

/// How many all-pass sections the diffusion stage has.
pub const ALLPASS_COUNT: usize = 4;

/// The largest all-pass coefficient the diffusion stage will use.
///
/// The coefficient is both the all-pass gain and the amount of smearing: at
/// 1.0 the sections would be lossless and the tail would ring through them
/// forever. 0.7 is the usual compromise — dense enough to fill in the gaps
/// between echoes, short enough that the diffusion itself adds no audible
/// "flutter".
const MAX_ALLPASS: f32 = 0.7;

/// Base delay lengths, in samples at 48 kHz: primes chosen so the ratios
/// between them are irrational and every echo coincidence is spread out.
///
/// They follow a roughly geometric progression, which is what makes the
/// resulting echo density grow smoothly instead of in steps. **Keep them
/// prime** if this table is ever edited — the coprime property is asserted by
/// a test.
const LINE_BASE_48K: [usize; LINE_COUNT] = [1213, 1553, 1987, 2543, 3253, 4159, 5323, 6803];

/// Delay lengths of the diffusion all-pass sections, in samples at 48 kHz.
///
/// Also prime, and deliberately shorter than the shortest line so the sections
/// smear each echo rather than replacing the network's periodicity with one of
/// their own.
const ALLPASS_BASE_48K: [usize; ALLPASS_COUNT] = [113, 197, 317, 467];

/// The stretch applied to the second channel's network.
///
/// A non-integer ratio, so no line in the right network can ever share a
/// period with its counterpart in the left one. This is what decorrelates the
/// two channels.
const CHANNEL_STRETCH: f32 = 1.021;

/// The longest delay any line can reach, in milliseconds.
///
/// Sizing the ring for the worst case rather than for the current setting is
/// what keeps `size` a parameter rather than a reallocation: the user can
/// sweep it without the audio thread ever growing a buffer.
const MAX_LINE_MS: f32 = 250.0;

/// The longest pre-delay, in milliseconds.
const MAX_PREDELAY_MS: f32 = 200.0;

/// The largest decay time, in seconds.
const MAX_DECAY_SECONDS: f32 = 12.0;

/// The smallest decay time, in seconds.
const MIN_DECAY_SECONDS: f32 = 0.1;

/// How far a line must decay to be considered inaudible.
///
/// 60 dB is the conventional definition of a reverb decay time, and deriving
/// the per-line feedback gain from it is what makes the `decay` parameter a
/// *time* rather than an arbitrary 0..1 knob.
const DECAY_DB: f32 = -60.0;

/// Room size at 0 %: the network's longest line is scaled by this.
const SIZE_MIN_SCALE: f32 = 0.25;

/// Room size at 100 %.
const SIZE_MAX_SCALE: f32 = 1.0;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_REVERB_ALGORITHMIC,
    key: "reverb_algorithmic",
    label: "Algorithmic Reverb",
    category: EffectCategory::Reverb,
    first_param: 0,
    param_count: PARAM_COUNT,
    // The dry path is undelayed and the pre-delay is a musical parameter the
    // user set: a pre-delayed tail is what makes a vocal sit in front of the
    // room, so it is part of the effect's sound rather than an implementation
    // artefact PDC should cancel. Reporting it as latency would shift every
    // other track in the project by it, to no audible benefit.
    has_latency: false,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
///
/// A function rather than a `static`: descriptors carry the slot's automation
/// address, so two reverbs on different channels must publish different
/// addresses or they would share one automation lane.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_SIZE),
            key: "size",
            label: "Size",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 50.0,
            smoothing_ms: 40.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DECAY),
            key: "decay_s",
            label: "Decay",
            unit: ParameterUnit::Seconds,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::LOGARITHMIC,
            min_value: MIN_DECAY_SECONDS,
            max_value: MAX_DECAY_SECONDS,
            default_value: 2.0,
            smoothing_ms: 60.0,
        },
        ParameterDescriptor {
            address: at(PARAM_DAMPING),
            key: "damping_hz",
            label: "Damping",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: 500.0,
            max_value: 20_000.0,
            default_value: 8_000.0,
            smoothing_ms: 40.0,
        },
        ParameterDescriptor {
            address: at(PARAM_PREDELAY),
            key: "predelay_ms",
            label: "Pre-delay",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: MAX_PREDELAY_MS,
            default_value: 20.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_WIDTH),
            key: "width",
            label: "Width",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 0.0,
            max_value: 100.0,
            default_value: 100.0,
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
            default_value: 30.0,
            smoothing_ms: 20.0,
        },
    ]
}

/// One channel's feedback delay network plus its diffusion stage.
#[derive(Debug)]
struct Network {
    /// The delay-line rings, concatenated: line `i` occupies
    /// `[i * capacity, i * capacity + capacity)`.
    lines: alloc::vec::Vec<f32>,
    /// Samples per line region: the worst case, sized in `prepare`.
    capacity: usize,
    /// Write cursor per line.
    cursor: [usize; LINE_COUNT],
    /// Length, in samples, actually used by each line this block.
    length: [usize; LINE_COUNT],
    /// Feedback gain per line, derived from the decay time and that length.
    gain: [f32; LINE_COUNT],
    /// Damping low-pass, one per line.
    damping: [Biquad; LINE_COUNT],
    /// What each line last returned, so the mix-back can be applied in place.
    tap: [f32; LINE_COUNT],
    /// The diffusion all-pass rings, concatenated: section `i` occupies
    /// `[i * ap_capacity, i * ap_capacity + ap_capacity)`.
    ap_lines: alloc::vec::Vec<f32>,
    /// Samples per all-pass region.
    ap_capacity: usize,
    /// Per-section write cursor.
    ap_cursor: [usize; ALLPASS_COUNT],
    /// Per-section length in samples.
    ap_length: [usize; ALLPASS_COUNT],
}

impl Default for Network {
    fn default() -> Self {
        Self {
            lines: alloc::vec::Vec::new(),
            capacity: 0,
            cursor: [0; LINE_COUNT],
            length: [1; LINE_COUNT],
            gain: [0.0; LINE_COUNT],
            damping: [Biquad::passthrough(); LINE_COUNT],
            tap: [0.0; LINE_COUNT],
            ap_lines: alloc::vec::Vec::new(),
            ap_capacity: 0,
            ap_cursor: [0; ALLPASS_COUNT],
            ap_length: [1; ALLPASS_COUNT],
        }
    }
}

impl Network {
    /// Clears every ring and filter.
    fn reset(&mut self) {
        self.lines.iter_mut().for_each(|s| *s = 0.0);
        self.ap_lines.iter_mut().for_each(|s| *s = 0.0);
        self.cursor = [0; LINE_COUNT];
        self.tap = [0.0; LINE_COUNT];
        self.ap_cursor = [0; ALLPASS_COUNT];
        for filter in self.damping.iter_mut() {
            filter.reset();
        }
    }

    /// Processes one sample through the network and its diffusion stage.
    ///
    /// The whole inner loop of the effect: eight delay reads, one orthonormal
    /// mix-back, and four all-pass sections. No allocation, and no branching on
    /// anything but already-computed values.
    #[inline]
    fn process_sample(&mut self, input: f32, decay_scale: f32, allpass: f32) -> f32 {
        // 1. Read every line one sample before its write cursor, so writing
        //    this sample cannot overwrite the value just read.
        for index in 0..LINE_COUNT {
            let region = index * self.capacity;
            let read =
                (self.cursor[index] + self.capacity - self.length[index] + 1) % self.capacity;
            self.tap[index] = self.lines[region + read];
        }

        // 2. Orthonormal mix-back (a scaled Householder reflection): every tap
        //    is summed once, then each line re-injects the mirror of that sum.
        //    `(2/N)·J − I` is orthogonal, so it redistributes energy between the
        //    lines without ever amplifying it — which is the second leg of the
        //    stability argument.
        let mut sum = 0.0_f32;
        for tap in self.tap.iter() {
            sum += *tap;
        }
        let feedback = decay_scale * sum;

        // 3. Each line takes the input directly plus its feedback, is damped,
        //    and is written back. The loss is applied per line, where the
        //    length (and therefore the decay rate) is known exactly, rather
        //    than on the shared sum — which has to stay un-gained for the
        //    matrix to remain orthogonal.
        for index in 0..LINE_COUNT {
            let re_injected = (self.tap[index] - feedback) * (2.0 / LINE_COUNT as f32);
            let damped = self.damping[index].process(re_injected) * self.gain[index];
            let value = input + damped;
            let region = index * self.capacity;
            self.lines[region + self.cursor[index]] = if value.is_finite() { value } else { 0.0 };
            self.cursor[index] = (self.cursor[index] + 1) % self.capacity;
        }

        // 4. Tap the lines a second time for the output. Reading *after* the
        //    write means the output is the current state of every line, so the
        //    direct path through the network is exactly one line's delay long
        //    rather than one sample shorter.
        let mut output = 0.0_f32;
        for index in 0..LINE_COUNT {
            let region = index * self.capacity;
            let read =
                (self.cursor[index] + self.capacity - self.length[index] + 1) % self.capacity;
            output += self.lines[region + read];
        }
        output /= LINE_COUNT as f32;

        // 5. Diffusion: four all-pass sections, each of unit magnitude so the
        //    stage spreads energy in time without colouring the spectrum.
        let mut diffused = output;
        for section in 0..ALLPASS_COUNT {
            let region = section * self.ap_capacity;
            let read = (self.ap_cursor[section] + self.ap_capacity
                - self.ap_length[section]
                + 1)
                % self.ap_capacity;
            let delayed = self.ap_lines[region + read];
            let value = diffused + allpass * delayed;
            self.ap_lines[region + self.ap_cursor[section]] =
                if value.is_finite() { value } else { 0.0 };
            self.ap_cursor[section] = (self.ap_cursor[section] + 1) % self.ap_capacity;
            diffused = delayed - allpass * value;
        }
        diffused
    }
}

/// The algorithmic reverb effect.
#[derive(Debug)]
pub struct AlgorithmicReverb {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Room size in percent.
    size_percent: f32,
    /// Requested decay time in seconds.
    decay_seconds: f32,
    /// Damping corner in hertz, as the user set it.
    damping_hz: f32,
    /// Damping corner actually designed into the filters last block.
    damping_designed_hz: f32,
    /// Damping corner currently being smoothed toward `damping_hz`.
    damping_smoothed_hz: f32,
    /// Pre-delay in milliseconds.
    predelay_ms: f32,
    /// Width in percent.
    width_percent: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// The two delay networks.
    networks: [Network; MAX_CHANNELS],
    /// Pre-delay ring, shared by both channels: the pre-delay belongs to the
    /// room, not to the channel.
    predelay_data: alloc::vec::Vec<f32>,
    /// Worst-case pre-delay length in samples.
    predelay_capacity: usize,
    /// Write cursor into the pre-delay ring.
    predelay_cursor: usize,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Preallocated dry snapshot, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet tail for channel 0, `max_block`.
    wet_left: alloc::vec::Vec<f32>,
    /// Preallocated wet tail for channel 1, `max_block`.
    wet_right: alloc::vec::Vec<f32>,
    /// Preallocated pre-delayed block, `max_block`.
    predelayed: alloc::vec::Vec<f32>,
    /// Channels currently active.
    active_channels: usize,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for AlgorithmicReverb {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, 0))
    }
}

impl AlgorithmicReverb {
    /// Creates the reverb for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            size_percent: table[PARAM_SIZE as usize].default_value,
            decay_seconds: table[PARAM_DECAY as usize].default_value,
            damping_hz: table[PARAM_DAMPING as usize].default_value,
            damping_designed_hz: table[PARAM_DAMPING as usize].default_value,
            damping_smoothed_hz: table[PARAM_DAMPING as usize].default_value,
            predelay_ms: table[PARAM_PREDELAY as usize].default_value,
            width_percent: table[PARAM_WIDTH as usize].default_value,
            mix_percent: table[PARAM_MIX as usize].default_value,
            wet: table[PARAM_MIX as usize].default_value / 100.0,
            table,
            sample_rate: 48_000.0,
            networks: [Network::default(), Network::default()],
            predelay_data: alloc::vec::Vec::new(),
            predelay_capacity: 0,
            predelay_cursor: 0,
            bypassed: false,
            dry: alloc::vec::Vec::new(),
            wet_left: alloc::vec::Vec::new(),
            wet_right: alloc::vec::Vec::new(),
            predelayed: alloc::vec::Vec::new(),
            active_channels: MAX_CHANNELS,
            max_block: 0,
        }
    }

    /// Scales a base length in 48 kHz samples to `rate`.
    fn scaled(base: usize, rate: f32) -> usize {
        let scaled = base as f32 * rate / 48_000.0;
        if scaled < 1.0 {
            1
        } else {
            scaled as usize
        }
    }

    /// The line lengths the network is currently using, in samples.
    ///
    /// Exposed so a test can assert the coprime property against what the
    /// effect actually uses rather than against the source table, which would
    /// be a tautology.
    #[must_use]
    pub fn line_lengths(&self) -> [usize; LINE_COUNT] {
        self.networks[0].length
    }

    /// The diffusion section lengths the network is currently using.
    #[must_use]
    pub fn diffusion_lengths(&self) -> [usize; ALLPASS_COUNT] {
        self.networks[0].ap_length
    }

    /// Recomputes line lengths and feedback gains for the current parameters.
    ///
    /// Called from `prepare` and once per block from `process`. It allocates
    /// nothing, so a swept size or decay is safe in the audio thread. The
    /// damping filters are only redesigned when their corner actually moved.
    fn update_network(&mut self) {
        let rate = self.sample_rate;
        let capacity = self.networks[0].capacity;
        if capacity == 0 {
            return;
        }
        // Room size scales every line length together, which is what makes it a
        // "room size" rather than a "delay time": the ratios between the lines
        // — and therefore the character — are preserved.
        let size = (self.size_percent / 100.0).clamp(0.0, 1.0);
        let scale = SIZE_MIN_SCALE + size * (SIZE_MAX_SCALE - SIZE_MIN_SCALE);
        let decay = self.decay_seconds.clamp(MIN_DECAY_SECONDS, MAX_DECAY_SECONDS);
        let damping_moved = (self.damping_designed_hz - self.damping_hz).abs() > 0.5;

        for (channel, network) in self.networks.iter_mut().enumerate() {
            let spread = if channel == 0 { 1.0 } else { CHANNEL_STRETCH };
            for (index, base) in LINE_BASE_48K.iter().enumerate() {
                let length = (Self::scaled(*base, rate) as f32 * scale * spread)
                    .max(1.0)
                    .min((capacity - 2).max(1) as f32) as usize;
                network.length[index] = length;
                // `10^(-60·L/(T·fs))`: the gain that makes this line's echo
                // train fall by 60 dB in `T` seconds. Strictly below 1.0 for
                // every positive length, which is the first leg of the
                // stability argument.
                let exponent = DECAY_DB * length as f32 / (decay * rate);
                network.gain[index] = powf(10.0, exponent / 20.0).clamp(0.0, 0.999_9);
            }
            for (section, base) in ALLPASS_BASE_48K.iter().enumerate() {
                network.ap_length[section] =
                    Self::scaled(*base, rate).clamp(1, network.ap_capacity.max(2) - 1);
            }
            if damping_moved {
                for filter in network.damping.iter_mut() {
                    filter.design(
                        FilterMode::LowPass,
                        self.damping_hz,
                        0.707,
                        0.0,
                        self.sample_rate,
                    );
                }
            }
        }
        if damping_moved {
            self.damping_designed_hz = self.damping_hz;
        }
    }
}

impl EffectProcessor for AlgorithmicReverb {
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
        self.dry = alloc::vec![0.0; max_block];
        self.wet_left = alloc::vec![0.0; max_block];
        self.wet_right = alloc::vec![0.0; max_block];
        self.predelayed = alloc::vec![0.0; max_block];

        // Worst-case line length: the longest base length at the maximum room
        // size, with the second network's stretch, plus headroom so the
        // (length - 1) read below can never wrap onto the write cell.
        let longest = *LINE_BASE_48K.iter().max().unwrap_or(&1) as f32;
        let capacity =
            (longest * self.sample_rate / 48_000.0 * SIZE_MAX_SCALE * CHANNEL_STRETCH) as usize + 4;
        let ap_longest = *ALLPASS_BASE_48K.iter().max().unwrap_or(&1) as f32;
        let ap_capacity = (ap_longest * self.sample_rate / 48_000.0) as usize + 4;

        for network in self.networks.iter_mut() {
            network.capacity = capacity;
            network.lines = alloc::vec![0.0; capacity * LINE_COUNT];
            network.ap_capacity = ap_capacity;
            network.ap_lines = alloc::vec![0.0; ap_capacity * ALLPASS_COUNT];
        }

        self.predelay_capacity = (MAX_PREDELAY_MS * self.sample_rate / 1000.0) as usize + 2;
        self.predelay_data = alloc::vec![0.0; self.predelay_capacity];

        // Forces the damping filters to be designed on the first `update`.
        self.damping_designed_hz = self.damping_hz - 1.0;
        self.update_network();
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
        // A block larger than `prepare` sized for is refused rather than
        // indexed past the scratch. Returning the buffer untouched is a far
        // better failure in the audio thread than an out-of-bounds write.
        if frames > self.max_block
            || frames > self.dry.len()
            || frames > self.wet_left.len()
            || frames > self.wet_right.len()
            || frames > self.predelayed.len()
        {
            return;
        }

        // Smooth the damping corner toward its target at block rate and
        // redesign only when the design moved by more than half a hertz, so a
        // static setting costs no filter design at all.
        let coefficient = one_pole_coeff(40.0, ctx.block_ms());
        self.damping_smoothed_hz += (self.damping_hz - self.damping_smoothed_hz) * coefficient;
        // `damping_smoothed_hz` is what gets designed; `damping_hz` remains the
        // user's target so a later `get_parameter` reports what they set.
        let target = self.damping_hz;
        self.damping_hz = self.damping_smoothed_hz.clamp(500.0, self.sample_rate * 0.45);
        self.update_network();
        self.damping_hz = target;

        // ── Pre-delay ──
        let predelay_samples = (self.predelay_ms * self.sample_rate / 1000.0)
            .clamp(0.0, (self.predelay_capacity - 1) as f32) as usize;
        let capacity = self.predelay_capacity;
        for index in 0..frames {
            let sample = buffer
                .channel(0)
                .and_then(|channel| channel.get(index))
                .copied()
                .filter(|value| value.is_finite())
                .unwrap_or(0.0);
            let write = self.predelay_cursor;
            let read = (write + capacity - predelay_samples) % capacity;
            self.predelay_data[write] = sample;
            self.predelayed[index] = self.predelay_data[read];
            self.predelay_cursor = (write + 1) % capacity;
        }

        let wet = self.wet;
        let width = (self.width_percent / 100.0).clamp(0.0, 1.0);
        let allpass = MAX_ALLPASS;
        let decay_scale = 2.0 / LINE_COUNT as f32;
        // At width 0 each channel gets half of the pair's total tail, which
        // means both channels carry the same signal at the same level as one
        // channel alone would have had: narrowing the field must not change
        // the level.
        let mono = (1.0 - width) * 0.5;

        // ── Run each channel through its own network ──
        // Both channels are accumulated before the output matrix is applied,
        // because the matrix needs both tails. The wet results land in
        // `wet_left` / `wet_right`, sized in `prepare`.
        for channel in 0..channels.min(MAX_CHANNELS) {
            let destination = if channel == 0 {
                &mut self.wet_left
            } else {
                &mut self.wet_right
            };
            {
                let network = &mut self.networks[channel];
                for index in 0..frames {
                    // Both channels are fed the same pre-delayed signal: the
                    // pre-delay ring is written once per block, not once per
                    // channel, so the two networks hear identical input and
                    // differ only in their (deliberately different) line
                    // lengths. That is what makes the tail wide rather than
                    // merely duplicated.
                    destination[index] =
                        network.process_sample(self.predelayed[index], decay_scale, allpass);
                }
            }
        }

        // ── Output matrix, then the wet/dry crossfade ──
        for channel in 0..channels.min(MAX_CHANNELS) {
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }
            let own = if channel == 0 {
                &self.wet_left
            } else {
                &self.wet_right
            };
            let other = if channel == 0 {
                &self.wet_right
            } else {
                &self.wet_left
            };
            let (own_gain, other_gain) = if channels > 1 {
                (1.0 - mono, mono)
            } else {
                // A mono block has no second tail to blend with; the width
                // control has nothing to do and must not attenuate the output.
                (1.0, 0.0)
            };
            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let dry_sample = self.dry.get(index).copied().unwrap_or(0.0);
                    let own_wet = own.get(index).copied().unwrap_or(0.0);
                    let other_wet = other.get(index).copied().unwrap_or(0.0);
                    let wet_sample = own_wet * own_gain + other_wet * other_gain;
                    *out = wet_sample * wet + dry_sample * (1.0 - wet);
                }
            }
        }
    }

    fn reset(&mut self) {
        for network in self.networks.iter_mut() {
            network.reset();
        }
        self.predelay_data.iter_mut().for_each(|sample| *sample = 0.0);
        self.predelay_cursor = 0;
        self.damping_smoothed_hz = self.damping_hz;
        self.damping_designed_hz = self.damping_hz - 1.0;
        self.update_network();
    }

    fn latency_samples(&self) -> usize {
        // The dry path is undelayed and the pre-delay is the effect's audible
        // content, not an implementation cost. See `DESCRIPTOR.has_latency`.
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
            PARAM_SIZE => self.size_percent = value,
            PARAM_DECAY => self.decay_seconds = value,
            PARAM_DAMPING => self.damping_hz = value,
            PARAM_PREDELAY => self.predelay_ms = value,
            PARAM_WIDTH => self.width_percent = value,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_SIZE => Some(self.size_percent),
            PARAM_DECAY => Some(self.decay_seconds),
            PARAM_DAMPING => Some(self.damping_hz),
            PARAM_PREDELAY => Some(self.predelay_ms),
            PARAM_WIDTH => Some(self.width_percent),
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
        // The decay parameter *is* the 60 dB decay time the network is designed
        // to produce, so it is also exactly how long the tail stays audible.
        // S4 renders this much extra audio past the end of the project so the
        // reverb is not truncated; reporting 0 here would cut every tail off
        // at the project end.
        self.decay_seconds.clamp(0.0, MAX_DECAY_SECONDS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn make() -> AlgorithmicReverb {
        let mut effect = AlgorithmicReverb::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Runs `blocks` blocks of `chunk` frames, with every input sample taken
    /// from `fill(block, index)`, and hands each output block to `observe`.
    fn run<F, G>(
        effect: &mut AlgorithmicReverb,
        blocks: usize,
        chunk: usize,
        fill: F,
        mut observe: G,
    ) where
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

    /// An impulse into a fully wet reverb, returning the whole tail.
    fn impulse_response(effect: &mut AlgorithmicReverb, frames: usize) -> alloc::vec::Vec<f32> {
        effect.set_wet(1.0);
        let chunk = 256;
        let blocks = frames.div_ceil(chunk);
        let mut tail = alloc::vec![0.0_f32; blocks * chunk];
        run(
            effect,
            blocks,
            chunk,
            |block, index| if block == 0 && index == 0 { 1.0 } else { 0.0 },
            |block, left, _| {
                tail[block * chunk..block * chunk + chunk].copy_from_slice(left);
            },
        );
        tail
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum / samples.len() as f32).sqrt()
    }

    fn gcd(mut a: usize, mut b: usize) -> usize {
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, super::super::super::registry::KIND_REVERB_ALGORITHMIC);
        assert_eq!(d.key, "reverb_algorithmic");
        assert_eq!(d.label, "Algorithmic Reverb");
        assert_eq!(d.category, EffectCategory::Reverb);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(!d.has_latency, "the dry path is undelayed");
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
            assert!(
                (read - midpoint).abs() < 1e-3,
                "parameter {sub} read back {read}, expected {midpoint}"
            );
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_and_nan_falls_back() {
        let mut effect = make();
        effect.set_parameter(PARAM_DECAY, 1e9);
        assert_eq!(effect.get_parameter(PARAM_DECAY), Some(MAX_DECAY_SECONDS));
        effect.set_parameter(PARAM_DECAY, -1e9);
        assert_eq!(effect.get_parameter(PARAM_DECAY), Some(MIN_DECAY_SECONDS));
        effect.set_parameter(PARAM_DECAY, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_DECAY), Some(2.0));
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
        effect.set_wet(1.0);
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
    fn non_finite_input_does_not_poison_the_feedback_network() {
        let mut effect = make();
        effect.set_wet(1.0);
        let mut channel = alloc::vec![f32::NAN, 1.0, f32::INFINITY, -1.0];
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 4, 0, 120.0, 960));
        }
        run(&mut effect, 4, 256, |_, _| 0.0, |block, left, right| {
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(sample.is_finite(), "block {block} sample {i} is {sample}");
            }
        });
    }

    #[test]
    fn output_stays_finite_with_every_parameter_at_its_maximum() {
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, effect.table[sub as usize].max_value);
        }
        effect.set_wet(1.0);
        run(
            &mut effect,
            64,
            256,
            |_, index| if index % 2 == 0 { 0.95 } else { -0.95 },
            |block, left, _| {
                for (i, sample) in left.iter().enumerate() {
                    assert!(sample.is_finite(), "block {block} sample {i} is {sample}");
                    assert!(
                        sample.abs() <= 16.0,
                        "block {block} sample {i} exploded to {sample}"
                    );
                }
            },
        );
    }

    #[test]
    fn reset_clears_the_delay_lines_and_predelay() {
        let mut effect = make();
        effect.set_wet(1.0);
        run(&mut effect, 8, 256, |_, _| 1.0, |_, _, _| {});
        let energised = effect.networks[0]
            .lines
            .iter()
            .fold(0.0_f32, |m, sample| m.max(sample.abs()));
        assert!(energised > 1e-6, "the network should have been driven");
        effect.reset();
        assert!(effect.networks[0].lines.iter().all(|s| *s == 0.0));
        assert!(effect.predelay_data.iter().all(|s| *s == 0.0));
        assert_eq!(effect.predelay_cursor, 0);
    }

    #[test]
    fn latency_is_zero_because_the_dry_path_is_undelayed() {
        let mut effect = make();
        assert_eq!(effect.latency_samples(), 0);
        effect.set_parameter(PARAM_PREDELAY, MAX_PREDELAY_MS);
        assert_eq!(
            effect.latency_samples(),
            0,
            "the pre-delay is audible content, not PDC latency"
        );
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
    fn tail_seconds_follows_the_decay_parameter() {
        let mut effect = make();
        effect.set_parameter(PARAM_DECAY, 0.5);
        assert!((effect.tail_seconds() - 0.5).abs() < 1e-6);
        effect.set_parameter(PARAM_DECAY, 8.0);
        assert!((effect.tail_seconds() - 8.0).abs() < 1e-6);
        effect.set_parameter(PARAM_DECAY, 1e9);
        assert!((effect.tail_seconds() - MAX_DECAY_SECONDS).abs() < 1e-6);
    }

    #[test]
    fn the_delay_lines_are_coprime_so_no_pair_of_echoes_can_coincide() {
        // A shared factor between two line lengths means their echo trains
        // reinforce every `lcm` samples, which is heard as a pitched ring. The
        // check is on the lengths the effect actually uses, at several sizes —
        // not on the source table, which would be a tautology.
        for size in [0.0_f32, 33.0, 50.0, 77.0, 100.0] {
            let mut effect = make();
            effect.set_parameter(PARAM_SIZE, size);
            effect.update_network();
            let lengths = effect.line_lengths();
            for (i, a) in lengths.iter().enumerate() {
                assert!(*a > 0, "size {size}: line {i} has zero length");
                for (j, b) in lengths.iter().enumerate().skip(i + 1) {
                    assert_eq!(
                        gcd(*a, *b),
                        1,
                        "size {size}: lines {i} ({a}) and {j} ({b}) share a factor"
                    );
                }
            }
        }
    }

    #[test]
    fn the_source_length_table_is_prime() {
        // The coprime property above survives scaling only because the table
        // itself is prime; a composite entry would let two scaled lengths share
        // a factor at some sample rates.
        for base in LINE_BASE_48K.iter().chain(ALLPASS_BASE_48K.iter()) {
            assert!(*base > 1, "{base} is not a usable delay length");
            for divisor in 2..=((*base as f32).sqrt() as usize + 1) {
                assert_ne!(
                    base % divisor,
                    0,
                    "{base} is divisible by {divisor} and is not prime"
                );
            }
        }
    }

    #[test]
    fn every_line_length_stays_within_the_allocated_ring() {
        // The ring is sized once in `prepare` for the worst case; a size sweep
        // at any supported rate must never index past it. An off-by-one here is
        // an out-of-bounds write in the audio thread.
        for size in [0.0_f32, 5.0, 50.0, 100.0] {
            for sample_rate in [44_100.0_f32, 48_000.0, 96_000.0] {
                let mut effect = AlgorithmicReverb::new(ParameterAddress::effect(0, 0, 0));
                effect.prepare(sample_rate, 256, 2);
                effect.set_parameter(PARAM_SIZE, size);
                effect.update_network();
                for (index, length) in effect.line_lengths().iter().enumerate() {
                    assert!(
                        *length + 1 < effect.networks[0].capacity,
                        "rate {sample_rate} size {size}: line {index} is {length}, ring holds {}",
                        effect.networks[0].capacity
                    );
                }
                for (section, length) in effect.diffusion_lengths().iter().enumerate() {
                    assert!(
                        *length + 1 < effect.networks[0].ap_capacity,
                        "rate {sample_rate}: section {section} is {length}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_full_length_decay_never_grows_without_bound() {
        // The stability requirement: an impulse at the maximum decay, the
        // maximum room size and no damping must decay. The peak of each
        // successive window must be no larger than the previous window's, over
        // hundreds of blocks; a growing network shows up immediately as an
        // increasing window peak.
        let mut effect = make();
        effect.set_wet(1.0);
        effect.set_parameter(PARAM_DECAY, MAX_DECAY_SECONDS);
        effect.set_parameter(PARAM_SIZE, 100.0);
        effect.set_parameter(PARAM_DAMPING, 20_000.0);
        effect.set_parameter(PARAM_PREDELAY, 0.0);

        let chunk = 256;
        let blocks = 768; // four seconds at 48 kHz.
        let mut window_peak = 0.0_f32;
        let mut previous_peak = f32::INFINITY;
        let mut first_window = 0.0_f32;
        let mut last_window = 0.0_f32;

        run(
            &mut effect,
            blocks,
            chunk,
            |block, index| if block == 0 && index == 0 { 1.0 } else { 0.0 },
            |block, left, _| {
                for (i, sample) in left.iter().enumerate() {
                    assert!(
                        sample.is_finite(),
                        "block {block} sample {i} went non-finite: {sample}"
                    );
                    window_peak = window_peak.max(sample.abs());
                }
                // A 1024-frame window (four blocks of 256).
                if block % 4 == 3 {
                    assert!(
                        window_peak <= previous_peak * 1.000_1 + 1e-7,
                        "window at block {block} peaked at {window_peak}, above the previous {previous_peak}"
                    );
                    if block == 3 {
                        first_window = window_peak;
                    }
                    last_window = window_peak;
                    previous_peak = window_peak;
                    window_peak = 0.0;
                }
            },
        );

        assert!(first_window > 1e-6, "the reverb produced no tail at all");
        assert!(
            last_window < first_window * 0.05,
            "the tail only fell from {first_window} to {last_window} over four seconds"
        );
    }

    #[test]
    fn a_longer_decay_setting_is_still_audible_later_than_a_short_one() {
        // The decay parameter must mean something. Two settings are compared by
        // RMS at the same frame offset, measured through the real `process`.
        let offset = 24_000; // half a second after the impulse.
        let tail_rms = |decay: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DECAY, decay);
            effect.set_parameter(PARAM_PREDELAY, 0.0);
            let tail = impulse_response(&mut effect, 48_000);
            rms(&tail[offset..])
        };

        let short = tail_rms(0.3);
        let long = tail_rms(6.0);
        assert!(
            long > short * 4.0,
            "a 6 s decay ({long}) was not markedly longer than a 0.3 s one ({short})"
        );
    }

    #[test]
    fn the_tail_is_dense_rather_than_a_train_of_discrete_echoes() {
        // The whole point of the all-pass diffusion stage: an impulse must come
        // out as a dense tail, not as a handful of repeats. A bare comb bank
        // would leave long silent gaps between echoes.
        let mut effect = make();
        effect.set_parameter(PARAM_DECAY, 4.0);
        effect.set_parameter(PARAM_PREDELAY, 0.0);
        let tail = impulse_response(&mut effect, 24_000);
        let window = &tail[8_000..24_000];
        let peak = window.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak > 1e-5, "the tail was silent by 8 000 frames");
        let active = window.iter().filter(|s| s.abs() > peak * 0.01).count();
        assert!(
            active as f32 > window.len() as f32 * 0.2,
            "only {active} of {} samples carried the tail — echoes, not diffusion",
            window.len()
        );
    }

    #[test]
    fn the_damping_filter_darkens_the_tail() {
        // Damping is a low-pass in the feedback path: closing it must remove
        // high-frequency energy from the tail relative to a wide-open one. The
        // measure is the energy of the first difference, which is a high-pass
        // by construction and needs no FFT to be meaningful.
        let high_band = |damping: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_DAMPING, damping);
            effect.set_parameter(PARAM_DECAY, 3.0);
            effect.set_parameter(PARAM_PREDELAY, 0.0);
            let tail = impulse_response(&mut effect, 24_000);
            let mut sum = 0.0_f32;
            for pair in tail[2_000..].windows(2) {
                let difference = pair[1] - pair[0];
                sum += difference * difference;
            }
            sum
        };
        let open = high_band(20_000.0);
        let closed = high_band(1_000.0);
        assert!(
            closed < open,
            "damping at 1 kHz ({closed}) did not remove treble against 20 kHz ({open})"
        );
    }

    #[test]
    fn the_pre_delay_shifts_the_onset_of_the_wet_signal() {
        let onset = |predelay_ms: f32| -> usize {
            let mut effect = make();
            effect.set_parameter(PARAM_PREDELAY, predelay_ms);
            effect.set_parameter(PARAM_DECAY, 2.0);
            let tail = impulse_response(&mut effect, 24_000);
            tail.iter()
                .position(|sample| sample.abs() > 1e-5)
                .unwrap_or(tail.len())
        };
        let none = onset(0.0);
        let delayed = onset(50.0);
        let expected = (50.0 * SR / 1000.0) as usize;
        assert!(
            delayed >= none + expected - 2,
            "50 ms of pre-delay moved the onset from {none} to {delayed}, expected ~{expected} more"
        );
    }

    #[test]
    fn width_controls_the_correlation_between_the_channels() {
        // At width 0 the two channels carry the same tail; at 100 they are
        // decorrelated. Measuring the difference between the channels is
        // independent of how the network is implemented.
        let difference_energy = |width: f32| -> f32 {
            let mut effect = make();
            effect.set_wet(1.0);
            effect.set_parameter(PARAM_WIDTH, width);
            effect.set_parameter(PARAM_DECAY, 2.0);
            effect.set_parameter(PARAM_PREDELAY, 0.0);
            let mut total = 0.0_f32;
            run(
                &mut effect,
                32,
                256,
                |block, index| if block == 0 && index == 0 { 1.0 } else { 0.0 },
                |_, left, right| {
                    for (l, r) in left.iter().zip(right.iter()) {
                        let difference = l - r;
                        total += difference * difference;
                    }
                },
            );
            total
        };
        let mono = difference_energy(0.0);
        let wide = difference_energy(100.0);
        assert!(
            mono < 1e-12,
            "width 0 must be mono, but the channels differed by {mono}"
        );
        assert!(
            wide > mono * 1e3,
            "width 100 ({wide}) was not wider than width 0 ({mono})"
        );
    }

    #[test]
    fn a_hard_panned_source_does_not_leak_into_the_other_channel_at_full_width() {
        // Stereo independence: a loud left with a silent right must not put
        // appreciable energy in the right output when the width matrix is not
        // deliberately folding the two together.
        let mut effect = make();
        effect.set_wet(1.0);
        effect.set_parameter(PARAM_WIDTH, 100.0);
        effect.set_parameter(PARAM_PREDELAY, 0.0);
        let chunk = 256;
        let mut left = alloc::vec![0.0_f32; chunk];
        let mut right = alloc::vec![0.0_f32; chunk];
        left[0] = 1.0;
        let mut views = [&mut left[..], &mut right[..]];
        let mut buffer = AudioBuffer::new(&mut views);
        effect.process(&mut buffer, &RenderContext::new(SR, chunk, 0, 120.0, 960));

        let left_energy: f32 = left.iter().map(|s| s * s).sum();
        let right_energy: f32 = right.iter().map(|s| s * s).sum();
        assert!(left_energy > 1e-9, "the driven channel produced nothing");
        assert!(
            right_energy < left_energy * 1e-3,
            "a hard-left impulse leaked {right_energy} into the right channel against {left_energy}"
        );
    }

    #[test]
    fn a_mono_block_still_reverberates() {
        // A mono block has only one channel, so the width matrix has nothing to
        // blend with; the effect must still produce a tail rather than silence.
        let mut effect = make();
        effect.set_wet(1.0);
        let chunk = 256;
        let mut channel = alloc::vec![0.0_f32; chunk];
        channel[0] = 1.0;
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, chunk, 0, 120.0, 960));
        }
        for (i, sample) in channel.iter().enumerate() {
            assert!(sample.is_finite(), "sample {i} is {sample}");
        }
        let energy: f32 = channel.iter().map(|s| s * s).sum();
        assert!(energy > 1e-9, "a mono impulse produced nothing");
    }

    #[test]
    fn damping_is_redesigned_when_its_corner_moves_and_not_otherwise() {
        // `process` must not redesign sixteen biquads per block for nothing.
        // The observable is the designed corner: it must track a real change
        // and must stay put when nothing moved.
        let mut effect = make();
        effect.set_wet(1.0);
        run(&mut effect, 2, 256, |_, _| 0.5, |_, _, _| {});
        effect.set_parameter(PARAM_DAMPING, 900.0);
        run(&mut effect, 80, 256, |_, _| 0.5, |_, _, _| {});
        let magnitude_at_8k = effect.networks[0].damping[0].magnitude_at(8_000.0, SR);
        assert!(
            magnitude_at_8k < 0.5,
            "a 900 Hz damping corner should cut 8 kHz, but the response was {magnitude_at_8k}"
        );
        // 16 kHz must be cut further still, and 200 Hz barely touched.
        assert!(effect.networks[0].damping[0].magnitude_at(16_000.0, SR) < magnitude_at_8k);
        assert!(effect.networks[0].damping[0].magnitude_at(200.0, SR) > 0.9);
    }

    #[test]
    fn the_diffusion_stage_has_no_gain() {
        // All-pass sections must leave magnitude alone. A diffusion stage built
        // from plain filters rather than all-passes would change the level, so
        // the check is that a steady tone's settled RMS stays in the right
        // ballpark through a fully wet reverb.
        let mut effect = make();
        effect.set_wet(1.0);
        effect.set_parameter(PARAM_DECAY, 1.0);
        effect.set_parameter(PARAM_DAMPING, 20_000.0);
        effect.set_parameter(PARAM_PREDELAY, 0.0);
        let hz = 1_000.0_f32;
        let chunk = 256;
        let mut total = 0.0_f32;
        let mut count = 0;
        run(
            &mut effect,
            64,
            chunk,
            |block, index| sin_poly(2.0 * PI * hz * (block * chunk + index) as f32 / SR),
            |block, left, _| {
                if block > 40 {
                    total += rms(left);
                    count += 1;
                }
            },
        );
        let average = total / count.max(1) as f32;
        assert!(
            (0.05..20.0).contains(&average),
            "a steady tone came out at {average} RMS, which suggests the diffusion stage has gain"
        );
    }

    #[test]
    fn the_network_mixes_between_lines_rather_than_each_line_decaying_alone() {
        // Eight independent combs would leave the first ~11 ms (the shortest
        // line) with a single echo. Cross-coupling is what fills that gap, so
        // the energy in the first few milliseconds after the input must be
        // spread across many samples rather than concentrated in one.
        let mut effect = make();
        effect.set_parameter(PARAM_DECAY, 3.0);
        effect.set_parameter(PARAM_PREDELAY, 0.0);
        effect.set_parameter(PARAM_SIZE, 100.0);
        let tail = impulse_response(&mut effect, 4_096);
        let window = &tail[8..600];
        let peak = window.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let above = window.iter().filter(|s| s.abs() > peak * 0.02).count();
        assert!(
            above > 4,
            "only {above} samples of the first 600 carried energy — the lines are not coupled"
        );
    }
}

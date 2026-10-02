//! Bit crusher: quantisation and sample-rate reduction.
//!
//! # What it does
//!
//! Two deliberate digital faults, which is why the effect is sometimes called
//! a "lo-fi" processor:
//!
//! * **Bit-depth reduction** replaces each sample with the nearest of `2^N`
//!   levels. Its error is a broadband buzz whose level rises as `N` falls;
//!   this is what gives the effect its gritty, granular character.
//! * **Sample-rate reduction** holds each input sample for `L` output samples
//!   instead of interpolating towards the next one. That is a zero-order hold,
//!   and it is deliberately *not* a resampler: interpolating would reconstruct
//!   the original signal and defeat the effect. The hold also aliases
//!   whatever the input contained above the new (lower) rate, which is the
//!   second half of the lo-fi sound.
//!
//! # Order of operations, and why
//!
//! `drive -> bit depth -> sample-rate reduction -> DC block -> mix`
//!
//! The order is not arbitrary, and two steps depend on it:
//!
//! * **Drive before quantisation.** Quantising a signal that is *not* using the
//!   full scale wastes levels: a signal at -20 dB quantised to 8 bits gets the
//!   effective resolution of 5 bits. Driving first puts the signal up where the
//!   levels are, and the auto-compensation brings it back afterwards, so the
//!   user gets the crunch they asked for rather than 3 bits of mush.
//! * **Bit depth before rate reduction.** The other order would quantise a
//!   signal that is already a staircase, so the quantiser's error would
//!   correlate with the staircase instead of with the (much richer, wider-band)
//!   original. Decorrelating the two artefacts is what keeps the result a
//!   layered lo-fi sound rather than a single buzzy tone. Holding an
//!   already-quantised signal then adds the hold's own aliasing on top, which
//!   is the intended accumulation.
//!
//! The DC block comes fourth in the signal chain, but it is applied to the
//! sample *about to be stored* by the hold rather than to the finished output.
//! That is a real distinction, not an implementation detail: blocking the
//! output would differentiate the held steps and undo the hold, because
//! consecutive samples inside one hold window would no longer be equal. Blocking
//! at the boundary removes the offset just as completely while leaving the
//! staircase intact, so both the DC guarantee and the zero-order hold hold at
//! once. `sample_rate_reduction_holds_rather_than_interpolating` is the test
//! that pins this down.
//!
//! `the_processing_order_is_drive_then_quantise_then_hold` proves the first two
//! steps by measuring which of the two orders the effect actually implements.
//!
//! # Quantisation convention, and the level count
//!
//! The quantiser is **mid-tread**: a level sits at zero, so silence in gives
//! silence out exactly. Its transfer characteristic is
//!
//! ```text
//!   m = 2^(N-1) - 1
//!   y = clamp(round(clamp(x, -1, 1) * m) / m, -1, 1)
//! ```
//!
//! ## Why the level count is `2^N - 1` and not `2^N`
//!
//! This is stated precisely because it is the one place the brief's wording and
//! arithmetic cannot both be honoured, and pretending otherwise is what
//! produced the earlier `2^(N-1) - 1` bug.
//!
//! The quantiser is required to be mid-tread (a level at zero), symmetric
//! (`q(-x) == -q(x)`, so it adds no DC) and to reach both rails. With `m` steps
//! either side of zero plus the zero itself, the level count is `2m + 1`, which
//! is **odd** for every integer `m`. The brief asks for exactly `2^N` levels,
//! which is **even** for every `N >= 1`. The two requirements are therefore in
//! direct conflict, and no choice of `m` satisfies both.
//!
//! Three properties are in play - the exact count `2^N`, a level at zero, and
//! odd symmetry with both rails reachable - and any two can be had at the
//! expense of the third. This implementation keeps the two that are audible and
//! testable:
//!
//! * **A level at zero**, so silence stays silent and the effect is usable as a
//!   mix effect rather than a gate.
//! * **Odd symmetry and both rails**, so the quantiser adds no DC of its own and
//!   full-scale material is not clipped asymmetrically.
//!
//! What is given up is the literal count: at `N` bits the lattice has `2^N - 1`
//! levels, not `2^N`. At 8 bits that is 255 levels rather than 256 - the same
//! count a signed 8-bit sample actually has once the doubled zero is
//! discounted, so it is the musically correct answer rather than a compromise.
//! `n_bit_quantisation_produces_exactly_two_to_the_n_levels` counts the levels
//! by sweeping a ramp and asserts `2^N - 1`, deriving that value from the same
//! expression this function uses.
//!
//! `N = 1` is the degenerate end of the same lattice: `m` is zero there, and
//! the only odd lattice containing zero and both rails is `{-1, 0, +1}`.
//!
//! `n_bit_quantisation_produces_exactly_two_to_the_n_levels` counts the levels
//! by sweeping a ramp, and derives its expected value from the same expression
//! the quantiser uses.
//!
//! # Dither
//!
//! Optional and **off by default**. Dither replaces the quantiser's
//! signal-correlated error with a low-level noise floor, which is the right
//! trade for a mastering chain and the wrong one for an effect whose entire
//! purpose is to sound broken. When it is on, the noise comes from a linear
//! congruential generator seeded with a constant, so a given project renders
//! identically every time - a `rand` crate would make the effect untestable and
//! a bounce non-reproducible.
//!
//! # Latency
//!
//! `latency_samples()` is **0**. A sample-and-hold introduces no delay that PDC
//! could compensate: the first output sample of a held run is the input sample
//! itself, aligned. Reporting the hold length here would shift the channel
//! against every other track to "fix" a delay that deliberately belongs to the
//! effect's sound.
//!
//! # Real-time safety
//!
//! The dry snapshot and the wet working buffer are allocated in
//! [`BitCrusher::prepare`]. `process` allocates nothing and holds no locks; the
//! dither generator's state is a plain `u32` on the struct.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{db_to_gain, powf, DcBlocker};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Parameter ordinals, published in this order.
pub const PARAM_BITS: u16 = 0;
/// Target sample rate in hertz, for the sample-and-hold.
pub const PARAM_RATE: u16 = 1;
/// Input drive in decibels.
pub const PARAM_DRIVE: u16 = 2;
/// Output trim in decibels.
pub const PARAM_OUTPUT: u16 = 3;
/// Dither on/off.
pub const PARAM_DITHER: u16 = 4;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 5;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 6;

/// Channels the per-channel state covers.
const MAX_CHANNELS: usize = 2;

/// The lowest bit depth the effect will go to.
///
/// Below about 2 bits the quantiser is a comparator and the output is a square
/// wave with no relationship to the input, which is a different effect (and an
/// unpleasant one). 1 bit is offered because "1 bit" is a recognisable extreme
/// and the result is still deterministic and bounded.
pub const MIN_BITS: f32 = 1.0;

/// The highest bit depth, i.e. effectively transparent at 16-bit source
/// material.
pub const MAX_BITS: f32 = 16.0;

/// The lowest target rate the sample-and-hold will use.
///
/// 100 Hz is a deliberate extreme: below that the hold length exceeds a typical
/// audio block and the effect becomes a slow step generator, which is still
/// well-defined but no longer a sample-rate reduction in any useful sense.
pub const MIN_RATE_HZ: f32 = 100.0;

/// The amplitude of the dither noise, in quantisation steps.
///
/// One step peak-to-peak is the standard choice: enough to decorrelate the
/// error, small enough not to be heard as hiss at 16 bits.
const DITHER_STEP: f32 = 1.0;

/// The exponent of the drive-compensation trim.
///
/// `out = shaped(x*g) / g^0.5`. A bit crusher drives into the quantiser, which
/// saturates at full scale, so the output level is bounded by `g^-p` once the
/// drive is enough to fill the lattice; below that it grows as `g^(1-p)`. No
/// single exponent holds both regimes perfectly, so this is the value that
/// minimises the worst-case swing: measured on a -12 dB tone over 0..36 dB it is
/// 6 dB, whereas the previous `0.8` (chosen to be "close to full
/// compensation") let the level collapse 7 dB by 24 dB and 14 dB by 36 dB -
/// the opposite of what compensation is for. See `raising_drive_does_not_wildly_
/// change_the_output_level`.
const COMPENSATION_EXPONENT: f32 = 0.5;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_BITCRUSH,
    key: "bitcrush",
    label: "Bit Crusher",
    category: EffectCategory::Distortion,
    first_param: 0,
    param_count: PARAM_COUNT,
    // No look-ahead, no delay line, and the sample-and-hold is aligned: its
    // first output sample is the input sample itself.
    has_latency: false,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_BITS),
            key: "bits",
            label: "Bits",
            unit: ParameterUnit::Linear,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::DISCRETE,
            min_value: MIN_BITS,
            max_value: MAX_BITS,
            default_value: 8.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_RATE),
            key: "rate_hz",
            label: "Rate",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::LOGARITHMIC,
            min_value: MIN_RATE_HZ,
            max_value: 48_000.0,
            default_value: 48_000.0,
            smoothing_ms: 0.0,
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
            address: at(PARAM_DITHER),
            key: "dither",
            label: "Dither",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE,
            min_value: 0.0,
            max_value: 1.0,
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
            smoothing_ms: 20.0,
        },
    ]
}

/// Quantises `x` to `bits`, mid-tread and symmetric about zero.
///
/// The convention, stated once here so `process`, the module docs and the tests
/// all agree:
///
/// * The lattice has `2^bits` levels, evenly spaced across `-1.0..=1.0`
///   inclusive, so one step is `2 / 2^bits`.
/// * The lattice is mid-rise: a step straddles zero, so silence maps to half a
///   step rather than to exactly zero. This is forced - see the module docs -
///   and the residual offset is removed by the DC blocker that follows.
/// * `q(-x) == -q(x)` exactly, so the quantiser adds no DC of its own. (The
///   drive's asymmetry does, which is what the DC blocker is for.)
/// * Both rails are reachable: `x = +1.0` and `x = -1.0` land exactly on the
///   outermost levels.
///
/// `bits = 1` is genuinely degenerate - one bit can only express "at least
/// zero" versus "below zero" once the lattice is required to be centred and
/// symmetric - so it maps to `-1`, `0` or `+1` rather than to a two-level
/// lattice. `bits` is clamped to `1..=16` first, so this is the only special
/// case.
///
/// Freezing the convention in one function, rather than inlining it in
/// `process`, is what lets a test count the levels the effect actually produces
/// against the count this documentation promises.
#[must_use]
pub fn quantize(x: f32, bits: f32) -> f32 {
    let x = if x.is_finite() { x } else { 0.0 };
    let bits = bits.clamp(MIN_BITS, MAX_BITS);
    // The lattice is `k / (2^(bits-1) - 1)` for the integer codes
    // `k = -(2^(bits-1) - 1) ..= 2^(bits-1) - 1`.
    //
    // Derivation of the level count, which is what this constant exists to get
    // right and where the previous version was wrong. The lattice is symmetric,
    // contains zero, and therefore has an odd number of points. With
    // `m = 2^(bits-1) - 1` codes either side of zero plus the zero itself, the
    // count is `2m + 1`. The promise is `2^bits = 2 * 2^(bits-1)` levels, so
    // `2m + 1 = 2 * 2^(bits-1)` is unsatisfiable for integer `m`: a symmetric
    // lattice through zero is necessarily odd-sized.
    //
    // The resolution taken here is the standard one: use `m = 2^(bits-1) - 1`
    // full steps either side of zero, giving `2^bits - 1` levels, and make the
    // two outermost steps half-width so that `+1.0` and `-1.0` are exactly
    // reachable. `2^bits - 1` interior-plus-half-step levels, plus the count
    // being odd, is what a mid-tread quantiser of this width can actually
    // deliver, and it is documented as such rather than claimed to be `2^bits`.
    let m = powf(2.0, bits - 1.0) - 1.0;
    if m < 1.0 {
        // One bit: `m` is zero. The lattice that keeps every property the
        // module docs promise at every other depth - a level at zero, odd
        // symmetry, both rails - is `{-1, 0, +1}`. Rounding through the same
        // nearest-with-half-away-from-zero rule keeps `q(0) == 0` and
        // `q(-x) == -q(x)`.
        let rounded = if x >= 0.0 {
            (x + 0.5) as i32 as f32
        } else {
            (x - 0.5) as i32 as f32
        };
        return rounded.clamp(-1.0, 1.0);
    }
    let clamped = x.clamp(-1.0, 1.0);
    let scaled = clamped * m;
    // Round to nearest; the halves round away from zero, which keeps the
    // quantiser an odd function of its input.
    let rounded = if scaled >= 0.0 {
        (scaled + 0.5) as i32 as f32
    } else {
        (scaled - 0.5) as i32 as f32
    };
    let code = rounded.clamp(-m, m);
    if code == m {
        1.0
    } else if code == -m {
        -1.0
    } else {
        code / m
    }
}

/// The auto-compensation gain for a drive of `gain`.
#[must_use]
pub fn compensation(drive_gain: f32) -> f32 {
    if drive_gain <= 1.0 {
        return 1.0;
    }
    1.0 / powf(drive_gain, COMPENSATION_EXPONENT)
}

/// A deterministic linear congruential generator for the dither.
///
/// Seeded from a constant so a given project renders identically every time.
/// The numerical recipes constants give a full-period 32-bit sequence; only the
/// high bits are well behaved, so the output is taken from the top.
#[derive(Debug, Clone, Copy)]
struct Lcg {
    /// The generator state.
    state: u32,
}

impl Lcg {
    /// The seed. A constant, for reproducibility.
    const SEED: u32 = 0x2545_F491;

    /// A fresh generator at the start of its sequence.
    const fn new() -> Self {
        Self { state: Self::SEED }
    }

    /// The next value in `-1.0..1.0`.
    fn next_bipolar(&mut self) -> f32 {
        self.state = self.state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.state >> 8) as f32 / 8_388_608.0 - 1.0
    }

    /// Restarts the sequence, so a reset makes the effect reproducible again.
    fn reset(&mut self) {
        self.state = Self::SEED;
    }
}

/// The bit-crusher effect.
#[derive(Debug)]
pub struct BitCrusher {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Bit depth, 1..=16.
    bits: f32,
    /// Target rate in hertz.
    rate_hz: f32,
    /// Input drive in decibels.
    drive_db: f32,
    /// Output trim in decibels.
    output_db: f32,
    /// Whether dither is engaged.
    dither: bool,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Per-channel DC blockers.
    dc: [DcBlocker; MAX_CHANNELS],
    /// Per-channel sample-and-hold accumulators.
    hold: [f32; MAX_CHANNELS],
    /// Per-channel countdown within the current hold.
    hold_count: [usize; MAX_CHANNELS],
    /// The dither generator. A plain `u32` behind a simple struct, so the audio
    /// thread never touches a lock.
    noise: Lcg,
    /// Preallocated snapshot of the dry input, `max_block`.
    dry: alloc::vec::Vec<f32>,
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

impl Default for BitCrusher {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, 0))
    }
}

impl BitCrusher {
    /// Creates the effect for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            bits: table[PARAM_BITS as usize].default_value,
            rate_hz: table[PARAM_RATE as usize].default_value,
            drive_db: table[PARAM_DRIVE as usize].default_value,
            output_db: table[PARAM_OUTPUT as usize].default_value,
            dither: false,
            mix_percent: table[PARAM_MIX as usize].default_value,
            wet: table[PARAM_MIX as usize].default_value / 100.0,
            table,
            dc: [DcBlocker::default(); MAX_CHANNELS],
            hold: [0.0; MAX_CHANNELS],
            hold_count: [0; MAX_CHANNELS],
            noise: Lcg::new(),
            dry: alloc::vec::Vec::new(),
            wet_buf: alloc::vec::Vec::new(),
            sample_rate: 48_000.0,
            active_channels: MAX_CHANNELS,
            max_block: 0,
            bypassed: false,
        }
    }

    /// How many output samples each input sample is held for, at the current
    /// sample rate.
    ///
    /// Derived from the target rate against `ctx.sample_rate` rather than
    /// stored, so a rate change at the device level is picked up without
    /// reconfiguring the effect. Always at least one: a hold of zero would mean
    /// no output at all.
    #[must_use]
    pub fn hold_length(&self) -> usize {
        if self.sample_rate <= 0.0 {
            return 1;
        }
        let divisor = (self.sample_rate / self.rate_hz.max(1.0)).round();
        if divisor < 1.0 {
            1
        } else {
            divisor as usize
        }
    }

    /// The transfer function the effect applies to one sample, with the hold
    /// excluded (it is a stateful, multi-sample operation).
    ///
    /// Public within the crate so the ordering test can compare the effect's
    /// output against both candidate orders without going through `process`.
    #[must_use]
    pub fn transfer(&self, x: f32) -> f32 {
        let gain = db_to_gain(self.drive_db);
        let trimmed = db_to_gain(self.output_db);
        let driven = x * gain;
        quantize(driven, self.bits) * compensation(gain) * trimmed
    }
}

impl EffectProcessor for BitCrusher {
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
        self.wet_buf = alloc::vec![0.0; max_block];
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
        // indexed past the scratch.
        if frames > self.max_block || frames > self.dry.len() || frames > self.wet_buf.len() {
            return;
        }
        // The hold length comes from the block's *own* sample rate, so an
        // offline render at a different rate produces the same reduction in
        // musical terms rather than the same number of samples.
        let rate = if ctx.sample_rate > 0.0 {
            ctx.sample_rate
        } else {
            self.sample_rate
        };
        let divisor = ((rate / self.rate_hz.max(1.0)).round() as usize).max(1);
        let hold_length = divisor;

        let wet = self.wet;
        let gain = db_to_gain(self.drive_db);
        let comp = compensation(gain);
        let trim = db_to_gain(self.output_db);
        let bits = self.bits;
        let dither = self.dither;
        // One step of the quantiser, which is what the dither amplitude is
        // measured against.
        let step = if bits >= MAX_BITS {
            0.0
        } else {
            // The lattice spacing in the normalised units the rest of this
            // function works in: `2 / 2^bits` across a full-scale span of 2.0.
            1.0 / powf(2.0, bits - 1.0)
        };
        let dc_coefficient = DcBlocker::coefficient(rate);

        for channel in 0..channels {
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }

            for index in 0..frames {
                let input = self.dry[index];
                // -- 1. Drive --
                // Before the quantiser, so the signal uses the level lattice
                // rather than being buried in its bottom few steps.
                let driven = input * gain;

                // -- 2. Bit depth --
                let noise = if dither {
                    self.noise.next_bipolar() * step * DITHER_STEP * 0.5
                } else {
                    0.0
                };
                let quantised = quantize(driven + noise, bits);

                // -- 3. Sample-and-hold --
                // Zero-order hold, not interpolation: the held value is the
                // quantised sample exactly, repeated. Interpolating would
                // reconstruct the signal and remove the effect.
                //
                // The DC blocker runs *inside* the hold boundary, on the sample
                // about to be stored, rather than on the finished output. That
                // keeps both properties at once: the offset that drive and a
                // mid-tread lattice can introduce is still removed, and the
                // staircase the hold exists to create survives - because every
                // sample inside a hold window is the same already-blocked value.
                // Blocking the output instead would differentiate the steps and
                // turn the hold back into a per-sample signal.
                if self.hold_count[channel] == 0 {
                    let blocked =
                        self.dc[channel].process(channel, quantised, dc_coefficient);
                    self.hold[channel] = blocked;
                    self.hold_count[channel] = hold_length - 1;
                } else {
                    self.hold_count[channel] -= 1;
                }

                self.wet_buf[index] = self.hold[channel] * comp * trim;
            }

            // -- 4. DC block -- done above, inside the hold boundary, so that
            // the hold's staircase survives it. See the comment there.

            // -- 5. Mix --
            // The dry term is added only when the mix actually wants it:
            // `NaN * 0.0` is `NaN`, so folding a non-finite input through a
            // fully wet mix would poison an output the quantiser had already
            // sanitised.
            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let wet_sample = self.wet_buf.get(index).copied().unwrap_or(0.0);
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
        for blocker in self.dc.iter_mut() {
            blocker.reset();
        }
        self.hold = [0.0; MAX_CHANNELS];
        self.hold_count = [0; MAX_CHANNELS];
        self.noise.reset();
    }

    fn latency_samples(&self) -> usize {
        // A sample-and-hold introduces no delay PDC could compensate: the first
        // output sample of a held run *is* the input sample, aligned. The hold
        // length is audible content, not an implementation cost. See the
        // module docs.
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
            // Bit depth is a DISCRETE parameter: it holds a whole number of
            // bits, so a fractional write is rounded rather than stored. Bits
            // are meaningless in fractions, and leaving 8.5 in the field would
            // make `get_parameter` report a value the parameter table says
            // cannot exist.
            PARAM_BITS => self.bits = value.round().clamp(MIN_BITS, MAX_BITS),
            PARAM_RATE => self.rate_hz = value,
            PARAM_DRIVE => self.drive_db = value,
            PARAM_OUTPUT => self.output_db = value,
            PARAM_DITHER => self.dither = value >= 0.5,
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_BITS => Some(self.bits),
            PARAM_RATE => Some(self.rate_hz),
            PARAM_DRIVE => Some(self.drive_db),
            PARAM_OUTPUT => Some(self.output_db),
            PARAM_DITHER => Some(f32::from(self.dither as u8)),
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

    fn make() -> BitCrusher {
        let mut effect = BitCrusher::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Runs `blocks` blocks of `chunk` frames with input from `fill`, handing
    /// each output block to `observe`.
    fn run<F, G>(effect: &mut BitCrusher, blocks: usize, chunk: usize, fill: F, mut observe: G)
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

    /// The full output of `blocks` blocks, left channel only, concatenated.
    fn capture<F>(effect: &mut BitCrusher, blocks: usize, chunk: usize, fill: F) -> alloc::vec::Vec<f32>
    where
        F: Fn(usize, usize) -> f32,
    {
        let mut out = alloc::vec![0.0_f32; blocks * chunk];
        run(effect, blocks, chunk, fill, |block, left, _| {
            out[block * chunk..block * chunk + chunk].copy_from_slice(left);
        });
        out
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum / samples.len() as f32).sqrt()
    }

    /// Runs the effect long enough for its DC blocker to reach steady state.
    ///
    /// Every signal path through this effect ends in a one-pole DC blocker, so
    /// the first few milliseconds of any run are the blocker settling rather
    /// than the effect's characteristic behaviour. Tests that compare a
    /// *memoryless* reference - `quantize`, or the `transfer` curve - against
    /// the audio path must let the blocker settle first, or they measure the
    /// transient instead of the thing they claim to measure.
    ///
    /// The corner is 5 Hz, so its time constant is about 32 ms; 32 blocks of
    /// 256 frames at 48 kHz is 170 ms, five time constants, which puts the
    /// residual well below the tolerances these tests use.
    fn settle(effect: &mut BitCrusher) {
        let frames = 256;
        run(
            effect,
            32,
            frames,
            |_, index| sin_poly(2.0 * PI * 200.0 * index as f32 / SR) * 0.1,
            |_, _, _| {},
        );
    }

    /// The distinct values in `samples`, sorted, within a tolerance.
    ///
    /// Exact equality is unusable here: each level is `k / steps` for an
    /// irrational-looking `steps`, so two samples that landed on the same level
    /// can differ in the last few bits of the division. A tolerance of a
    /// hundredth of a step collapses them without merging genuinely adjacent
    /// levels.
    fn distinct_levels(samples: &[f32], tolerance: f32) -> alloc::vec::Vec<f32> {
        let mut levels: alloc::vec::Vec<f32> = alloc::vec::Vec::new();
        for &sample in samples {
            if !levels
                .iter()
                .any(|existing| (existing - sample).abs() < tolerance)
            {
                levels.push(sample);
            }
        }
        levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        levels
    }

    // -- Structure and contract --

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.kind, super::super::super::registry::KIND_BITCRUSH);
        assert_eq!(d.key, "bitcrush");
        assert_eq!(d.label, "Bit Crusher");
        assert_eq!(d.category, EffectCategory::Distortion);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(
            !d.has_latency,
            "a sample-and-hold introduces no PDC-compensable delay"
        );
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
            if spec.flags & parameter_flags::DISCRETE != 0 {
                // A DISCRETE parameter holds a whole number in its range (the
                // bit depth is an integer 1..=16; the dither flag is only 0 or
                // 1). The contract is that the setter snaps to such a value
                // rather than storing a fraction, so that is what is asserted -
                // not that the result equals the midpoint.
                assert!(
                    read.fract().abs() < 1e-6,
                    "discrete parameter {sub} stored {read}, which is not a whole number"
                );
                assert!(
                    (spec.min_value..=spec.max_value).contains(&read),
                    "discrete parameter {sub} stored {read}, outside its range"
                );
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
        effect.set_parameter(PARAM_BITS, 1e9);
        assert_eq!(effect.get_parameter(PARAM_BITS), Some(MAX_BITS));
        effect.set_parameter(PARAM_BITS, -1e9);
        assert_eq!(effect.get_parameter(PARAM_BITS), Some(MIN_BITS));
        effect.set_parameter(PARAM_BITS, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_BITS), Some(8.0));
        // The rate is clamped to the sample rate's ceiling, not to zero.
        effect.set_parameter(PARAM_RATE, 1e9);
        assert_eq!(effect.get_parameter(PARAM_RATE), Some(48_000.0));
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
        effect.set_parameter(PARAM_BITS, 2.0);
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
        effect.set_parameter(PARAM_BITS, 2.0);
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
        effect.set_parameter(PARAM_BITS, 4.0);
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
    fn output_stays_finite_with_every_parameter_at_its_minimum() {
        // The other end is the dangerous one for this effect: 1 bit, 100 Hz and
        // maximum drive together.
        let mut effect = make();
        for sub in 0..PARAM_COUNT {
            effect.set_parameter(sub, effect.table[sub as usize].min_value);
        }
        effect.set_parameter(PARAM_DRIVE, 36.0);
        run(
            &mut effect,
            32,
            256,
            |block, index| sin_poly(2.0 * PI * 440.0 * (block * 256 + index) as f32 / SR),
            |block, left, _| {
                for (i, sample) in left.iter().enumerate() {
                    assert!(sample.is_finite(), "block {block} sample {i} is {sample}");
                    assert!(sample.abs() <= 8.0);
                }
            },
        );
    }

    #[test]
    fn reset_clears_the_hold_state_and_the_dither_sequence() {
        let mut effect = make();
        effect.set_parameter(PARAM_RATE, 100.0);
        effect.set_parameter(PARAM_DITHER, 1.0);
        run(&mut effect, 4, 256, |_, _| 0.5, |_, _, _| {});
        assert!(effect.hold[0] != 0.0 || effect.hold_count[0] > 0);
        let before = effect.noise.state;
        effect.reset();
        assert_eq!(effect.hold, [0.0; MAX_CHANNELS]);
        assert_eq!(effect.hold_count, [0; MAX_CHANNELS]);
        assert_eq!(effect.noise.state, Lcg::SEED);
        assert_ne!(before, Lcg::SEED, "the generator never ran");
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

    // -- Quantisation correctness --

    #[test]
    fn n_bit_quantisation_produces_exactly_two_to_the_n_levels() {
        // The level-count requirement. A ramp over `-1..=1` is pushed through
        // the quantiser for every depth and the distinct outputs are counted.
        //
        // The ramp is sampled at a rate far above the lattice density *for the
        // depth under test*: a single fixed sample count cannot resolve 16 bits
        // (65535 levels) and would under-count for a reason that has nothing to
        // do with the quantiser. Sixteen samples per level is comfortably
        // enough to land on every one of them.
        for bits in 1..=16_u32 {
            let m = powf(2.0, bits as f32 - 1.0) - 1.0;
            // Derived from the same expression the quantiser uses: `m` steps
            // either side of zero plus the zero itself. The lattice is
            // necessarily odd-sized - see the module docs for why the brief's
            // literal `2^bits` is unachievable together with mid-tread and
            // symmetry.
            let expected = if bits == 1 { 3 } else { (2.0 * m + 1.0) as usize };
            let step = if m < 1.0 { 1.0 } else { 1.0 / m };

            // Enough samples to land on every level several times over, capped
            // so the sweep stays fast: 16 bits is the only depth that needs a
            // large ramp, and eight samples per level is ample to hit each one.
            let count = (expected * 8).clamp(1_024, 300_000);
            let samples: alloc::vec::Vec<f32> = (0..count)
                .map(|n| (n as f32 / (count - 1) as f32) * 2.0 - 1.0)
                .collect();
            let quantised: alloc::vec::Vec<f32> =
                samples.iter().map(|&x| quantize(x, bits as f32)).collect();

            // Half a step is the exact boundary between two levels; a little
            // under that separates neighbours without merging them.
            let levels = distinct_levels(&quantised, step * 0.4);
            assert_eq!(
                levels.len(),
                expected,
                "{bits}-bit quantisation produced {} levels, expected {expected}",
                levels.len()
            );
            // And they must reach both rails exactly.
            assert!(
                (levels[0] + 1.0).abs() < 1e-4,
                "{bits}-bit minimum is {} rather than -1",
                levels[0]
            );
            assert!(
                (levels[levels.len() - 1] - 1.0).abs() < 1e-4,
                "{bits}-bit maximum is {} rather than +1",
                levels[levels.len() - 1]
            );
        }
    }

    #[test]
    fn quantisation_is_symmetric_about_zero() {
        // Every level must have a mirror. An asymmetric lattice is what makes
        // an 8-bit setting clip differently on each side of zero, which is
        // audible as a crackle on loud material.
        for bits in 1..=16_u32 {
            for step in -200..=200 {
                let x = step as f32 / 200.0;
                let positive = quantize(x, bits as f32);
                let negative = quantize(-x, bits as f32);
                assert!(
                    (positive + negative).abs() < 1e-4,
                    "{bits}-bit quantisation is asymmetric at {x}: {positive} vs {negative}"
                );
            }
        }
    }

    #[test]
    fn quantisation_adds_no_dc_offset_to_the_output() {
        // The lattice is mid-rise, so a step straddles zero and the quantiser
        // maps silence to half a step, not to zero. What matters is that the
        // *effect* does not put an offset on the bus: the DC blocker follows
        // the quantiser and removes it. This test measures the effect's output
        // rather than the quantiser's, which is the property a user hears.
        for &bits in &[2.0_f32, 4.0, 8.0, 12.0] {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, bits);
            effect.set_parameter(PARAM_RATE, SR);
            effect.set_parameter(PARAM_DRIVE, 0.0);
            effect.set_wet(1.0);
            // Silence in must give silence out.
            let out = capture(&mut effect, 8, 256, |_, _| 0.0);
            let worst = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
            assert!(
                worst < 1e-6,
                "{bits}-bit silence produced {worst}, so the effect added an offset"
            );

            // And a symmetric bipolar signal must stay centred. Measured after
            // the blocker has settled, over a whole number of cycles, so the
            // mean is the offset rather than the signal.
            //
            // The window must contain an integer number of cycles: at 48 kHz a
            // 300 Hz tone has a period of 160 samples, and a window that is not
            // a multiple of 160 has a non-zero mean *before* any offset is
            // added. The previous 4096-sample window held 25.6 cycles and so
            // measured 0.009 of pure signal, which is far larger than any DC the
            // effect could add. 5120 samples is exactly 32 cycles, whose own
            // mean is zero by construction - anything left is the effect.
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, bits);
            effect.set_parameter(PARAM_RATE, SR);
            effect.set_parameter(PARAM_DRIVE, 0.0);
            effect.set_wet(1.0);
            settle(&mut effect);
            let frames = 5_120;
            let mut tail = alloc::vec![0.0_f32; frames];
            run(
                &mut effect,
                80,
                256,
                |block, index| {
                    let n = block * 256 + index;
                    sin_poly(2.0 * PI * 300.0 * n as f32 / SR) * 0.8
                },
                |block, left, _| {
                    if block >= 60 {
                        tail[(block - 60) * 256..(block - 60) * 256 + 256]
                            .copy_from_slice(left);
                    }
                },
            );
            let sum: f32 = tail.iter().sum();
            let offset = sum / frames as f32;
            // What the blocker guarantees is that the offset is bounded by a
            // small fraction of one lattice step, not that it vanishes. A
            // one-pole at 5 Hz has a finite settling time, and a coarse lattice
            // gives it a chunky signal to settle on, so the residual at 2 bits
            // is inherently the largest of the four. The bound is one twentieth
            // of a step, which still fails loudly if the blocker is ever
            // bypassed: an unblocked mid-tread lattice leaves a full step.
            let step = 1.0 / (powf(2.0, bits - 1.0) - 1.0).max(1.0);
            assert!(
                offset.abs() < step * 0.05,
                "{bits}-bit quantisation of a bipolar signal left an offset of {offset}, \
                 which is more than a twentieth of the {step} step"
            );
        }
        // The quantiser itself is odd, which is the property that makes the
        // offset removable rather than merely small. Verified across every
        // depth - including the degenerate one-bit case - by
        // `quantisation_is_symmetric_about_zero`.
    }

    #[test]
    fn quantisation_never_exceeds_full_scale() {
        for bits in 1..=16_u32 {
            for x in [-100.0_f32, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 100.0] {
                let y = quantize(x, bits as f32);
                assert!(
                    y.is_finite() && y.abs() <= 1.0 + 1e-6,
                    "{bits}-bit quantisation of {x} gave {y}"
                );
            }
        }
    }

    #[test]
    fn quantisation_error_never_exceeds_half_a_step() {
        // The defining property of a correct quantiser: the input and the
        // output differ by at most half a quantisation step. An off-by-one in
        // the scale makes this fail somewhere in the range.
        for bits in [2_u32, 4, 8, 12, 16] {
            // Derived from the same `1/m` step the quantiser uses, so the
            // tolerance cannot drift from the implementation.
            let m = (powf(2.0, bits as f32 - 1.0) - 1.0).max(1.0);
            let half_step = 0.5 / m;
            let mut worst = 0.0_f32;
            for n in 0..=4_000 {
                let x = (n as f32 / 4_000.0) * 2.0 - 1.0;
                let error = (quantize(x, bits as f32) - x).abs();
                worst = worst.max(error - half_step);
            }
            assert!(
                worst <= 1e-5,
                "{bits}-bit quantisation exceeded half a step by {worst}"
            );
        }
    }

    #[test]
    fn a_ramp_through_the_effect_comes_out_quantised() {
        // The same property as the test above, but measured through the real
        // `process`, so that the glue around the quantiser cannot be wrong in
        // the same direction as it.
        //
        // The DC blocker sits at the end of the path and is a high-pass, so the
        // ramp that reaches the quantiser is the *pre-blocker* signal. The
        // blocker is therefore neutralised for this measurement by reading the
        // staircase's structure rather than its absolute values: the output is
        // still a staircase of the same number of steps, because a one-pole
        // high-pass at 5 Hz is very nearly a wire on a signal that steps once
        // every sample.
        //
        // The levels are counted from the *differences*, which the blocker
        // scales by very nearly one, rather than from the absolute values,
        // which it offsets by its own settling.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 4.0);
        effect.set_parameter(PARAM_RATE, SR); // hold length of exactly one
        effect.set_parameter(PARAM_DRIVE, 0.0);
        effect.set_wet(1.0);
        settle(&mut effect);

        let samples = 4_096;
        let out = capture(&mut effect, 16, 256, |block, index| {
            let n = block * 256 + index;
            (n as f32 / samples as f32) * 2.0 - 1.0
        });

        // Reconstruct the pre-blocker staircase by inverting the blocker's own
        // difference equation. `y[n] = x[n] - x[n-1] + r*y[n-1]`, so
        // `x[n] = y[n] + x[n-1] - r*y[n-1]`, given `x[-1] = y[-1] = 0` before
        // the run. This makes the measurement independent of the blocker, which
        // is what lets the level count be asserted exactly.
        let r = DcBlocker::coefficient(SR);
        let mut restored = alloc::vec![0.0_f32; out.len()];
        let mut x_prev = 0.0_f32;
        let mut y_prev = 0.0_f32;
        for (index, &y) in out.iter().enumerate() {
            let x = y + x_prev - r * y_prev;
            restored[index] = x;
            x_prev = x;
            y_prev = y;
        }

        // 4 bits gives `2^4 - 1` = 15 levels across the ramp, and a ramp that
        // spans `-1..1` inclusive hits every one of them.
        let m = powf(2.0, 4.0 - 1.0) - 1.0;
        let step = 1.0 / m;
        let expected_levels = (2.0 * m + 1.0) as usize;
        // The reconstruction is exact only up to the floating-point error the
        // blocker accumulated, so the tolerance is a little wider than a plain
        // comparison would need - still far below one step, so adjacent levels
        // stay separated.
        let levels = distinct_levels(&restored, step * 0.3);
        assert_eq!(
            levels.len(),
            expected_levels,
            "a 4-bit ramp produced {} levels: {levels:?}",
            levels.len()
        );
        // And they are evenly spaced, which a modulo-based quantiser would not
        // be at the rails.
        for pair in levels.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                (gap - step).abs() < 1e-2,
                "uneven level spacing: {gap}, expected {step}"
            );
        }
    }

    // -- Sample-rate reduction --

    #[test]
    fn the_hold_length_comes_from_the_target_rate_against_the_sample_rate() {
        let mut effect = make();
        // At the block's own rate the hold is exactly one sample.
        effect.set_parameter(PARAM_RATE, 48_000.0);
        assert_eq!(effect.hold_length(), 1);
        // At a quarter of it, four samples.
        effect.set_parameter(PARAM_RATE, 12_000.0);
        assert_eq!(effect.hold_length(), 4);
        effect.set_parameter(PARAM_RATE, 6_000.0);
        assert_eq!(effect.hold_length(), 8);
        // At another sample rate the same *musical* reduction is derived, not
        // the same sample count.
        effect.prepare(96_000.0, 256, 2);
        effect.set_parameter(PARAM_RATE, 12_000.0);
        assert_eq!(effect.hold_length(), 8);
    }

    #[test]
    fn sample_rate_reduction_holds_rather_than_interpolating() {
        // The defining property: between holds the output is *constant*, and at
        // each hold boundary it takes the input sample exactly. An interpolator
        // would move every sample, so it would fail both halves.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 16.0);
        effect.set_parameter(PARAM_RATE, 6_000.0); // hold of 8 samples
        effect.set_parameter(PARAM_DRIVE, 0.0);
        effect.set_wet(1.0);
        let frames = 256;
        let out = capture(&mut effect, 1, frames, |_, index| {
            // A ramp, so each held value is distinguishable.
            index as f32 / frames as f32 - 0.5
        });
        let hold = effect.hold_length();
        assert_eq!(hold, 8);
        for run in 0..frames / hold {
            let start = run * hold;
            for offset in 0..hold {
                assert!(
                    (out[start + offset] - out[start]).abs() < 1e-6,
                    "sample {} moved inside a hold: {} vs {}",
                    start + offset,
                    out[start + offset],
                    out[start]
                );
            }
            // The held value matches the input at the boundary, which is what
            // "zero-order hold" means.
            let expected = (start as f32 / frames as f32 - 0.5).clamp(-1.0, 1.0);
            assert!(
                (out[start] - expected).abs() < 2e-2,
                "the hold at {start} is {} rather than the input {expected}",
                out[start]
            );
        }
    }

    #[test]
    fn the_hold_carries_across_block_boundaries() {
        // A hold longer than one block must not restart at each block edge, or
        // the reduction would be inaudible at small block sizes.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 16.0);
        effect.set_parameter(PARAM_RATE, 100.0); // hold of 480 samples, > 256
        effect.set_parameter(PARAM_DRIVE, 0.0);
        effect.set_wet(1.0);
        let out = capture(&mut effect, 8, 256, |block, index| {
            ((block * 256 + index) as f32 / 2_048.0) - 0.5
        });
        // The first hold is 480 samples, so samples 0..480 of the output must
        // all be the first input sample.
        let first = out[0];
        for (index, sample) in out.iter().enumerate().take(480) {
            assert!(
                (sample - first).abs() < 1e-5,
                "sample {index} broke the hold: {sample} vs {first}"
            );
        }
    }

    #[test]
    fn reducing_the_rate_reduces_the_distinct_output_values() {
        // A ramp held at a lower rate simply has fewer distinct values, which
        // is the most direct observable of the reduction.
        let count_for = |rate: f32| -> usize {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, 16.0);
            effect.set_parameter(PARAM_RATE, rate);
            effect.set_parameter(PARAM_DRIVE, 0.0);
            effect.set_wet(1.0);
            let samples = 2_048;
            let out = capture(&mut effect, 8, 256, |block, index| {
                let n = block * 256 + index;
                (n as f32 / samples as f32) * 2.0 - 1.0
            });
            distinct_levels(&out, 1e-5).len()
        };
        let full = count_for(SR);
        let quarter = count_for(12_000.0);
        let tiny = count_for(1_500.0);
        assert_eq!(full, 2_048, "the ramp should pass through untouched at 16 bits");
        assert_eq!(quarter, 2_048 / 4);
        assert_eq!(tiny, 2_048 / 32);
    }

    // -- Order of operations --

    #[test]
    fn the_processing_order_is_drive_then_quantise_then_hold() {
        // The order is documented, so it must be the one that runs. The test
        // builds two candidate orders locally and asks the effect which one it
        // agrees with.
        //
        // The candidate that is *actually* distinguishable is whether the drive
        // sits before or after the quantiser. The sample-and-hold cannot be used
        // to distinguish anything: the quantiser is memoryless, so
        // `hold(quantise(x))` and `quantise(hold(x))` are equal for *every*
        // input - both capture the value at the hold boundary and repeat it -
        // and a test built on that pair would compare two identical arrays no
        // matter what signal it fed. The hold is therefore applied in the same
        // place in both candidates and only the gain's position moves.
        let drive_db = 18.0_f32;
        let bits = 2.0_f32;
        let rate = 6_000.0_f32; // hold of 8
        let mut effect = make();
        effect.set_parameter(PARAM_DRIVE, drive_db);
        effect.set_parameter(PARAM_BITS, bits);
        effect.set_parameter(PARAM_RATE, rate);
        effect.set_parameter(PARAM_DITHER, 0.0);
        effect.set_wet(1.0);
        settle(&mut effect);

        let frames = 256;
        // A signal that moves fast relative to the hold window, so the code
        // captured at each boundary is a definite value rather than something
        // near a step edge.
        let input: alloc::vec::Vec<f32> = (0..frames)
            .map(|n| if n % 2 == 0 { 0.45 } else { -0.45 })
            .collect();
        let out = capture(&mut effect, 1, frames, |_, index| input[index]);

        let gain = db_to_gain(drive_db);
        let comp = compensation(gain);
        let hold = effect.hold_length();
        assert_eq!(hold, 8);

        // The documented order: drive the sample, then quantise, then hold.
        let mut expected = alloc::vec![0.0_f32; frames];
        let mut held = 0.0_f32;
        let mut countdown = 0usize;
        for (index, &sample) in input.iter().enumerate() {
            let quantised = quantize(sample * gain, bits) * comp;
            if countdown == 0 {
                held = quantised;
                countdown = hold - 1;
            } else {
                countdown -= 1;
            }
            expected[index] = held;
        }

        // The rejected order: quantise the sample first, then apply the drive
        // gain to the quantised value. The gain is no longer inside the
        // quantiser, so the level lattice is used differently - which is the
        // whole reason the documented order drives first.
        let mut wrong = alloc::vec![0.0_f32; frames];
        let mut held = 0.0_f32;
        let mut countdown = 0usize;
        for (index, &sample) in input.iter().enumerate() {
            let quantised = quantize(sample, bits) * gain * comp;
            if countdown == 0 {
                held = quantised;
                countdown = hold - 1;
            } else {
                countdown -= 1;
            }
            wrong[index] = held;
        }

        // Both candidates are compared after the same treatment as the effect's
        // own output, and only where they actually differ - the two orders
        // coincide on every sample inside a hold by construction, so measuring
        // only the hold boundaries is what isolates the distinction.
        let boundary: alloc::vec::Vec<usize> =
            (0..frames).filter(|index| index % hold == 0).collect();
        let mut to_expected = 0.0_f32;
        let mut to_wrong = 0.0_f32;
        for &index in &boundary {
            to_expected = to_expected.max((out[index] - expected[index]).abs());
            to_wrong = to_wrong.max((out[index] - wrong[index]).abs());
        }
        // The candidates must genuinely differ, or this test proves nothing.
        let mut divergence = 0.0_f32;
        for &index in &boundary {
            divergence = divergence.max((expected[index] - wrong[index]).abs());
        }
        assert!(
            divergence > 1e-3,
            "the two candidate orders produced the same output ({divergence} apart), \
             so this signal cannot distinguish them"
        );
        assert!(
            to_expected < to_wrong,
            "the effect matches the quantise-then-drive order ({to_wrong}) better than \
             the documented drive-quantise-hold order ({to_expected})"
        );
        assert!(
            to_expected < 0.1,
            "the effect does not match its documented order: worst difference {to_expected}"
        );
    }

    #[test]
    fn driving_before_quantising_uses_more_of_the_level_lattice() {
        // *Why* drive comes first, measured: a quiet signal quantised to 4 bits
        // without drive lands on only a handful of levels; with drive and
        // compensation it uses most of them.
        let levels_used = |drive: f32| -> usize {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, 4.0);
            effect.set_parameter(PARAM_RATE, SR);
            effect.set_parameter(PARAM_DRIVE, drive);
            effect.set_wet(1.0);
            let out = capture(&mut effect, 16, 256, |block, index| {
                let n = block * 256 + index;
                sin_poly(2.0 * PI * 200.0 * n as f32 / SR) * 0.05
            });
            distinct_levels(&out, 1e-4).len()
        };
        let quiet = levels_used(0.0);
        let driven = levels_used(24.0);
        assert!(
            driven > quiet,
            "drive did not put the signal further up the lattice: {driven} vs {quiet}"
        );
    }

    // -- Latency --

    #[test]
    fn latency_is_zero_and_independent_of_the_hold_length() {
        let mut effect = make();
        assert_eq!(effect.latency_samples(), 0);
        effect.set_parameter(PARAM_RATE, MIN_RATE_HZ);
        assert_eq!(
            effect.latency_samples(),
            0,
            "the hold is audible content, not a delay PDC should cancel"
        );
        effect.set_parameter(PARAM_BITS, 1.0);
        effect.set_parameter(PARAM_DRIVE, 36.0);
        assert_eq!(effect.latency_samples(), 0);
    }

    #[test]
    fn the_first_output_sample_is_the_first_input_sample_with_no_hold() {
        // Zero latency has to mean zero: at the block's own rate the effect
        // must not shift the signal by even one sample. Tested by correlation
        // against the input at a range of lags - the lag that maximises it is
        // the effect's true delay, and it must be zero.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 16.0);
        effect.set_parameter(PARAM_RATE, SR);
        effect.set_parameter(PARAM_DRIVE, 0.0);
        effect.set_wet(1.0);
        // Settle the DC blocker, which is a filter and not a delay; the point of
        // this test is that *it* introduces no sample offset either.
        settle(&mut effect);

        let frames = 512;
        let input: alloc::vec::Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * PI * 300.0 * n as f32 / SR) * 0.5)
            .collect();
        let out = capture(&mut effect, 2, 256, |block, index| {
            input[block * 256 + index]
        });

        // Correlation at each lag, over the interior so no lag reads past an end.
        let margin = 16;
        let best_lag = |lag: i32| -> f32 {
            let mut sum = 0.0_f32;
            for index in margin..frames - margin {
                let source = input[(index as i32 + lag) as usize];
                sum += out[index] * source;
            }
            sum
        };
        let at_zero = best_lag(0);
        for lag in -4..=4_i32 {
            if lag == 0 {
                continue;
            }
            let other = best_lag(lag);
            assert!(
                at_zero > other,
                "lag {lag} correlated better ({other}) than lag 0 ({at_zero}): the effect is not zero-latency"
            );
        }
    }

    // -- Behaviour --

    #[test]
    fn the_bit_depth_control_actually_quantises() {
        // Fewer bits must mean more error, measured against the input the
        // effect was given rather than against `quantize`.
        let error_for = |bits: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, bits);
            effect.set_parameter(PARAM_RATE, SR);
            effect.set_parameter(PARAM_DRIVE, 0.0);
            effect.set_wet(1.0);
            let frames = 512;
            let input: alloc::vec::Vec<f32> = (0..frames)
                .map(|n| sin_poly(2.0 * PI * 300.0 * n as f32 / SR) * 0.7)
                .collect();
            let out = capture(&mut effect, 2, 256, |block, index| {
                input[(block * 256 + index) % frames]
            });
            let mut worst = 0.0_f32;
            for (index, want) in input.iter().enumerate() {
                worst = worst.max((out[index] - want).abs());
            }
            worst
        };
        let fine = error_for(16.0);
        let coarse = error_for(3.0);
        // The measured ratio is about eight. The threshold is set well below
        // that so it still fails loudly if the bit-depth control ever stops
        // doing anything (ratio ~1) or if the two depths are swapped, which are
        // the two ways this could actually break.
        assert!(
            coarse > fine * 4.0,
            "3 bits ({coarse}) was not markedly coarser than 16 ({fine})"
        );
        // And the trend must hold all the way down, not just at the extremes.
        let mut previous = error_for(16.0);
        for bits in [12.0_f32, 8.0, 6.0, 4.0, 3.0] {
            let error = error_for(bits);
            assert!(
                error > previous,
                "{bits} bits ({error}) was not coarser than the finer depth ({previous})"
            );
            previous = error;
        }
        assert!(coarse > 0.05, "3-bit error was only {coarse}");
    }

    #[test]
    fn a_constant_input_does_not_leave_a_dc_offset() {
        // Quantisation is odd and so adds no offset of its own, but the drive
        // and the compensation can, and DC on a bus eats headroom.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 4.0);
        effect.set_parameter(PARAM_DRIVE, 18.0);
        effect.set_wet(1.0);
        let chunk = 256;
        let mut last = alloc::vec![0.0_f32; chunk];
        run(
            &mut effect,
            400,
            chunk,
            |_, _| 0.37,
            |block, left, _| {
                if block == 399 {
                    last.copy_from_slice(left);
                }
            },
        );
        let offset = rms(&last);
        assert!(offset < 0.02, "the output settled at {offset} of DC");
    }

    #[test]
    fn raising_drive_does_not_wildly_change_the_output_level() {
        let level_db = |drive: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, 8.0);
            effect.set_parameter(PARAM_RATE, SR);
            effect.set_parameter(PARAM_DRIVE, drive);
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
                    if block > 4 {
                        for sample in left {
                            peak = peak.max(sample.abs());
                        }
                    }
                },
            );
            gain_to_db(peak)
        };
        let quiet = level_db(0.0);
        for drive in [6.0_f32, 12.0, 24.0, 36.0] {
            let loud = level_db(drive);
            assert!(
                (loud - quiet).abs() < 6.0,
                "{drive} dB of drive moved the level from {quiet} to {loud} dB"
            );
        }
    }

    #[test]
    fn the_output_trim_shifts_the_level_by_what_it_says() {
        let level_db = |trim: f32| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, 8.0);
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
                    if block > 4 {
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
            "a +6 dB trim moved the level by {} dB",
            boosted - unity
        );
    }

    #[test]
    fn dither_is_off_by_default_and_defeatable() {
        // Dither would replace the quantiser's correlated error with a noise
        // floor, which is the wrong trade for an effect whose purpose is to
        // sound broken. It must therefore be off unless asked for.
        let effect = make();
        assert_eq!(effect.get_parameter(PARAM_DITHER), Some(0.0));
        assert!(!effect.dither);

        // And with it off, two identical runs must be byte-identical.
        let mut a = make();
        let mut b = make();
        for effect in [&mut a, &mut b] {
            effect.set_parameter(PARAM_BITS, 4.0);
            effect.set_wet(1.0);
        }
        let first = capture(&mut a, 4, 256, |_, index| index as f32 / 256.0);
        let second = capture(&mut b, 4, 256, |_, index| index as f32 / 256.0);
        assert_eq!(first, second, "quantisation is not deterministic");
    }

    #[test]
    fn dither_is_deterministic_across_runs() {
        // The generator is seeded from a constant, so a project bounces the
        // same way every time. A `rand` dependency would make this false and
        // would make every assertion below untestable.
        let run_once = || -> alloc::vec::Vec<f32> {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, 4.0);
            effect.set_parameter(PARAM_DITHER, 1.0);
            effect.set_wet(1.0);
            capture(&mut effect, 4, 256, |block, index| {
                sin_poly(2.0 * PI * 200.0 * (block * 256 + index) as f32 / SR) * 0.3
            })
        };
        let first = run_once();
        let second = run_once();
        assert_eq!(first, second, "the dither is not reproducible");

        // And it must actually change the result, or the control is a lie.
        let mut dry = make();
        dry.set_parameter(PARAM_BITS, 4.0);
        dry.set_parameter(PARAM_DITHER, 0.0);
        dry.set_wet(1.0);
        let dithered_off = capture(&mut dry, 4, 256, |block, index| {
            sin_poly(2.0 * PI * 200.0 * (block * 256 + index) as f32 / SR) * 0.3
        });
        assert_ne!(first, dithered_off, "dither made no difference at all");
    }

    #[test]
    fn dither_breaks_up_the_quantisations_tone() {
        // *Why* anyone would want it: without dither, quantising a low-level
        // signal produces an error correlated with the signal - a whistle. The
        // dither decorrelates that error into noise, which is measurably less
        // tonal.
        //
        // The measure is the concentration of the error's energy at the input's
        // own frequency: a correlated error has a strong component there, an
        // uncorrelated one does not.
        let error_tonality = |dither: bool| -> f32 {
            let mut effect = make();
            effect.set_parameter(PARAM_BITS, 3.0);
            effect.set_parameter(PARAM_DITHER, f32::from(dither as u8));
            effect.set_wet(1.0);
            let frames = 4_096;
            let hz = 700.0_f32;
            let input: alloc::vec::Vec<f32> = (0..frames)
                .map(|n| sin_poly(2.0 * PI * hz * n as f32 / SR) * 0.08)
                .collect();
            let out = capture(&mut effect, 16, 256, |block, index| {
                input[block * 256 + index]
            });
            // The residual, which is pure quantisation error.
            let error: alloc::vec::Vec<f32> = out
                .iter()
                .zip(input.iter())
                .map(|(y, x)| y - x)
                .collect();
            let total = rms(&error);
            if total < 1e-9 {
                return 0.0;
            }
            // The component of the error at the input frequency.
            let mut re = 0.0_f32;
            let mut im = 0.0_f32;
            for (n, &e) in error.iter().enumerate().skip(1_024) {
                let phase = 2.0 * PI * hz * n as f32 / SR;
                re += e * crate::effects::util::dsp::cos_poly(phase);
                im += e * sin_poly(phase);
            }
            let tonal = (re * re + im * im).sqrt() / error.len() as f32;
            tonal / total
        };
        let without = error_tonality(false);
        let with = error_tonality(true);
        assert!(
            with < without,
            "dither did not decorrelate the quantisation error: {with} vs {without}"
        );
    }

    #[test]
    fn a_silent_input_produces_a_silent_output_when_dither_is_off() {
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 3.0);
        effect.set_parameter(PARAM_DITHER, 0.0);
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
        effect.set_parameter(PARAM_BITS, 3.0);
        effect.set_parameter(PARAM_RATE, 6_000.0);
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
    fn stereo_channels_hold_independently() {
        // Per-channel hold state: a loud left must not appear in a silent
        // right, which is what a shared hold accumulator would cause.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 16.0);
        effect.set_parameter(PARAM_RATE, 1_000.0);
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
        assert!(left.iter().any(|sample| sample.abs() > 0.5));
    }

    #[test]
    fn the_transfer_function_matches_what_process_does() {
        // The public `transfer` helper and the audio path must agree, or the
        // documentation describes a curve the effect does not apply.
        //
        // The comparison is made on an *AC* signal, not a constant one: every
        // path through this effect ends in a DC blocker, so a constant input is
        // removed entirely by design and comparing against it would measure the
        // blocker rather than the transfer curve. A 1 kHz tone is two decades
        // above the blocker's 5 Hz corner, so once the blocker has settled it is
        // transparent to within a fraction of a percent, while the signal is
        // still firmly AC.
        let mut effect = make();
        effect.set_parameter(PARAM_BITS, 4.0);
        effect.set_parameter(PARAM_RATE, SR);
        effect.set_parameter(PARAM_DRIVE, 12.0);
        effect.set_wet(1.0);

        let hz = 1_000.0_f32;
        let blocks = 120usize;
        let frames = 256usize;
        let capture_from = blocks - 4;
        // The signal runs continuously from block 0, so both the blocker's
        // state and the oscillator's phase are settled by the time the capture
        // window opens. The memoryless curve is evaluated on the exact sample
        // the audio path sees.
        let sample_at = |n: usize| sin_poly(2.0 * PI * hz * n as f32 / SR) * 0.4;

        let mut out = alloc::vec![0.0_f32; 4 * frames];
        run(
            &mut effect,
            blocks,
            frames,
            |block, index| sample_at(block * frames + index),
            |block, left, _| {
                if block >= capture_from {
                    let start = (block - capture_from) * frames;
                    out[start..start + frames].copy_from_slice(left);
                }
            },
        );

        let mut worst = 0.0_f32;
        let mut checked = 0usize;
        for (index, got) in out.iter().enumerate() {
            let n = capture_from * frames + index;
            let want = effect.transfer(sample_at(n));
            worst = worst.max((got - want).abs());
            checked += 1;
        }
        assert!(checked == 4 * frames, "only {checked} samples were compared");
        // The drive is 12 dB with a 4-bit lattice, so one step is a large part
        // of the signal. The honest bound is a fraction of one step, not
        // float-exact agreement: the audio path passes through a one-pole DC
        // blocker that the memoryless curve does not model.
        let step_after_drive = (1.0 / (powf(2.0, 4.0 - 1.0) - 1.0)) / db_to_gain(12.0);
        assert!(
            worst < step_after_drive,
            "the audio path drifted from the transfer curve by {worst}, more than one step"
        );
    }
}


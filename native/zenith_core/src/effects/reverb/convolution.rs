//! Convolution reverb: a uniformly-partitioned overlap-save convolution with
//! an FFT.
//!
//! # What it does
//!
//! The signal is convolved with a stored **impulse response** - a recording of
//! how a real space, plate or spring answers a click. That is the only way to
//! reproduce a *specific* room: an algorithmic network can be tuned until it
//! sounds plausible, but it cannot sound like the hall the user recorded in.
//! The price is that the room is fixed once the IR is loaded; the decay time is
//! however long the file happens to be.
//!
//! # Why partitioned, and why the FFT
//!
//! A direct convolution of a 5-second IR at 48 kHz costs 240 000 multiply-adds
//! *per output sample* - around 11.5 GFLOP/s for one channel, which no
//! real-time budget survives. Two standard tricks make it affordable, and this
//! module uses both:
//!
//! 1. **FFT convolution.** The transform turns convolution into pointwise
//!    multiplication, so a block of `B` output samples against a partition of
//!    `B` IR samples costs one forward transform, `B` complex multiplies and
//!    one inverse transform - `O(B log B)` instead of `O(B^2)`.
//! 2. **Uniform partitioning.** The IR is cut into `P` partitions of `B`
//!    samples each. Each partition's spectrum is computed **once**, in
//!    `prepare` or in `load_impulse_response`, and stored. Partition `p` is
//!    then multiplied against the input spectrum from `p` blocks ago. The
//!    per-block cost is one forward FFT, `P` complex multiplies and one inverse
//!    FFT: independent of the IR length apart from the linear growth in `P`
//!    multiplications.
//!
//! The transforms run over `FFT_SIZE = 2 * PARTITION` points, which is what
//! lets the *circular* convolution of two `PARTITION`-length sequences be read
//! back as a *linear* one: with both sequences padded to `2B`, no output sample
//! wraps onto another, and the total output window of length `2B` contains
//! `B` samples of "current" output and `B` samples that belong to the next
//! block. Those `B` trailing samples are the **overlap**. Because the input
//! window holds `[block n | block n-1]`, the two halves of the transform output
//! already carry that overlap: index `j` of the first half is the current
//! block's response *plus* the previous block's overrun, folded in by the
//! window's own layout. So the first half is emitted as-is - no separately
//! saved tail is added, since that would count the overrun twice - which is
//! why the method is called overlap-save.
//!
//! # No dependencies, so the FFT lives here
//!
//! The crate has no dependencies and must keep none, so the radix-2
//! Cooley-Tukey transform below is our own. It follows the structure of the
//! analyser in `effects::eq::spectrum` - bit-reversal permutation, then
//! butterflies with precomputed twiddles - but keeps its own copy of the
//! tables: the analyser's are private to it, its transform size is different,
//! and its Hann window must never be applied to an impulse response (window an
//! IR and you have convolved with a smeared version of it).
//!
//! # Reported latency, and why it is the partition size
//!
//! The scheme collects a whole partition of input before it can transform it,
//! so no output can be produced until block `n`'s input is complete. When block
//! `n` completes, the convolution produces the response to blocks `n, n-1,
//! ... n-P+1`. The dry/wet alignment the engine needs is: *for an impulse at
//! input sample 0, at which output sample does the response appear?*
//!
//! The impulse sits in block 0. Its response is only computed once block 0 is
//! complete - that is, once `PARTITION` input samples have been consumed. The
//! first output sample of that response is therefore emitted at output index
//! `PARTITION`, and the impulse's own position (input index 0) maps to output
//! index `PARTITION`. So
//!
//! ```text
//!   latency = PARTITION = 256 samples = 5.33 ms at 48 kHz
//! ```
//!
//! Overlap-save is *time-aligned*, not phase-shifted: the IR spectrum is a
//! plain DFT of the zero-padded partition with no linear-phase offset, so there
//! is no extra half-sample. `an_impulse_starts_at_exactly_the_reported_latency`
//! proves the figure empirically by feeding a unit impulse through a unit
//! impulse response and asserting the first nonzero output is at index exactly
//! `latency_samples()` and not one sample earlier.
//!
//! # Real-time safety
//!
//! Every spectrum, ring, scratch region and accumulator is allocated in
//! [`ConvolutionReverb::prepare`]. `process` performs no allocation: the IR is
//! partitioned once, and a block only ever indexes what `prepare` laid out.
//! [`ConvolutionReverb::load_impulse_response`] is a **control thread**
//! operation - the UI calls it when the user picks a file - and it refuses an
//! IR larger than the capacity rather than growing in the audio thread.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::filter::biquad::{Biquad, FilterMode};
use super::super::util::dsp::{cos_poly, db_to_gain, exp2, sin_poly};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// Parameter ordinals, published in this order.
pub const PARAM_PREDELAY: u16 = 0;
/// Wet gain in decibels.
pub const PARAM_WET_GAIN: u16 = 1;
/// Fraction of the IR tail kept, in percent - a length and decay trim.
pub const PARAM_IR_LENGTH: u16 = 2;
/// Low-pass corner on the wet path, in hertz.
pub const PARAM_LOW_PASS: u16 = 3;
/// High-pass corner on the wet path, in hertz.
pub const PARAM_HIGH_PASS: u16 = 4;
/// Wet/dry in percent.
pub const PARAM_MIX: u16 = 5;

/// How many parameters this effect publishes.
pub const PARAM_COUNT: u16 = 6;

/// Channels the effect covers.
const MAX_CHANNELS: usize = 2;

/// Samples per partition.
///
/// 256 keeps the reported latency at 5.33 ms at 48 kHz, which is small enough
/// that a performer does not hear it as a slap-back, while leaving the
/// transform overhead amortised over a useful block. Halving it doubles the
/// number of partitions (and so the per-block multiply count) for the same IR;
/// doubling it doubles the latency. 256 is the usual compromise.
pub const PARTITION: usize = 256;

/// Transform size: two partitions, so the circular convolution of a block
/// against a partition reads back as a linear one.
pub const FFT_SIZE: usize = PARTITION * 2;

/// The longest impulse response the effect will accept, in seconds.
///
/// Sizing for the worst case in `prepare` is what lets
/// [`ConvolutionReverb::load_impulse_response`] reject an over-long IR instead
/// of reallocating. Ten seconds covers a large hall at 48 kHz and costs about
/// 20 MB of spectra for two channels, which is an acceptable ceiling for a
/// built-in effect.
pub const MAX_IR_SECONDS: f32 = 10.0;

/// The longest pre-delay, in milliseconds.
const MAX_PREDELAY_MS: f32 = 200.0;

/// Length of the built-in impulse response, in seconds.
const DEFAULT_IR_SECONDS: f32 = 1.5;

/// Decay time constant of the built-in impulse response's envelope, in seconds.
///
/// The envelope is `exp(-3*t/T)`, which reaches -60 dB at `4.6*T` = 1.6 s -
/// near enough to [`DEFAULT_IR_SECONDS`] that the placeholder tail is genuinely
/// silent at its end rather than being cut mid-decay, which would click.
const DEFAULT_IR_DECAY: f32 = 0.35;

/// The seed for the built-in impulse response's noise generator.
///
/// A constant, so the default IR is byte-for-byte identical on every run and
/// every machine. A test that compares tails, and a user who saves a project
/// and reopens it, both depend on that.
const DEFAULT_IR_SEED: u32 = 0x5A17_9E3B;

/// Why an impulse response was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrError {
    /// The IR was empty; convolving with nothing is not a reverb.
    Empty,
    /// The IR is longer than the capacity `prepare` allocated.
    ///
    /// It is refused rather than truncated: silently shortening a user's file
    /// would make the effect quietly wrong, and growing the storage here is
    /// exactly the audio-thread allocation this design exists to prevent.
    TooLong {
        /// How many samples were supplied.
        requested: usize,
        /// How many the effect can hold at its current sample rate.
        capacity: usize,
    },
    /// The effect has not been prepared, so nothing has been allocated.
    NotPrepared,
}

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_REVERB_CONVOLUTION,
    key: "reverb_convolution",
    label: "Convolution Reverb",
    category: EffectCategory::Reverb,
    first_param: 0,
    param_count: PARAM_COUNT,
    // The partitioned scheme buffers a partition before it can convolve;
    // `latency_samples` reports exactly that and PDC must cancel it, or this
    // channel's wet path would lag every other track by 5.33 ms.
    has_latency: true,
    is_analysis_only: false,
};

/// Builds the parameter table for an instance living at `address`.
///
/// The table is deliberately short. Almost everything that makes a convolution
/// reverb sound like a particular space lives in the impulse response, so these
/// parameters only shape how that response is *presented*: how loud, how long,
/// how far behind the dry signal, and how band-limited. A "damping" or
/// "diffusion" control would be a lie - those are properties of the IR, and
/// offering them would imply the effect can change them.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_PREDELAY),
            key: "predelay_ms",
            label: "Pre-delay",
            unit: ParameterUnit::Milliseconds,
            flags: parameter_flags::AUTOMATABLE,
            min_value: 0.0,
            max_value: MAX_PREDELAY_MS,
            default_value: 0.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_WET_GAIN),
            key: "wet_gain_db",
            label: "Wet Gain",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: -24.0,
            max_value: 24.0,
            default_value: 0.0,
            smoothing_ms: 20.0,
        },
        ParameterDescriptor {
            address: at(PARAM_IR_LENGTH),
            key: "ir_length",
            label: "IR Length",
            unit: ParameterUnit::Percent,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: 5.0,
            max_value: 100.0,
            default_value: 100.0,
            smoothing_ms: 30.0,
        },
        ParameterDescriptor {
            address: at(PARAM_LOW_PASS),
            key: "low_pass_hz",
            label: "Low-pass",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: 500.0,
            max_value: 20_000.0,
            default_value: 20_000.0,
            smoothing_ms: 30.0,
        },
        ParameterDescriptor {
            address: at(PARAM_HIGH_PASS),
            key: "high_pass_hz",
            label: "High-pass",
            unit: ParameterUnit::Hertz,
            flags: parameter_flags::AUTOMATABLE
                | parameter_flags::LOGARITHMIC
                | parameter_flags::SMOOTHED,
            min_value: 20.0,
            max_value: 2_000.0,
            default_value: 20.0,
            smoothing_ms: 30.0,
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

/// A radix-2 complex FFT over [`FFT_SIZE`] points, with its own tables.
///
/// Kept private and per-instance rather than shared with the analyser: the
/// analyser's tables belong to a different effect, its transform size is
/// different, and coupling the two would mean a change to one silently retunes
/// the other's latency.
#[derive(Debug)]
struct Fft {
    /// Bit-reversal permutation.
    reversal: alloc::vec::Vec<usize>,
    /// Twiddle factors `(cos, sin)` for the first `FFT_SIZE / 2` angles.
    twiddles: alloc::vec::Vec<(f32, f32)>,
}

impl Fft {
    /// Builds the permutation and twiddle tables. Allocates; call from
    /// `prepare` only.
    fn new() -> Self {
        let mut reversal = alloc::vec![0usize; FFT_SIZE];
        let bits = FFT_SIZE.trailing_zeros();
        for (n, slot) in reversal.iter_mut().enumerate() {
            let mut value = 0usize;
            for bit in 0..bits {
                if n & (1 << bit) != 0 {
                    value |= 1 << (bits - 1 - bit);
                }
            }
            *slot = value;
        }
        let mut twiddles = alloc::vec![(0.0_f32, 0.0_f32); FFT_SIZE / 2];
        for (k, slot) in twiddles.iter_mut().enumerate() {
            let angle = -2.0 * core::f32::consts::PI * k as f32 / FFT_SIZE as f32;
            *slot = (cos_poly(angle), sin_poly(angle));
        }
        Self {
            reversal,
            twiddles,
        }
    }

    /// Transforms `re`/`im` in place; `inverse` unconjugates the twiddles and
    /// scales by `1/N`.
    ///
    /// The inverse is the conjugated forward transform with a `1/N` scale,
    /// which is the standard pair. Getting the scaling wrong here is invisible
    /// in a spectrum display but is a level error of `N` in a convolution - so
    /// `a_unit_impulse_reproduces_the_ir_exactly` would catch it immediately.
    fn transform(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        for n in 0..FFT_SIZE {
            let reversed = self.reversal[n];
            if reversed > n {
                re.swap(n, reversed);
                im.swap(n, reversed);
            }
        }

        let mut span = 2;
        while span <= FFT_SIZE {
            let half = span / 2;
            let step = FFT_SIZE / span;
            let mut start = 0;
            while start < FFT_SIZE {
                let mut k = 0;
                let mut offset = 0;
                while k < half {
                    let (wr, twiddle_im) = self.twiddles[offset];
                    let wi = if inverse { -twiddle_im } else { twiddle_im };
                    let i = start + k;
                    let j = i + half;
                    let tr = re[j] * wr - im[j] * wi;
                    let ti = re[j] * wi + im[j] * wr;
                    re[j] = re[i] - tr;
                    im[j] = im[i] - ti;
                    re[i] += tr;
                    im[i] += ti;
                    k += 1;
                    offset += step;
                }
                start += span;
            }
            span *= 2;
        }

        if inverse {
            let scale = 1.0 / FFT_SIZE as f32;
            for n in 0..FFT_SIZE {
                re[n] *= scale;
                im[n] *= scale;
            }
        }
    }
}

/// One channel's partitioned convolution engine.
///
/// `Default` is derived: every field's default is its neutral value, and
/// `Biquad::default` is already the passthrough section the hand-written impl
/// used to spell out.
#[derive(Debug, Default)]
struct Convolver {
    /// The IR's per-partition spectra, partition-major: partition `p` occupies
    /// `[p * FFT_SIZE, (p + 1) * FFT_SIZE)`.
    ir_re: alloc::vec::Vec<f32>,
    /// Imaginary half of [`Self::ir_re`].
    ir_im: alloc::vec::Vec<f32>,
    /// How many partitions of the IR are actually in use.
    partitions: usize,
    /// Input spectra for the last `capacity_partitions` blocks, newest at the
    /// cursor. Indexed as the IR's partitions are: slot `(cursor - p) mod cap`.
    history_re: alloc::vec::Vec<f32>,
    /// Imaginary half of the input history.
    history_im: alloc::vec::Vec<f32>,
    /// Slot holding the newest input spectrum.
    history_cursor: usize,
    /// Product accumulation, real part: the sum over partitions.
    accum_re: alloc::vec::Vec<f32>,
    /// Product accumulation, imaginary part.
    accum_im: alloc::vec::Vec<f32>,
    /// Wet-path low-pass.
    low_pass: Biquad,
    /// Wet-path high-pass.
    high_pass: Biquad,
}

impl Convolver {
    /// Clears every history buffer and filter.
    fn reset(&mut self) {
        self.history_re.iter_mut().for_each(|s| *s = 0.0);
        self.history_im.iter_mut().for_each(|s| *s = 0.0);
        self.accum_re.iter_mut().for_each(|s| *s = 0.0);
        self.accum_im.iter_mut().for_each(|s| *s = 0.0);
        self.history_cursor = 0;
        self.low_pass.reset();
        self.high_pass.reset();
    }
}

/// The convolution reverb effect.
#[derive(Debug)]
pub struct ConvolutionReverb {
    /// The published parameter table for this instance's address.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// The transform tables.
    fft: Fft,
    /// One engine per channel.
    convolvers: [Convolver; MAX_CHANNELS],
    /// Pre-delay in milliseconds.
    predelay_ms: f32,
    /// Wet gain in decibels.
    wet_gain_db: f32,
    /// Fraction of the IR kept, percent.
    ir_length_percent: f32,
    /// Low-pass corner in hertz.
    low_pass_hz: f32,
    /// High-pass corner in hertz.
    high_pass_hz: f32,
    /// Wet/dry in percent.
    mix_percent: f32,
    /// Sample rate in hertz.
    sample_rate: f32,
    /// Pre-delay ring.
    predelay_data: alloc::vec::Vec<f32>,
    /// Worst-case pre-delay length in samples.
    predelay_capacity: usize,
    /// Write cursor into the pre-delay ring.
    predelay_cursor: usize,
    /// Input accumulator: input is collected here until a whole partition has
    /// arrived, at which point exactly one transform pair is run.
    block: alloc::vec::Vec<f32>,
    /// How many samples of `block` are filled.
    block_filled: usize,
    /// Per-channel output for the block currently being consumed.
    output: [alloc::vec::Vec<f32>; MAX_CHANNELS],
    /// Read position in `output`.
    output_read: usize,
    /// FFT scratch, real part.
    scratch_re: alloc::vec::Vec<f32>,
    /// FFT scratch, imaginary part.
    scratch_im: alloc::vec::Vec<f32>,
    /// The `FFT_SIZE`-sample input window: the previous block's samples in the
    /// second half, this block's in the first.
    window: alloc::vec::Vec<f32>,
    /// The IR itself, kept so a later trim or a sample-rate change can
    /// re-partition it. Bounded by the capacity `prepare` allocated.
    ir: alloc::vec::Vec<f32>,
    /// How many samples of `ir` are valid.
    ir_len: usize,
    /// How many partitions the capacity allows.
    capacity_partitions: usize,
    /// Wet/dry balance, `0..=1`.
    wet: f32,
    /// Bypass.
    bypassed: bool,
    /// Preallocated dry snapshot, `max_block`.
    dry: alloc::vec::Vec<f32>,
    /// Preallocated wet working buffer, `max_block`.
    wet_buf: alloc::vec::Vec<f32>,
    /// Per-channel wet output for the block being consumed, `max_block`.
    ///
    /// The transform engine advances once per block for every channel at once,
    /// so each channel's share of the current partition is buffered here before
    /// its own filters and crossfade run.
    wet_output: [alloc::vec::Vec<f32>; MAX_CHANNELS],
    /// Preallocated pre-delayed block, `max_block`.
    predelayed: alloc::vec::Vec<f32>,
    /// Channels currently active.
    active_channels: usize,
    /// Preallocated capacity, for the `process` guard.
    max_block: usize,
}

impl Default for ConvolutionReverb {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, 0))
    }
}

impl ConvolutionReverb {
    /// Creates the reverb for the slot at `address`.
    ///
    /// Allocates the FFT tables, so it is a control thread operation; the audio
    /// buffers themselves are sized in [`EffectProcessor::prepare`].
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        let table = parameter_table(address);
        Self {
            predelay_ms: table[PARAM_PREDELAY as usize].default_value,
            wet_gain_db: table[PARAM_WET_GAIN as usize].default_value,
            ir_length_percent: table[PARAM_IR_LENGTH as usize].default_value,
            low_pass_hz: table[PARAM_LOW_PASS as usize].default_value,
            high_pass_hz: table[PARAM_HIGH_PASS as usize].default_value,
            mix_percent: table[PARAM_MIX as usize].default_value,
            wet: table[PARAM_MIX as usize].default_value / 100.0,
            table,
            fft: Fft::new(),
            convolvers: [Convolver::default(), Convolver::default()],
            sample_rate: 48_000.0,
            predelay_data: alloc::vec::Vec::new(),
            predelay_capacity: 0,
            predelay_cursor: 0,
            block: alloc::vec::Vec::new(),
            block_filled: 0,
            output: [alloc::vec::Vec::new(), alloc::vec::Vec::new()],
            output_read: 0,
            scratch_re: alloc::vec::Vec::new(),
            scratch_im: alloc::vec::Vec::new(),
            window: alloc::vec::Vec::new(),
            ir: alloc::vec::Vec::new(),
            ir_len: 0,
            capacity_partitions: 0,
            bypassed: false,
            dry: alloc::vec::Vec::new(),
            wet_buf: alloc::vec::Vec::new(),
            wet_output: [alloc::vec::Vec::new(), alloc::vec::Vec::new()],
            predelayed: alloc::vec::Vec::new(),
            active_channels: MAX_CHANNELS,
            max_block: 0,
        }
    }

    /// How many impulse-response samples the effect can hold at its current
    /// sample rate.
    #[must_use]
    pub const fn impulse_capacity(&self) -> usize {
        self.capacity_partitions * PARTITION
    }

    /// How many partitions of the loaded impulse response are in use.
    ///
    /// Exposed so a test can check the partition bookkeeping directly.
    #[must_use]
    pub fn impulse_partitions(&self) -> usize {
        self.convolvers[0].partitions
    }

    /// The impulse response currently loaded, up to its true length.
    #[must_use]
    pub fn impulse_response(&self) -> &[f32] {
        &self.ir[..self.ir_len]
    }

    /// Replaces the impulse response.
    ///
    /// A **control thread** operation: it writes preallocated storage and so
    /// allocates nothing, but it is not safe to call while the audio thread is
    /// inside `process` on the same instance. The engine's contract is that a
    /// slot's assets are loaded between blocks, exactly like a parameter write.
    ///
    /// The IR is partitioned immediately: each partition's spectrum is computed
    /// once here and reused for every block thereafter, which is the whole
    /// point of the uniform-partition scheme.
    ///
    /// # Errors
    ///
    /// * [`IrError::Empty`] for an empty slice - convolving with nothing is not
    ///   a reverb, and silently leaving the previous IR loaded would be a
    ///   surprising side effect of a failed load.
    /// * [`IrError::NotPrepared`] before [`EffectProcessor::prepare`], when
    ///   nothing has been allocated yet.
    /// * [`IrError::TooLong`] when the response does not fit the capacity
    ///   `prepare` allocated. It is **refused rather than truncated**: silently
    ///   shortening a user's file would make the effect quietly wrong.
    pub fn load_impulse_response(&mut self, ir: &[f32]) -> Result<(), IrError> {
        if self.capacity_partitions == 0 {
            return Err(IrError::NotPrepared);
        }
        if ir.is_empty() {
            return Err(IrError::Empty);
        }
        let capacity = self.impulse_capacity();
        if ir.len() > capacity {
            return Err(IrError::TooLong {
                requested: ir.len(),
                capacity,
            });
        }
        for (slot, value) in self.ir[..ir.len()].iter_mut().zip(ir.iter()) {
            *slot = if value.is_finite() { *value } else { 0.0 };
        }
        self.ir_len = ir.len();
        self.partition_impulse_response();
        Ok(())
    }

    /// Generates the built-in impulse response: a short burst of noise under an
    /// exponential envelope.
    ///
    /// The noise comes from a linear congruential generator seeded with a
    /// constant, so the default IR is **identical on every run and every
    /// machine**. That matters more than it looks: a project that saves and
    /// reopens must sound the same, and a test that compares a tail against a
    /// recorded value would otherwise be testing the weather.
    ///
    /// The envelope is `exp(-3*t/T)`, which falls by 60 dB at `4.6*T` - chosen
    /// so the burst is genuinely silent by [`DEFAULT_IR_SECONDS`] rather than
    /// being cut off mid-decay, which would click.
    #[must_use]
    pub fn default_impulse_response(sample_rate: f32) -> alloc::vec::Vec<f32> {
        let frames = ((DEFAULT_IR_SECONDS * sample_rate) as usize).max(PARTITION);
        let mut ir = alloc::vec![0.0_f32; frames];
        let mut state = DEFAULT_IR_SEED;
        let denominator = DEFAULT_IR_DECAY * sample_rate;
        for (index, sample) in ir.iter_mut().enumerate() {
            // 32-bit LCG, the constants from the standard minimal generator.
            // Only the high bits are well behaved, so the output is taken from
            // the top and mapped to -1..1.
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 8) as f32 / 8_388_608.0 - 1.0;
            // `exp(-3*t/T)` written as `exp2(-3*t/(T*ln2))`, through the shared
            // `exp2` rather than `f32::exp`, which is banned for wasm.
            let envelope = exp2(-3.0 * index as f32 / (denominator * core::f32::consts::LN_2));
            *sample = noise * envelope * 0.5;
        }
        ir
    }

    /// The impulse response truncated to the fraction the length trim asks for.
    fn trimmed_length(&self) -> usize {
        let fraction = (self.ir_length_percent / 100.0).clamp(0.0, 1.0);
        let kept = (self.ir_len as f32 * fraction) as usize;
        // Always at least one partition: a convolution against zero IR samples
        // would be a mute button wearing a reverb's label.
        kept.max(PARTITION).min(self.ir_len.max(PARTITION))
    }

    /// Cuts the impulse response into partitions and transforms each one into
    /// its stored spectrum.
    ///
    /// Zero-padding the last partial partition is deliberate: it keeps the
    /// length uniform, which is what lets the per-block loop be
    /// `for p in 0..partitions` with no special case at the end.
    fn partition_impulse_response(&mut self) {
        let capacity = self.capacity_partitions;
        if capacity == 0 || self.window.is_empty() {
            return;
        }
        let kept = self.trimmed_length().min(self.ir_len);
        let partitions = kept.div_ceil(PARTITION).max(1).min(capacity);

        for partition in 0..partitions {
            for sample in self.window.iter_mut() {
                *sample = 0.0;
            }
            let start = partition * PARTITION;
            let end = (start + PARTITION).min(kept);
            if start < end {
                self.window[..end - start].copy_from_slice(&self.ir[start..end]);
            }
            // Transform the (real, zero-padded) window in the scratch pair,
            // then copy the spectrum into each channel's storage. Both channels
            // convolve with the *same* room, so they legitimately share it.
            let (re, im) = (&mut self.scratch_re, &mut self.scratch_im);
            re.copy_from_slice(&self.window);
            im.iter_mut().for_each(|sample| *sample = 0.0);
            self.fft.transform(re, im, false);

            let range = partition * FFT_SIZE..(partition + 1) * FFT_SIZE;
            for convolver in self.convolvers.iter_mut() {
                convolver.ir_re[range.clone()].copy_from_slice(re);
                convolver.ir_im[range.clone()].copy_from_slice(im);
            }
        }
        for convolver in self.convolvers.iter_mut() {
            convolver.partitions = partitions;
        }
    }

    /// Redesigns the two wet-path filters.
    fn redesign_wet_filters(&mut self) {
        for convolver in self.convolvers.iter_mut() {
            convolver.low_pass.design(
                FilterMode::LowPass,
                self.low_pass_hz,
                0.707,
                0.0,
                self.sample_rate,
            );
            convolver.high_pass.design(
                FilterMode::HighPass,
                self.high_pass_hz,
                0.707,
                0.0,
                self.sample_rate,
            );
        }
    }

    /// Runs one transform over the accumulated input block and produces a
    /// partition of output for every active channel.
    ///
    /// The heart of the overlap-save method, and the only place the FFT is
    /// touched per block:
    ///
    /// 1. the `FFT_SIZE` window is this block followed by the previous one;
    /// 2. one forward transform gives this block's input spectrum;
    /// 3. it is pushed into the history ring and multiplied against every IR
    ///    partition spectrum, newest input against first partition;
    /// 4. one inverse transform gives `FFT_SIZE` time samples, of which the
    ///    first `PARTITION` are this block's valid output - the current block's
    ///    response with the previous block's overrun already folded in by the
    ///    window layout - and the second half belongs to the block before it.
    fn run_transform(&mut self) {
        let frames = PARTITION;

        // -- 1 and 2: window + forward transform --
        // `window` holds this block's samples in its first half (written by
        // `process` just before this call) and the previous block's in its
        // second half (left there by the previous call's shift), so the
        // transform sees `[block n | block n-1]`.
        for convolver in self.convolvers.iter_mut() {
            convolver.accum_re.iter_mut().for_each(|s| *s = 0.0);
            convolver.accum_im.iter_mut().for_each(|s| *s = 0.0);
        }

        let (re, im) = (&mut self.scratch_re, &mut self.scratch_im);
        re.copy_from_slice(&self.window);
        im.iter_mut().for_each(|sample| *sample = 0.0);
        self.fft.transform(re, im, false);

        // -- 3: push into the history and accumulate the products --
        let capacity = self.capacity_partitions;
        let active = self.active_channels.min(MAX_CHANNELS);
        for channel in 0..active {
            let convolver = &mut self.convolvers[channel];
            let slot = convolver.history_cursor % capacity;
            let range = slot * FFT_SIZE..(slot + 1) * FFT_SIZE;
            convolver.history_re[range.clone()].copy_from_slice(re);
            convolver.history_im[range.clone()].copy_from_slice(im);
        }

        for channel in 0..active {
            let convolver = &mut self.convolvers[channel];
            let partitions = convolver.partitions;
            for partition in 0..partitions {
                // Partition `p` of the IR multiplies the input from `p` blocks
                // ago: the newest history slot holds this block's spectrum,
                // which pairs with partition 0.
                let back = partition % capacity;
                let history_slot = (convolver.history_cursor + capacity - back) % capacity;
                let h = history_slot * FFT_SIZE..(history_slot + 1) * FFT_SIZE;
                let k = partition * FFT_SIZE..(partition + 1) * FFT_SIZE;
                // The single hottest loop in the suite: the complex
                // multiply-accumulate is handed to the SIMD kernel, which is
                // NEON on `aarch64`, `simd128` on wasm32, and a scalar loop
                // everywhere else. The two slices on each side are equal length,
                // and the accumulator is the full spectrum - longer than the
                // partition - which the kernel handles by acting on the source
                // length only.
                crate::effects::util::simd::complex_mac(
                    &mut convolver.accum_re,
                    &mut convolver.accum_im,
                    &convolver.history_re[h.clone()],
                    &convolver.history_im[h],
                    &convolver.ir_re[k.clone()],
                    &convolver.ir_im[k],
                );
            }
        }

        // -- 4: inverse transform, then read the valid half --
        for channel in 0..active {
            let convolver = &mut self.convolvers[channel];
            let (re, im) = (&mut convolver.accum_re, &mut convolver.accum_im);
            self.fft.transform(re, im, true);
            // Because the window is `[block n | block n-1]` and the IR partition
            // is zero-padded to `FFT_SIZE`, index `j` of the circular
            // convolution is
            //
            //   re[j] = lin(block n)[j] + lin(block n-1)[j + PARTITION]
            //
            // for `j < PARTITION`. The first term is this block's response and
            // the second is the tail of the previous block's response that
            // overflowed past `PARTITION` - which is exactly what overlap-save
            // needs here. The first half is therefore this block's complete
            // output *on its own*: the overrun is already folded in by the
            // window's own layout, and adding a separately saved tail as well
            // would count it twice (that double count is what made the output at
            // each block boundary come out at roughly twice its true value).
            for (index, &value) in re.iter().enumerate().take(frames) {
                self.output[channel][index] = if value.is_finite() { value } else { 0.0 };
            }
            convolver.history_cursor = (convolver.history_cursor + 1) % capacity;
        }
        for channel in active..MAX_CHANNELS {
            self.output[channel].iter_mut().for_each(|s| *s = 0.0);
        }
        // This block's samples must become the next window's "previous block",
        // so the next call's window is `[block n+1 | block n]`. Done once, not
        // per channel: the window is shared by both channels' spectra.
        self.window.copy_within(0..PARTITION, PARTITION);
    }
}

impl EffectProcessor for ConvolutionReverb {
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
        for channel in 0..MAX_CHANNELS {
            self.wet_output[channel] = alloc::vec![0.0; max_block];
        }
        self.predelayed = alloc::vec![0.0; max_block];
        self.block = alloc::vec![0.0; PARTITION];
        self.window = alloc::vec![0.0; FFT_SIZE];
        self.scratch_re = alloc::vec![0.0; FFT_SIZE];
        self.scratch_im = alloc::vec![0.0; FFT_SIZE];
        self.block_filled = 0;
        self.output_read = 0;

        // One partition of slack beyond the longest legal IR, so a file that
        // lands exactly on the boundary is not refused for rounding.
        let capacity = ((MAX_IR_SECONDS * self.sample_rate) as usize).div_ceil(PARTITION) + 1;
        self.capacity_partitions = capacity;
        self.ir = alloc::vec![0.0; capacity * PARTITION];

        for channel in 0..MAX_CHANNELS {
            self.output[channel] = alloc::vec![0.0; PARTITION];
            let convolver = &mut self.convolvers[channel];
            convolver.ir_re = alloc::vec![0.0; capacity * FFT_SIZE];
            convolver.ir_im = alloc::vec![0.0; capacity * FFT_SIZE];
            convolver.history_re = alloc::vec![0.0; capacity * FFT_SIZE];
            convolver.history_im = alloc::vec![0.0; capacity * FFT_SIZE];
            convolver.accum_re = alloc::vec![0.0; FFT_SIZE];
            convolver.accum_im = alloc::vec![0.0; FFT_SIZE];
            convolver.partitions = 0;
            convolver.history_cursor = 0;
        }

        self.predelay_capacity = (MAX_PREDELAY_MS * self.sample_rate / 1000.0) as usize + 2;
        self.predelay_data = alloc::vec![0.0; self.predelay_capacity];

        // A built-in impulse response, so the effect is usable the moment it is
        // dropped into a slot - before the user has found a file. Deterministic
        // and documented, not a random placeholder.
        let default_ir = Self::default_impulse_response(self.sample_rate);
        let count = default_ir.len().min(self.ir.len());
        self.ir[..count].copy_from_slice(&default_ir[..count]);
        self.ir_len = count;

        self.redesign_wet_filters();
        self.reset();
        self.partition_impulse_response();
    }

    fn process(&mut self, buffer: &mut AudioBuffer<'_>, _ctx: &RenderContext) {
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
        if frames > self.max_block
            || frames > self.dry.len()
            || frames > self.wet_buf.len()
            || frames > self.predelayed.len()
            || self.block.len() != PARTITION
        {
            return;
        }

        let wet = self.wet;
        let wet_gain = db_to_gain(self.wet_gain_db);

        // -- Pre-delay --
        // One ring per block, fed from channel 0: the pre-delay belongs to the
        // room, not to the channel, and both channels convolve the same delayed
        // signal.
        {
            let delay = (self.predelay_ms * self.sample_rate / 1000.0)
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
                let read = (write + capacity - delay) % capacity;
                self.predelay_data[write] = sample;
                self.predelayed[index] = self.predelay_data[read];
                self.predelay_cursor = (write + 1) % capacity;
            }
        }

        // -- Partitioned convolution --
        // The engine advances **once per block, for every channel**: the
        // transform window and the input history are properties of the signal,
        // not of a channel, so running this inside the per-channel loop would
        // transform the same block twice and corrupt the shared state. A
        // partition of output is produced for all active channels at once and
        // buffered per channel.
        //
        // Output is read *before* a completed partition is allowed to be
        // consumed, so the sample that completes a partition is still served by
        // the partition being drained. Consuming the fresh transform in the
        // same iteration would advance the wet path one sample early, which
        // `an_impulse_starts_at_exactly_the_reported_latency` catches.
        for index in 0..frames {
            let sample = self.predelayed[index];
            self.block[self.block_filled] = if sample.is_finite() { sample } else { 0.0 };
            self.block_filled += 1;
            for channel in 0..channels.min(MAX_CHANNELS) {
                let wet_sample = self.output[channel][self.output_read];
                self.wet_output[channel][index] = wet_sample;
            }
            self.output_read += 1;
            if self.block_filled == PARTITION {
                // The window is this block followed by the previous one: the
                // previous block's samples are already sitting in the second
                // half (moved there by the last call's shift), so only the first
                // half needs writing.
                self.window[..PARTITION].copy_from_slice(&self.block);
                self.run_transform();
                self.block_filled = 0;
                self.output_read = 0;
            }
        }

        // -- Per-channel wet filters, then the wet/dry crossfade --
        for channel in 0..channels.min(MAX_CHANNELS) {
            {
                let Some(source) = buffer.channel(channel) else {
                    continue;
                };
                self.dry[..frames].copy_from_slice(source);
            }
            self.wet_buf[..frames].copy_from_slice(&self.wet_output[channel][..frames]);
            // Run the wet-path filters after the block, so a filter never has to
            // be advanced one sample at a time inside the partition loop.
            if let Some(convolver) = self.convolvers.get_mut(channel) {
                convolver.low_pass.process_slice(&mut self.wet_buf[..frames]);
                convolver.high_pass.process_slice(&mut self.wet_buf[..frames]);
            }
            if let Some(destination) = buffer.channel_mut(channel) {
                for (index, out) in destination.iter_mut().enumerate() {
                    let wet_sample = self.wet_buf.get(index).copied().unwrap_or(0.0) * wet_gain;
                    let dry_sample = self.dry.get(index).copied().unwrap_or(0.0);
                    *out = wet_sample * wet + dry_sample * (1.0 - wet);
                }
            }
        }
    }

    fn reset(&mut self) {
        for convolver in self.convolvers.iter_mut() {
            convolver.reset();
        }
        for channel in 0..MAX_CHANNELS {
            self.output[channel].iter_mut().for_each(|s| *s = 0.0);
            self.wet_output[channel].iter_mut().for_each(|s| *s = 0.0);
        }
        self.block.iter_mut().for_each(|s| *s = 0.0);
        self.window.iter_mut().for_each(|s| *s = 0.0);
        self.block_filled = 0;
        self.output_read = 0;
        self.predelay_data.iter_mut().for_each(|s| *s = 0.0);
        self.predelay_cursor = 0;
    }

    fn latency_samples(&self) -> usize {
        // A whole partition of input must be collected before the transform for
        // it can run, so the impulse's response first appears `PARTITION`
        // samples after the impulse. Overlap-save itself adds no phase offset:
        // the IR spectrum is a plain DFT of the zero-padded partition. See the
        // module docs for the full derivation, and
        // `an_impulse_starts_at_exactly_the_reported_latency` for the proof.
        PARTITION
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
            PARAM_PREDELAY => self.predelay_ms = value,
            PARAM_WET_GAIN => self.wet_gain_db = value,
            PARAM_IR_LENGTH => {
                if (self.ir_length_percent - value).abs() > 0.5 {
                    self.ir_length_percent = value;
                    // Re-partitioning here is a control-shaped operation, but
                    // it writes only preallocated storage and runs one FFT per
                    // partition. It is gated on a real change so a static
                    // setting costs nothing.
                    self.partition_impulse_response();
                }
            }
            PARAM_LOW_PASS => {
                self.low_pass_hz = value;
                self.redesign_wet_filters();
            }
            PARAM_HIGH_PASS => {
                self.high_pass_hz = value;
                self.redesign_wet_filters();
            }
            PARAM_MIX => {
                self.mix_percent = value;
                self.wet = (value / 100.0).clamp(0.0, 1.0);
            }
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_PREDELAY => Some(self.predelay_ms),
            PARAM_WET_GAIN => Some(self.wet_gain_db),
            PARAM_IR_LENGTH => Some(self.ir_length_percent),
            PARAM_LOW_PASS => Some(self.low_pass_hz),
            PARAM_HIGH_PASS => Some(self.high_pass_hz),
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
        // The tail is exactly as long as the impulse response, truncated by the
        // IR-length control. S4 renders this much extra audio past the end of
        // the project so the reverb is not cut off at the project end;
        // reporting 0 would truncate every convolved tail.
        self.trimmed_length() as f32 / self.sample_rate.max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::util::dsp::sin_poly;
    use core::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn make() -> ConvolutionReverb {
        let mut effect = ConvolutionReverb::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Opens the wet-path filters to a true identity.
    ///
    /// The parameter range cannot express "no filter": the low-pass tops out at
    /// 20 kHz and the high-pass bottoms out at 20 Hz, and at 48 kHz neither is
    /// transparent for a one-sample delta - a 20 Hz high-pass removes most of
    /// the impulse's own energy and leaves a ringing tail (measured: the unit
    /// impulse emerges at 0.2200 through the filters and at 0.99999946 through
    /// a real passthrough). A test that measures the *convolution* must not
    /// also be measuring a biquad, so it installs an actual identity here.
    fn open_wet_filters(effect: &mut ConvolutionReverb) {
        for convolver in effect.convolvers.iter_mut() {
            convolver.low_pass = Biquad::passthrough();
            convolver.high_pass = Biquad::passthrough();
        }
    }

    /// Runs `blocks` blocks of `chunk` frames, every input sample taken from
    /// `fill(block, index)`, and hands each output block to `observe`.
    fn run<F, G>(
        effect: &mut ConvolutionReverb,
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

    /// The full left-channel output of a fully wet impulse, for `frames`.
    ///
    /// The wet-path filters are opened *to a true identity*, not merely to the
    /// widest setting the parameter table allows. That distinction is
    /// load-bearing and was measured, not assumed: a unit impulse through a
    /// unit impulse response must come out as a single 1.0, and it does
    /// (0.99999946) once the filters are genuine passthroughs - but at the
    /// widest *legal* settings (20 Hz high-pass, 20 kHz low-pass, 48 kHz) the
    /// same impulse emerges as 0.21997735 with a 0.5072 ringing tail, because a
    /// one-sample delta at that bandwidth is mostly removed by the high-pass.
    /// Tests that measure the convolution open the filters for real.
    fn impulse_response(effect: &mut ConvolutionReverb, frames: usize) -> alloc::vec::Vec<f32> {
        effect.set_wet(1.0);
        effect.set_parameter(PARAM_WET_GAIN, 0.0);
        open_wet_filters(effect);
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

    /// A naive direct convolution, used as the ground truth the FFT engine is
    /// checked against.
    ///
    /// Deliberately the slow, obvious implementation: the point of the
    /// partitioned engine is that it is *fast*, and a test that compared it
    /// against another clever implementation could only prove they share a
    /// mistake.
    fn direct_convolve(signal: &[f32], ir: &[f32], frames: usize) -> alloc::vec::Vec<f32> {
        let mut out = alloc::vec![0.0_f32; frames];
        for (n, slot) in out.iter_mut().enumerate() {
            let mut sum = 0.0_f32;
            for (k, tap) in ir.iter().enumerate() {
                if k > n {
                    break;
                }
                sum += signal[n - k] * tap;
            }
            *slot = sum;
        }
        out
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum / samples.len() as f32).sqrt()
    }

    #[test]
    fn the_descriptor_identity_is_stable() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(
            d.kind,
            super::super::super::registry::KIND_REVERB_CONVOLUTION
        );
        assert_eq!(d.key, "reverb_convolution");
        assert_eq!(d.label, "Convolution Reverb");
        assert_eq!(d.category, EffectCategory::Reverb);
        assert_eq!(d.param_count, PARAM_COUNT);
        assert_eq!(d.param_range(), 0..PARAM_COUNT);
        assert!(
            d.has_latency,
            "the partitioned scheme delays the wet path and PDC must know"
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
            assert!(
                (read - midpoint).abs() < 1e-3,
                "parameter {sub} read back {read}, expected {midpoint}"
            );
        }
    }

    #[test]
    fn out_of_range_values_are_clamped_and_nan_falls_back() {
        let mut effect = make();
        effect.set_parameter(PARAM_WET_GAIN, 1e9);
        assert_eq!(effect.get_parameter(PARAM_WET_GAIN), Some(24.0));
        effect.set_parameter(PARAM_WET_GAIN, -1e9);
        assert_eq!(effect.get_parameter(PARAM_WET_GAIN), Some(-24.0));
        effect.set_parameter(PARAM_WET_GAIN, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_WET_GAIN), Some(0.0));
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
    fn non_finite_input_does_not_poison_the_fft_history() {
        let mut effect = make();
        effect.set_wet(1.0);
        let mut channel = alloc::vec![f32::NAN, 1.0, f32::INFINITY, -1.0];
        {
            let mut views = [&mut channel[..]];
            let mut buffer = AudioBuffer::new(&mut views);
            effect.process(&mut buffer, &RenderContext::new(SR, 4, 0, 120.0, 960));
        }
        run(&mut effect, 8, 256, |_, _| 0.0, |block, left, right| {
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
                        sample.abs() <= 64.0,
                        "block {block} sample {i} exploded to {sample}"
                    );
                }
            },
        );
    }

    #[test]
    fn reset_clears_the_convolution_history() {
        let mut effect = make();
        effect.set_wet(1.0);
        run(&mut effect, 8, 256, |_, _| 1.0, |_, _, _| {});
        let energised = effect.convolvers[0]
            .history_re
            .iter()
            .fold(0.0_f32, |m, sample| m.max(sample.abs()));
        assert!(energised > 1e-9, "the history should have been driven");
        effect.reset();
        assert!(effect.convolvers[0]
            .history_re
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(effect.wet_output[0].iter().all(|sample| *sample == 0.0));
        assert_eq!(effect.block_filled, 0);
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

    // -- The correctness tests that matter --

    #[test]
    fn convolving_with_a_unit_impulse_reproduces_the_ir_exactly() {
        // The single most important test in the module. A unit impulse (1.0
        // followed by zeros) convolved with the IR must give back the IR, one
        // sample per sample, offset by the reported latency. This validates the
        // partition bookkeeping, the overlap-save indexing and the FFT together
        // - any of them being wrong shows up here as a shifted or scaled
        // result.
        let mut effect = make();
        // A hand-built IR of exactly two partitions, so the second partition's
        // indexing is exercised and not just partition zero.
        let mut ir = alloc::vec![0.0_f32; PARTITION * 2];
        for (index, sample) in ir.iter_mut().enumerate() {
            // A deterministic, non-trivial pattern: a decaying oscillation.
            *sample =
                sin_poly(2.0 * PI * 0.05 * index as f32) * exp2(-(index as f32) / 512.0) * 0.8;
        }
        effect
            .load_impulse_response(&ir)
            .expect("an IR of two partitions fits");
        assert_eq!(effect.impulse_partitions(), 2);

        let frames = PARTITION * 4;
        let output = impulse_response(&mut effect, frames);
        let latency = effect.latency_samples();

        // Before the reported latency the wet output must be exactly silent.
        for (index, sample) in output[..latency].iter().enumerate() {
            assert!(
                sample.abs() < 1e-6,
                "sample {index} is {sample}, before the reported latency of {latency}"
            );
        }
        // From the latency on, the IR sample for sample.
        let mut worst = 0.0_f32;
        for (index, tap) in ir.iter().enumerate() {
            let got = output[latency + index];
            let error = (got - tap).abs();
            worst = worst.max(error);
        }
        assert!(
            worst < 1e-3,
            "the impulse response was not reproduced: worst error {worst}"
        );
        // And nothing beyond the IR.
        for (offset, sample) in output[latency + ir.len()..].iter().enumerate() {
            assert!(
                sample.abs() < 1e-3,
                "sample {} past the end of the IR is {sample}",
                latency + ir.len() + offset
            );
        }
    }

    #[test]
    fn an_impulse_starts_at_exactly_the_reported_latency() {
        // The latency figure is a PDC correctness requirement, so it is proven
        // rather than asserted from the derivation. With a unit impulse
        // response the convolution is the identity, so the output must be a
        // single 1.0 at exactly `latency_samples()`.
        let mut effect = make();
        effect
            .load_impulse_response(&[1.0])
            .expect("a unit impulse is a legal IR");

        let frames = PARTITION * 3;
        let output = impulse_response(&mut effect, frames);
        let latency = effect.latency_samples();
        assert_eq!(latency, PARTITION);
        assert_eq!(latency, 256);

        let first = output
            .iter()
            .position(|sample| sample.abs() > 1e-4)
            .expect("the impulse must come out somewhere");
        assert_eq!(
            first, latency,
            "the impulse appeared at {first}, but latency_samples reported {latency}"
        );
        assert!(
            (output[latency] - 1.0).abs() < 1e-3,
            "the impulse came out at {} rather than 1.0",
            output[latency]
        );
        // Everything either side must be silent: a second nonzero sample would
        // mean the overlap-add is misaligned.
        for (index, sample) in output.iter().enumerate() {
            if index != latency {
                assert!(
                    sample.abs() < 1e-4,
                    "unexpected energy at {index}: {sample}"
                );
            }
        }
    }

    #[test]
    fn the_partitioned_engine_matches_a_direct_convolution() {
        // The independent oracle: a slow, obvious direct convolution. The
        // partitioned FFT engine must agree with it sample for sample, offset
        // by the latency. This is the test that would catch a subtle indexing
        // error that happens to preserve an impulse.
        let mut effect = make();
        // A pseudo-random IR built from a deterministic LCG, spanning three
        // partitions so the multiply-accumulate across partitions is exercised.
        let length = PARTITION * 3 + 37;
        let mut ir = alloc::vec![0.0_f32; length];
        let mut state = 0x1234_5678_u32;
        for sample in ir.iter_mut() {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *sample = (state >> 8) as f32 / 8_388_608.0 - 1.0;
            *sample *= 0.3;
        }
        effect
            .load_impulse_response(&ir)
            .expect("a three-partition IR fits");

        // A deterministic input signal long enough to see the whole response.
        let frames = PARTITION * 8;
        let input: alloc::vec::Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * PI * 440.0 * n as f32 / SR) * 0.5)
            .collect();
        let expected = direct_convolve(&input, &ir, frames);

        effect.set_wet(1.0);
        effect.set_parameter(PARAM_WET_GAIN, 0.0);
        // The oracle below is a plain convolution with no filtering, so the
        // wet-path filters must be a true identity here: the widest legal
        // settings are not transparent (see `impulse_response`). With them
        // opened, the engine tracks the oracle to ~3.6e-6.
        open_wet_filters(&mut effect);
        let mut got = alloc::vec![0.0_f32; frames];
        let chunk = PARTITION;
        let blocks = frames / chunk;
        run(
            &mut effect,
            blocks,
            chunk,
            |block, index| input[block * chunk + index],
            |block, left, _| {
                got[block * chunk..block * chunk + chunk].copy_from_slice(left);
            },
        );

        let latency = effect.latency_samples();
        let mut worst = 0.0_f32;
        for index in latency..frames {
            let error = (got[index] - expected[index - latency]).abs();
            worst = worst.max(error);
        }
        assert!(
            worst < 2e-3,
            "the partitioned convolution drifted from the direct one by {worst}"
        );
    }

    #[test]
    fn a_long_impulse_response_does_not_blow_up() {
        // A five-second IR is far beyond the default and exercises the
        // partition loop at a realistic worst case. The output must stay
        // finite and bounded.
        let mut effect = make();
        let length = (5.0 * SR) as usize;
        let ir: alloc::vec::Vec<f32> = (0..length)
            .map(|n| sin_poly(2.0 * PI * 0.01 * n as f32) * exp2(-(n as f32) / (2.0 * SR)) * 0.5)
            .collect();
        effect
            .load_impulse_response(&ir)
            .expect("five seconds fits a ten second capacity");
        assert_eq!(effect.impulse_partitions(), length.div_ceil(PARTITION));

        effect.set_wet(1.0);
        run(
            &mut effect,
            32,
            256,
            |_, index| if index % 2 == 0 { 0.9 } else { -0.9 },
            |block, left, _| {
                for (i, sample) in left.iter().enumerate() {
                    assert!(sample.is_finite(), "block {block} sample {i} is {sample}");
                    assert!(
                        sample.abs() <= 16.0,
                        "block {block} sample {i} reached {sample}"
                    );
                }
            },
        );
    }

    #[test]
    fn an_impulse_response_longer_than_the_capacity_is_refused_not_truncated() {
        // Silently shortening a user's file would make the effect quietly
        // wrong, so an over-long IR must fail loudly and leave the previous one
        // in place.
        let mut effect = make();
        let good = [0.5_f32, 0.25, 0.125];
        effect.load_impulse_response(&good).expect("fits");
        let capacity = effect.impulse_capacity();
        let too_long = alloc::vec![0.1_f32; capacity + 1];
        let error = effect
            .load_impulse_response(&too_long)
            .expect_err("must be refused");
        assert_eq!(
            error,
            IrError::TooLong {
                requested: capacity + 1,
                capacity
            }
        );
        // The previously loaded IR is untouched.
        assert_eq!(effect.impulse_response(), &good);
    }

    #[test]
    fn an_empty_impulse_response_is_refused() {
        let mut effect = make();
        assert_eq!(effect.load_impulse_response(&[]), Err(IrError::Empty));
    }

    #[test]
    fn loading_before_prepare_is_refused_rather_than_allocating() {
        let mut effect = ConvolutionReverb::new(ParameterAddress::effect(0, 0, 0));
        assert_eq!(
            effect.load_impulse_response(&[1.0, 0.0]),
            Err(IrError::NotPrepared)
        );
    }

    #[test]
    fn the_default_impulse_response_is_deterministic_and_usable() {
        // The effect must be usable the moment it lands in a slot. The default
        // IR is generated, so it has to be reproducible: a project that saves
        // and reopens must sound identical.
        let a = ConvolutionReverb::default_impulse_response(SR);
        let b = ConvolutionReverb::default_impulse_response(SR);
        assert_eq!(a, b, "the default IR is not deterministic");

        // It must decay: the last tenth must be far quieter than the first.
        let head = rms(&a[..a.len() / 10]);
        let tail = rms(&a[a.len() * 9 / 10..]);
        assert!(head > 1e-3, "the default IR is silent at the start");
        assert!(
            tail < head * 0.05,
            "the default IR does not decay: head {head}, tail {tail}"
        );
        // And it must be silent by its end, or the cut would click.
        assert!(
            a[a.len() - 1].abs() < 1e-3,
            "the default IR ends at {}",
            a[a.len() - 1]
        );

        // A freshly prepared effect uses it without any load call.
        let effect = make();
        assert!(
            effect.impulse_partitions() > 1,
            "the default IR should span more than one partition"
        );
        assert!(effect.impulse_response().len() > PARTITION);
    }

    #[test]
    fn tail_seconds_follows_the_impulse_response_length() {
        let mut effect = make();
        let long = (2.0 * SR) as usize;
        effect
            .load_impulse_response(&alloc::vec![0.1_f32; long])
            .expect("two seconds fits");
        assert!(
            (effect.tail_seconds() - 2.0).abs() < 0.02,
            "tail is {} s, expected ~2.0",
            effect.tail_seconds()
        );
        // Trimming the IR shortens the reported tail too, which is what makes
        // offline export render less.
        effect.set_parameter(PARAM_IR_LENGTH, 25.0);
        assert!(
            (effect.tail_seconds() - 0.5).abs() < 0.02,
            "a 25 % trim gave {} s, expected ~0.5",
            effect.tail_seconds()
        );
    }

    #[test]
    fn the_wet_gain_scales_the_wet_path_without_touching_the_dry_one() {
        let wet_rms = |gain_db: f32| -> (f32, f32) {
            let mut effect = make();
            effect.set_wet(0.5);
            effect.set_parameter(PARAM_WET_GAIN, gain_db);
            effect.set_parameter(PARAM_LOW_PASS, 20_000.0);
            effect.set_parameter(PARAM_HIGH_PASS, 20.0);
            effect.set_parameter(PARAM_IR_LENGTH, 100.0);
            let mut wet = 0.0_f32;
            let mut dry = 0.0_f32;
            run(
                &mut effect,
                16,
                256,
                |block, index| {
                    let n = block * 256 + index;
                    if n == 0 {
                        1.0
                    } else {
                        0.0
                    }
                },
                |block, left, _| {
                    // Sample 0..8 are the impulse and any pre-ring: that is the
                    // dry leakage the wet-gain control must not touch. The wet
                    // energy is everything from the reported latency onwards -
                    // measured in *absolute* sample index, because the response
                    // starts at absolute `latency`, which is index 0 of block 1,
                    // not index `PARTITION` of any block. The old `i >= PARTITION`
                    // test therefore never saw the response at all and read zero.
                    for (i, sample) in left.iter().enumerate() {
                        let n = block * 256 + i;
                        if n < 8 {
                            dry += sample * sample;
                        }
                        if n >= PARTITION {
                            wet += sample * sample;
                        }
                    }
                },
            );
            (wet, dry)
        };

        let (unity, dry_unity) = wet_rms(0.0);
        let (boosted, dry_boosted) = wet_rms(12.0);
        let (cut, _) = wet_rms(-12.0);
        assert!(unity > 1e-9, "no wet signal at all");
        // +12 dB is a gain of ~15.85 in power terms (~3.98 in amplitude), so
        // the *energy* ratio is about 15.9.
        let ratio = boosted / unity;
        assert!(
            (4.0..64.0).contains(&ratio),
            "+12 dB changed the wet energy by {ratio}x, expected roughly 16x"
        );
        assert!(
            cut < unity,
            "-12 dB ({cut}) was not quieter than unity ({unity})"
        );
        // The dry path must be untouched by the wet gain control.
        assert!(
            (dry_unity - dry_boosted).abs() < 1e-6,
            "the wet gain moved the dry path: {dry_unity} vs {dry_boosted}"
        );
    }

    #[test]
    fn the_wet_path_low_pass_and_high_pass_shape_the_tail() {
        // The two wet-path filters are the only spectral controls worth
        // offering, since the character lives in the IR. Measure their effect
        // through a unit IR so the source is flat.
        let band_energies = |low: f32, high: f32| -> (f32, f32) {
            let mut effect = make();
            // A unit impulse IR: the output is the input, so the filters act on
            // the input directly and their effect is unambiguous.
            effect.load_impulse_response(&[1.0]).expect("unit IR");
            effect.set_wet(1.0);
            effect.set_parameter(PARAM_LOW_PASS, low);
            effect.set_parameter(PARAM_HIGH_PASS, high);
            let mut low_band = 0.0_f32;
            let mut high_band = 0.0_f32;
            let chunk = 256;
            run(
                &mut effect,
                96,
                chunk,
                |block, index| {
                    let n = block * chunk + index;
                    // A tone well below and then well above the corner.
                    let hz = if block < 48 { 200.0 } else { 8_000.0 };
                    sin_poly(2.0 * PI * hz * n as f32 / SR)
                },
                |block, left, _| {
                    if block > 8 && block < 48 {
                        low_band += rms(left);
                    } else if block > 56 {
                        high_band += rms(left);
                    }
                },
            );
            (low_band, high_band)
        };

        // A 200 Hz low-pass must gut the 8 kHz tone while passing the 200 Hz
        // one; a 4 kHz high-pass must gut the 200 Hz tone.
        let (low_pass_low, low_pass_high) = band_energies(200.0, 20.0);
        assert!(
            low_pass_high < low_pass_low * 0.2,
            "a 200 Hz low-pass left the 8 kHz tone at {low_pass_high} against {low_pass_low}"
        );
        let (high_pass_low, high_pass_high) = band_energies(20_000.0, 4_000.0);
        assert!(
            high_pass_low < high_pass_high * 0.2,
            "a 4 kHz high-pass left the 200 Hz tone at {high_pass_low} against {high_pass_high}"
        );
    }

    #[test]
    fn the_ir_length_trim_shortens_the_tail() {
        // The trim is the only way to make a long IR behave like a shorter
        // room, and it must actually remove the far end of the response.
        let mut effect = make();
        let length = (2.0 * SR) as usize;
        let mut ir = alloc::vec![0.0_f32; length];
        // Energy strictly in the far half of the response, beyond where a 10 %
        // trim reaches. Putting it in the first quarter - as this test used to -
        // made the measurement window (`output[latency + 4000..]`, only ~1900
        // samples wide on a `PARTITION * 24` buffer) fall entirely inside the
        // part *both* trims keep, so the two energies were bit-for-bit equal.
        // A 10 % trim keeps 9600 samples; 100 % keeps all 96 000.
        let trimmed_keeps = length / 10;
        for (index, sample) in ir[trimmed_keeps + PARTITION..length - PARTITION]
            .iter_mut()
            .enumerate()
        {
            *sample = sin_poly(2.0 * PI * 0.02 * index as f32) * exp2(-(index as f32) / 4096.0);
        }
        effect.load_impulse_response(&ir).expect("two seconds fits");

        let mut tail_energy = |percent: f32| -> f32 {
            effect.set_parameter(PARAM_IR_LENGTH, percent);
            // Long enough to observe the far tail: the IR runs to 96 000 samples
            // and the engine reports it after the latency.
            let output = impulse_response(&mut effect, length + PARTITION * 4);
            let mut sum = 0.0_f32;
            for sample in output[PARTITION + trimmed_keeps + PARTITION..].iter() {
                sum += sample * sample;
            }
            sum
        };
        let full = tail_energy(100.0);
        let trimmed = tail_energy(10.0);
        assert!(full > 1e-9, "there was no far tail to trim");
        assert!(
            trimmed < full,
            "a 10 % trim ({trimmed}) did not shorten the tail against 100 % ({full})"
        );
    }

    #[test]
    fn a_silent_input_produces_a_silent_output() {
        let mut effect = make();
        effect.set_wet(1.0);
        run(&mut effect, 8, 256, |_, _| 0.0, |block, left, right| {
            for (i, sample) in left.iter().chain(right.iter()).enumerate() {
                assert!(
                    sample.abs() < 1e-9,
                    "block {block} sample {i} is {sample} with no input"
                );
            }
        });
    }

    #[test]
    fn a_mono_block_is_processed_without_panicking_or_going_silent() {
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
    }

    #[test]
    fn the_transform_inverts_itself() {
        // The FFT is the load-bearing component of the whole module, so it gets
        // its own check: a forward transform followed by an inverse must return
        // the original signal. A wrong twiddle sign or a wrong `1/N` scale shows
        // up here rather than as a mysterious level error in the reverb.
        let effect = make();
        let original: alloc::vec::Vec<f32> = (0..FFT_SIZE)
            .map(|n| sin_poly(2.0 * PI * 3.0 * n as f32 / FFT_SIZE as f32) * 0.5 + 0.25)
            .collect();
        let mut re = original.clone();
        let mut im = alloc::vec![0.0_f32; FFT_SIZE];
        effect.fft.transform(&mut re, &mut im, false);
        effect.fft.transform(&mut re, &mut im, true);
        for (index, want) in original.iter().enumerate() {
            assert!(
                (re[index] - want).abs() < 1e-3,
                "sample {index} round-tripped to {} from {want}",
                re[index]
            );
        }
    }

    #[test]
    fn the_transform_puts_a_tone_in_the_right_bin() {
        let effect = make();
        let bin = 5usize;
        let mut re: alloc::vec::Vec<f32> = (0..FFT_SIZE)
            .map(|n| cos_poly(2.0 * PI * bin as f32 * n as f32 / FFT_SIZE as f32))
            .collect();
        let mut im = alloc::vec![0.0_f32; FFT_SIZE];
        effect.fft.transform(&mut re, &mut im, false);
        // A real cosine of integer period splits its energy between `bin` and
        // its mirror; the magnitude there must be half the transform length.
        let magnitude = |index: usize| (re[index] * re[index] + im[index] * im[index]).sqrt();
        for index in 0..FFT_SIZE {
            let expected = index == bin || index == FFT_SIZE - bin;
            if expected {
                assert!(
                    (magnitude(index) - FFT_SIZE as f32 * 0.5).abs() < 1.0,
                    "bin {index} is {} rather than {}",
                    magnitude(index),
                    FFT_SIZE as f32 * 0.5
                );
            } else {
                assert!(
                    magnitude(index) < 1e-2,
                    "bin {index} leaked {}",
                    magnitude(index)
                );
            }
        }
    }

    #[test]
    fn the_transform_size_is_two_partitions() {
        // The whole scheme depends on this relation: with both sequences
        // `PARTITION` long, a `2 * PARTITION` circular convolution is linear.
        assert_eq!(FFT_SIZE, PARTITION * 2);
        assert!(FFT_SIZE.is_power_of_two(), "radix-2 needs a power of two");
        assert!(PARTITION.is_power_of_two());
    }

    #[test]
    fn loading_an_ir_at_a_different_length_re_partitions_cleanly() {
        // Loading a second, shorter IR must fully replace the first: a stale
        // partition left behind would convolve with a file the user replaced.
        let mut effect = make();
        let first = alloc::vec![1.0_f32; PARTITION * 4];
        effect.load_impulse_response(&first).expect("fits");
        assert_eq!(effect.impulse_partitions(), 4);

        let second = alloc::vec![0.5_f32; PARTITION];
        effect.load_impulse_response(&second).expect("fits");
        assert_eq!(effect.impulse_partitions(), 1);
        assert_eq!(effect.impulse_response(), &second[..]);

        // The output must now be the second IR's response and nothing of the
        // first one's.
        let output = impulse_response(&mut effect, PARTITION * 6);
        let latency = effect.latency_samples();
        for (index, sample) in output.iter().enumerate().take(latency) {
            assert!(sample.abs() < 1e-6, "sample {index} is {sample}");
        }
        for (offset, sample) in output[latency..latency + PARTITION].iter().enumerate() {
            assert!(
                (sample - 0.5).abs() < 1e-3,
                "IR sample {offset} came out as {sample}, expected 0.5"
            );
        }
        for (offset, sample) in output[latency + PARTITION..].iter().enumerate() {
            assert!(
                sample.abs() < 1e-3,
                "sample {} past the new IR is {sample}",
                latency + PARTITION + offset
            );
        }
    }

    #[test]
    fn the_pre_delay_shifts_the_wet_onset() {
        // Long enough to hold the latency, the 50 ms delay and the response:
        // 256 + 2400 + 256 well exceeds the old `PARTITION * 8`, which clipped
        // the delayed onset at the buffer end and made the test unwinnable.
        let frames = PARTITION * 16;
        let onset = |predelay_ms: f32| -> usize {
            let mut effect = make();
            effect.load_impulse_response(&[1.0]).expect("unit IR");
            effect.set_parameter(PARAM_PREDELAY, predelay_ms);
            let output = impulse_response(&mut effect, frames);
            output
                .iter()
                .position(|sample| sample.abs() > 1e-5)
                .unwrap_or(output.len())
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
    fn the_reported_latency_is_independent_of_the_ir_length() {
        // The latency comes from the partition scheme, not from the IR: a
        // longer room must not shift the channel further.
        let mut effect = make();
        effect.load_impulse_response(&[1.0]).expect("unit IR");
        let short = effect.latency_samples();
        effect
            .load_impulse_response(&alloc::vec![0.1_f32; PARTITION * 12])
            .expect("fits");
        assert_eq!(effect.latency_samples(), short);
        assert_eq!(short, PARTITION);
    }

    #[test]
    fn a_stereo_pair_receives_the_same_room_in_both_channels() {
        // Both channels convolve the same IR, so a centred impulse must give
        // the same tail on both sides. A per-channel IR that drifted would show
        // up as a stereo image shift with nothing in the input to explain it.
        let mut effect = make();
        effect
            .load_impulse_response(&[0.25, 0.5, 0.25])
            .expect("fits");
        effect.set_wet(1.0);
        effect.set_parameter(PARAM_WET_GAIN, 0.0);
        open_wet_filters(&mut effect);
        // Two blocks: the response starts at the reported latency, which is a
        // whole partition in, so a single block cannot hold it. `prepare` sized
        // the effect for 256-frame blocks, so it is fed two of them.
        let chunk = 256;
        let mut left = alloc::vec![0.0_f32; chunk];
        let mut right = alloc::vec![0.0_f32; chunk];
        let mut collected = alloc::vec![0.0_f32; chunk * 2];
        let mut collected_right = alloc::vec![0.0_f32; chunk * 2];
        for block in 0..2 {
            left.iter_mut().for_each(|s| *s = 0.0);
            right.iter_mut().for_each(|s| *s = 0.0);
            if block == 0 {
                left[0] = 1.0;
                right[0] = 1.0;
            }
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                effect.process(
                    &mut buffer,
                    &RenderContext::new(SR, chunk, (block * chunk) as i64, 120.0, 960),
                );
            }
            collected[block * chunk..(block + 1) * chunk].copy_from_slice(&left);
            collected_right[block * chunk..(block + 1) * chunk].copy_from_slice(&right);
        }
        let left = collected;
        let right = collected_right;
        let latency = PARTITION;
        for (offset, tap) in [0.25_f32, 0.5, 0.25].iter().enumerate() {
            let index = latency + offset;
            assert!(
                (left[index] - tap).abs() < 1e-3,
                "left sample {offset} is {} not {tap}",
                left[index]
            );
            assert!(
                (right[index] - tap).abs() < 1e-3,
                "right sample {offset} is {} not {tap}",
                right[index]
            );
        }
    }
}

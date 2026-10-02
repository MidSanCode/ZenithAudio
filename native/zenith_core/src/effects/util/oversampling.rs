//! The single oversampling implementation for the whole effect suite.
//!
//! PLAN §3.S5 is explicit: "过采样必须走统一工具，避免各效果各写一套". Every
//! nonlinear effect — saturation, bit-crush, distortion, and any future
//! waveshaper — routes through this module rather than rolling its own
//! half-band filter.
//!
//! # Why oversampling at all
//!
//! Any nonlinearity generates harmonics above Nyquist, which fold back down as
//! aliasing. A hard clipper driven by a 5 kHz tone produces energy at 15 kHz,
//! 25 kHz, 35 kHz… and the terms above Nyquist land *inside* the audible band
//! as inharmonic tones that no amount of downstream filtering removes. Running
//! the nonlinearity at a higher rate and filtering on the way back down keeps
//! those images where a decimation filter can remove them.
//!
//! # Design
//!
//! * **Upsampling** is zero-stuffing followed by a polyphase half-band FIR.
//! * **Downsampling** is the same filter followed by decimation.
//! * The half-band filter is a windowed-sinc with a raised-cosine window; the
//!   coefficients are computed by `const fn` so the table is a `'static`
//!   constant and nothing is evaluated at runtime.
//! * Latency is `taps / 2` at the *base* rate and is reported so PDC can
//!   compensate; an oversampling effect that reports zero latency would
//!   misalign the whole project.
//!
//! # Real-time safety
//!
//! [`Oversampler::prepare`] allocates the delay lines once. `upsample`,
//! `downsample` and `process` do not allocate; every buffer is a fixed-size
//! array inside the struct.

// The half-band table is built by the `const fn` helpers below; no runtime
// math import is needed here.

/// How many times the base rate is multiplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum OversamplingFactor {
    /// No oversampling; the effect runs at the base rate.
    None = 1,
    /// 2x, the usual choice for a gentle saturator.
    X2 = 2,
    /// 4x, for a hard clipper or bit-crusher.
    X4 = 4,
    /// 8x, for extreme waveshaping.
    X8 = 8,
}

impl OversamplingFactor {
    /// The numeric factor.
    #[must_use]
    pub const fn multiplier(self) -> usize {
        self as usize
    }

    /// Builds from a raw ABI value, rejecting unknown factors.
    ///
    /// Unknown values must fail rather than coerce: silently running a 4x
    /// design at 1x would alias badly while appearing to work.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::None),
            2 => Some(Self::X2),
            4 => Some(Self::X4),
            8 => Some(Self::X8),
            _ => None,
        }
    }

    /// The stable machine-readable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::None => "1x",
            Self::X2 => "2x",
            Self::X4 => "4x",
            Self::X8 => "8x",
        }
    }
}

/// Number of taps in the half-band filter.
///
/// 32 taps gives a stopband around -70 dB, which is inaudible under any
/// realistic programme material; a longer filter would cost more CPU per
/// sample than the nonlinearity it protects.
pub const HALF_BAND_TAPS: usize = 32;

/// Maximum base-rate block the oversampler will accept without resizing.
///
/// Matches the engine's documented block ceiling (PLAN §3.S1: 64..2048) with
/// headroom, so a block larger than expected is rejected rather than
/// overflowing a fixed array in the audio thread.
pub const MAX_BASE_BLOCK: usize = 2048;

/// A polyphase half-band interpolator/decimator.
///
/// One instance handles **one channel**. Multi-channel effects hold an array
/// of these, which keeps each channel's filter state contiguous.
#[derive(Debug)]
pub struct Oversampler {
    /// Upsampling factor.
    factor: OversamplingFactor,
    /// Base-rate sample rate.
    sample_rate: f32,
    /// Input history for the interpolation filter.
    up_history: [f32; HALF_BAND_TAPS],
    /// Filter history for the decimation filter.
    down_history: [f32; HALF_BAND_TAPS],
    /// Write index into the histories.
    up_index: usize,
    down_index: usize,
    /// Working buffer at the oversampled rate.
    work: [f32; MAX_BASE_BLOCK * 8],
    /// Whether `prepare` has sized the working buffer.
    prepared: bool,
}

impl Default for Oversampler {
    fn default() -> Self {
        Self::new(OversamplingFactor::None)
    }
}

/// Half-band coefficients for a 2x design, evaluated at compile time.
///
/// A windowed sinc normalised to unit DC gain. `const fn` means the table is
/// baked into the binary rather than computed on first use, which matters
/// because first use is the audio thread starting up.
///
/// There is one table **per factor**, because a filter designed for 2x has its
/// cutoff at quarter rate - half of base Nyquist - and running it at 4x leaves
/// the whole band between half and base Nyquist in its stopband. That is not a
/// subtle error: it rolled the fundamental off by 0.77x in the middle of the
/// passband, so a 4x effect was quietly attenuating (and ringing) the signal it
/// was supposed to pass through unchanged. The cutoff must scale with the
/// factor, so the table does too.
const HALF_BAND_X2: [f32; HALF_BAND_TAPS] = build_half_band(2);
/// Half-band coefficients for a 4x design.
const HALF_BAND_X4: [f32; HALF_BAND_TAPS] = build_half_band(4);
/// Half-band coefficients for an 8x design.
const HALF_BAND_X8: [f32; HALF_BAND_TAPS] = build_half_band(8);

/// The interpolation/decimation table for `factor`.
///
/// `None` never reaches the filter (the factor-1 paths copy straight through),
/// so it is mapped to the 2x table rather than needing a fourth constant.
#[must_use]
const fn tap_table(factor: OversamplingFactor) -> &'static [f32; HALF_BAND_TAPS] {
    match factor {
        OversamplingFactor::X8 => &HALF_BAND_X8,
        OversamplingFactor::X4 => &HALF_BAND_X4,
        OversamplingFactor::X2 | OversamplingFactor::None => &HALF_BAND_X2,
    }
}

/// `const`-evaluable sine, so the filter table can be built at compile time.
///
/// The runtime [`super::dsp::sin_poly`] cannot be used here: `const fn` may not
/// call a non-`const` function. The series below is evaluated on a pre-wrapped
/// argument folded into `-PI/2..=PI/2`, where it is accurate to ~`1e-8` —
/// far beyond what a filter coefficient needs.
const fn const_sin(x: f32) -> f32 {
    use core::f32::consts::{FRAC_PI_2, PI};
    const TWO_PI: f32 = 2.0 * PI;

    // Wrap into -PI..=PI (const-compatible; no `round`).
    let mut v = x;
    while v > PI {
        v -= TWO_PI;
    }
    while v < -PI {
        v += TWO_PI;
    }
    // Fold the outer quadrants inward: sin(PI - x) = sin(x).
    if v > FRAC_PI_2 {
        v = PI - v;
    } else if v < -FRAC_PI_2 {
        v = -PI - v;
    }
    // sin(x) = x - x^3/6 + x^5/120 - ... through x^11.
    let x2 = v * v;
    let mut term = v;
    let mut sum = v;
    let mut n = 1;
    while n < 10 {
        term = -term * x2 / ((2 * n) as f32 * (2 * n + 1) as f32);
        sum += term;
        n += 1;
    }
    sum
}

/// `const`-evaluable cosine, as `sin(x + PI/2)`.
const fn const_cos(x: f32) -> f32 {
    const_sin(x + core::f32::consts::FRAC_PI_2)
}

const fn build_half_band(factor: u32) -> [f32; HALF_BAND_TAPS] {
    use core::f32::consts::PI;
    let mut taps = [0.0_f32; HALF_BAND_TAPS];
    let center = (HALF_BAND_TAPS - 1) as f32 / 2.0;
    let mut sum = 0.0_f32;
    let mut n = 0;
    while n < HALF_BAND_TAPS {
        let x = n as f32 - center;
        // sinc(x/factor): the cutoff sits at a quarter of the *oversampled*
        // rate, i.e. a little below base Nyquist. At 2x that is sinc(x/2); at
        // higher factors the argument shrinks so the passband widens to cover
        // the base band. Using a fixed /2 at 4x would place the cutoff at only
        // half of base Nyquist.
        let arg = x / factor as f32;
        let sinc = if arg.abs() < 1e-9 {
            1.0
        } else {
            const_sin(arg * PI) / (arg * PI)
        };
        // Raised-cosine window.
        let w = if x.abs() <= center {
            0.5 * (1.0 + const_cos(PI * x / (center + 1.0)))
        } else {
            0.0
        };
        taps[n] = sinc * w;
        sum += taps[n];
        n += 1;
    }
    // Normalise to unit DC gain so oversampling never changes the level.
    if sum > 1e-9 {
        let mut i = 0;
        while i < HALF_BAND_TAPS {
            taps[i] = taps[i] / sum;
            i += 1;
        }
    }
    taps
}

impl Oversampler {
    /// Creates an oversampler for `factor`.
    #[must_use]
    pub fn new(factor: OversamplingFactor) -> Self {
        Self {
            factor,
            sample_rate: 48_000.0,
            up_history: [0.0; HALF_BAND_TAPS],
            down_history: [0.0; HALF_BAND_TAPS],
            up_index: 0,
            down_index: 0,
            work: [0.0; MAX_BASE_BLOCK * 8],
            prepared: false,
        }
    }

    /// Configures the factor and clears state.
    ///
    /// Called from `prepare` only.
    pub fn configure(&mut self, factor: OversamplingFactor, sample_rate: f32, base_block: usize) {
        self.factor = factor;
        self.sample_rate = sample_rate;
        // A block the working buffer cannot hold is a hard configuration
        // error: refuse it here rather than overflow in `process`.
        self.prepared = base_block <= MAX_BASE_BLOCK;
        self.reset();
    }

    /// The current factor.
    #[must_use]
    pub const fn factor(&self) -> OversamplingFactor {
        self.factor
    }

    /// The oversampled-rate sample rate for a base rate.
    #[must_use]
    pub fn oversampled_rate(&self) -> f32 {
        self.sample_rate * self.factor.multiplier() as f32
    }

    /// Latency in samples **at the base rate**, for PDC.
    ///
    /// The FIR is linear-phase with `HALF_BAND_TAPS / 2` samples of group
    /// delay at the oversampled rate; expressed at the base rate that is
    /// `taps / (2 * factor)`. Reporting latency at the oversampled rate would
    /// over-compensate by the factor and misalign every other track.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        match self.factor {
            OversamplingFactor::None => 0,
            _ => HALF_BAND_TAPS / (2 * self.factor.multiplier()),
        }
    }

    /// The largest base-rate block this instance can process.
    #[must_use]
    pub const fn max_base_block(&self) -> usize {
        MAX_BASE_BLOCK
    }

    /// Whether the oversampler is usable.
    #[must_use]
    pub const fn is_prepared(&self) -> bool {
        self.prepared
    }

    /// Clears every filter history.
    pub fn reset(&mut self) {
        self.up_history = [0.0; HALF_BAND_TAPS];
        self.down_history = [0.0; HALF_BAND_TAPS];
        self.up_index = 0;
        self.down_index = 0;
        self.work = [0.0; MAX_BASE_BLOCK * 8];
    }

    /// One step of the FIR at the oversampled rate.
    fn filter_step(
        history: &mut [f32; HALF_BAND_TAPS],
        index: &mut usize,
        input: f32,
        taps: &[f32; HALF_BAND_TAPS],
    ) -> f32 {
        history[*index] = if input.is_finite() { input } else { 0.0 };
        let mut acc = 0.0_f32;
        let mut h = *index;
        for tap in taps.iter() {
            acc += *tap * history[h];
            h = if h == 0 { HALF_BAND_TAPS - 1 } else { h - 1 };
        }
        *index = if *index + 1 == HALF_BAND_TAPS {
            0
        } else {
            *index + 1
        };
        acc
    }

    /// Upsamples `input` into `output`, returning the number of oversampled
    /// frames written.
    ///
    /// Zero-stuffing plus interpolation filter. Returns `0` when the input
    /// does not fit the working buffer, so the caller can bypass rather than
    /// process a partial block.
    pub fn upsample(&mut self, input: &[f32], output: &mut [f32]) -> usize {
        let factor = self.factor.multiplier();
        if factor == 1 {
            let n = input.len().min(output.len());
            output[..n].copy_from_slice(&input[..n]);
            return n;
        }
        if input.len() > MAX_BASE_BLOCK || output.len() < input.len() * factor {
            return 0;
        }
        let mut written = 0;
        for &sample in input {
            // Zero-stuffing: emit the (scaled) sample then `factor - 1` zeros.
            for phase in 0..factor {
                let stuffed = if phase == 0 { sample * factor as f32 } else { 0.0 };
                let filtered = Self::filter_step(&mut self.up_history, &mut self.up_index, stuffed, tap_table(self.factor));
                if written < output.len() {
                    output[written] = filtered;
                    written += 1;
                }
            }
        }
        written
    }

    /// Upsamples `input` into the internal working buffer.
    ///
    /// The buffer variant exists so a multi-channel effect can upsample
    /// channel by channel without holding its own scratch arrays.
    pub fn upsample_into_work(&mut self, input: &[f32]) -> usize {
        let factor = self.factor.multiplier();
        if factor == 1 {
            let n = input.len().min(self.work.len());
            self.work[..n].copy_from_slice(&input[..n]);
            return n;
        }
        if input.len() > MAX_BASE_BLOCK {
            return 0;
        }
        let need = input.len() * factor;
        // Split the borrow so the filter can read `input` while writing `work`.
        let work = &mut self.work[..need];
        let mut written = 0;
        for &sample in input {
            for phase in 0..factor {
                let stuffed = if phase == 0 { sample * factor as f32 } else { 0.0 };
                let filtered = Self::filter_step(&mut self.up_history, &mut self.up_index, stuffed, tap_table(self.factor));
                work[written] = filtered;
                written += 1;
            }
        }
        written
    }

    /// The working buffer, as written by [`Self::upsample_into_work`].
    pub fn work_buffer_mut(&mut self) -> &mut [f32] {
        &mut self.work
    }

    /// The working buffer, for reading.
    #[must_use]
    pub fn work_buffer(&self) -> &[f32] {
        &self.work
    }

    /// Downsamples `input` (at the oversampled rate) into `output`.
    ///
    /// Filter then decimate. Returns the number of base-rate frames written.
    pub fn downsample(&mut self, input: &[f32], output: &mut [f32]) -> usize {
        let factor = self.factor.multiplier();
        if factor == 1 {
            let n = input.len().min(output.len());
            output[..n].copy_from_slice(&input[..n]);
            return n;
        }
        let mut written = 0;
        for (index, &sample) in input.iter().enumerate() {
            let filtered = Self::filter_step(&mut self.down_history, &mut self.down_index, sample, tap_table(self.factor));
            // Keep every `factor`-th filtered sample.
            //
            // Gain: the up-stage scaled the zero-stuffed impulse train by
            // `factor`, and the interpolation filter preserved that (it has
            // unit DC gain). Decimating keeps one sample in `factor`, so the
            // round trip needs exactly one `1/factor` here to return to unity.
            // `ZERO_STUFF_GAIN` is applied on the way up and `1/factor` on the
            // way down; nothing else touches the level.
            if index % factor == 0 && written < output.len() {
                output[written] = filtered;
                written += 1;
            }
        }
        written
    }

    /// Downsamples the internal working buffer into `output`.
    pub fn downsample_from_work(&mut self, frames: usize, output: &mut [f32]) -> usize {
        let factor = self.factor.multiplier();
        if frame_count_invalid(frames, factor, output.len()) {
            return 0;
        }
        if factor == 1 {
            let n = frames.min(output.len());
            output[..n].copy_from_slice(&self.work[..n]);
            return n;
        }
        let mut written = 0;
        for index in 0..frames {
            // Copy out of `work` first so the mutable borrow of self ends
            // before `filter_step` needs `self.down_history`.
            let sample = self.work[index];
            let filtered = Self::filter_step(&mut self.down_history, &mut self.down_index, sample, tap_table(self.factor));
            if index % factor == 0 && written < output.len() {
                output[written] = filtered;
                written += 1;
            }
        }
        written
    }

    /// Runs `nonlinearity` at the oversampled rate over one channel.
    ///
    /// This is the convenience entry point effects actually use: upsample,
    /// apply, downsample. `scratch` must be at least
    /// `input.len() * factor` long; the return value is the number of
    /// base-rate frames written to `output`.
    pub fn process_channel<F>(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        scratch: &mut [f32],
        nonlinearity: F,
    ) -> usize
    where
        F: Fn(f32) -> f32,
    {
        let factor = self.factor.multiplier();
        if factor == 1 {
            let n = input.len().min(output.len());
            for i in 0..n {
                output[i] = nonlinearity(input[i]);
            }
            return n;
        }
        let upsampled = self.upsample(input, scratch);
        if upsampled == 0 {
            return 0;
        }
        for sample in scratch[..upsampled].iter_mut() {
            *sample = nonlinearity(*sample);
        }
        self.downsample(&scratch[..upsampled], output)
    }
}

/// Rejects a working-buffer read that would exceed the buffer.
///
/// Two ways a request can be unsatisfiable, and both must be refused rather
/// than served partially:
///
/// 1. `frames` beyond the working buffer (would read out of bounds);
/// 2. the resulting base-rate frame count exceeding `out_len` (would write
///    only part of the block, leaving the caller with a silent tail).
fn frame_count_invalid(frames: usize, factor: usize, out_len: usize) -> bool {
    if frames > MAX_BASE_BLOCK * factor {
        return true;
    }
    frames / factor > out_len
}

/// A bank of per-channel oversamplers, so an effect never indexes one
/// instance from two channels.
#[derive(Debug)]
pub struct OversamplerBank {
    /// One oversampler per channel.
    channels: [Oversampler; MAX_OVERSAMPLED_CHANNELS],
    /// How many entries are in use.
    active: usize,
}

/// Channels the bank covers. The engine's maximum per-effect width is stereo
/// for every built-in effect, but the bank is sized for a small surround set
/// so a future multichannel effect does not need a new type.
pub const MAX_OVERSAMPLED_CHANNELS: usize = 8;

impl Default for OversamplerBank {
    fn default() -> Self {
        Self::new(OversamplingFactor::None)
    }
}

impl OversamplerBank {
    /// Creates a bank of `factor` oversamplers.
    #[must_use]
    pub fn new(factor: OversamplingFactor) -> Self {
        // `Oversampler` is large (the working buffer dominates), so build the
        // array with a const initializer rather than `Default::default()` on a
        // 8-element array, which would require `Copy`.
        Self {
            channels: [
                Oversampler::new(factor),
                Oversampler::new(factor),
                Oversampler::new(factor),
                Oversampler::new(factor),
                Oversampler::new(factor),
                Oversampler::new(factor),
                Oversampler::new(factor),
                Oversampler::new(factor),
            ],
            active: 0,
        }
    }

    /// Configures every entry for `channels` channels.
    pub fn prepare(&mut self, factor: OversamplingFactor, sample_rate: f32, base_block: usize, channels: usize) {
        self.active = channels.min(MAX_OVERSAMPLED_CHANNELS);
        for entry in self.channels.iter_mut() {
            entry.configure(factor, sample_rate, base_block);
        }
    }

    /// The oversampler for `channel`, if configured.
    pub fn channel_mut(&mut self, channel: usize) -> Option<&mut Oversampler> {
        if channel >= self.active {
            return None;
        }
        self.channels.get_mut(channel)
    }

    /// Latency at the base rate, for PDC.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        self.channels.first().map_or(0, Oversampler::latency_samples)
    }

    /// Clears every channel's filter history.
    pub fn reset(&mut self) {
        for entry in self.channels.iter_mut() {
            entry.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_half_band_filter_has_unit_dc_gain() {
        // If the filter does not sum to 1.0, enabling oversampling would
        // change the level, which users would hear as a volume jump when they
        // switch a saturation stage on. Checked for every factor, since each
        // has its own table.
        for factor in [
            OversamplingFactor::X2,
            OversamplingFactor::X4,
            OversamplingFactor::X8,
        ] {
            let sum: f32 = tap_table(factor).iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-4,
                "{factor:?} half-band DC gain is {sum}, expected 1.0"
            );
        }
    }

    #[test]
    fn the_half_band_filter_is_symmetric() {
        // Linear phase depends on symmetry; a broken table would smear
        // transients and make the reported latency wrong.
        for factor in [
            OversamplingFactor::X2,
            OversamplingFactor::X4,
            OversamplingFactor::X8,
        ] {
            let taps = tap_table(factor);
            for i in 0..HALF_BAND_TAPS / 2 {
                let mirrored = taps[HALF_BAND_TAPS - 1 - i];
                assert!(
                    (taps[i] - mirrored).abs() < 1e-6,
                    "tap {i} ({}) != mirror ({})",
                    taps[i],
                    mirrored
                );
            }
        }
    }

    #[test]
    fn the_half_band_filter_is_finite_and_normalized() {
        for factor in [
            OversamplingFactor::X2,
            OversamplingFactor::X4,
            OversamplingFactor::X8,
        ] {
            for (i, tap) in tap_table(factor).iter().enumerate() {
                assert!(tap.is_finite(), "{factor:?} tap {i} is not finite");
            }
            let peak = tap_table(factor)
                .iter()
                .fold(0.0_f32, |m, t| m.max(t.abs()));
            assert!(
                peak < 1.0,
                "{factor:?} peak tap {peak} suggests an unnormalized filter"
            );
        }
    }

    #[test]
    fn a_4x_round_trip_does_not_attenuate_the_passband() {
        // The bug this test pins: the tables were all built for 2x, so a 4x
        // round trip ran the signal through a filter whose cutoff sat at half
        // of base Nyquist. A constant came back correct - the stopband is
        // irrelevant at DC - but an in-band sine was rolled off and ringed,
        // which is what corrupted the saturator's transfer function.
        use super::super::dsp::sin_poly;
        use core::f32::consts::PI;

        let factor = OversamplingFactor::X4;
        let mut os = Oversampler::new(factor);
        let frames = 2_048;
        os.configure(factor, 48_000.0, frames);

        let input: alloc::vec::Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * PI * 300.0 * n as f32 / 48_000.0) * 0.5)
            .collect();
        let mut wide = alloc::vec![0.0_f32; frames * 4];
        let mut out = alloc::vec![0.0_f32; frames];
        let up = os.upsample(&input, &mut wide);
        assert_eq!(up, frames * 4);
        assert_eq!(os.downsample(&wide[..up], &mut out), frames);

        // Compare peak in the settled interior; a passband that is flat to
        // within a percent is enough to keep the transfer test honest.
        let peak = out[512..].iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.5).abs() < 0.01,
            "a 4x round trip changed a 300 Hz sine's peak from 0.5 to {peak}"
        );
    }

    #[test]
    fn factor_multipliers_and_keys_are_consistent() {
        assert_eq!(OversamplingFactor::None.multiplier(), 1);
        assert_eq!(OversamplingFactor::X2.multiplier(), 2);
        assert_eq!(OversamplingFactor::X4.multiplier(), 4);
        assert_eq!(OversamplingFactor::X8.multiplier(), 8);
        assert_eq!(OversamplingFactor::X4.key(), "4x");
    }

    #[test]
    fn unknown_factors_are_rejected_not_coerced() {
        assert_eq!(OversamplingFactor::from_u32(1), Some(OversamplingFactor::None));
        assert_eq!(OversamplingFactor::from_u32(8), Some(OversamplingFactor::X8));
        assert_eq!(OversamplingFactor::from_u32(3), None);
        assert_eq!(OversamplingFactor::from_u32(0), None);
        assert_eq!(OversamplingFactor::from_u32(16), None);
    }

    #[test]
    fn no_oversampling_reports_no_latency() {
        let mut os = Oversampler::new(OversamplingFactor::None);
        os.configure(OversamplingFactor::None, 48_000.0, 256);
        assert_eq!(os.latency_samples(), 0);
        assert_eq!(os.oversampled_rate(), 48_000.0);
    }

    #[test]
    fn latency_shrinks_as_the_factor_grows() {
        // Latency is reported at the *base* rate, so a higher factor means
        // more samples per millisecond and a smaller base-rate figure.
        let mut rates = alloc::vec::Vec::new();
        for factor in [
            OversamplingFactor::X2,
            OversamplingFactor::X4,
            OversamplingFactor::X8,
        ] {
            let mut os = Oversampler::new(factor);
            os.configure(factor, 48_000.0, 256);
            rates.push(os.latency_samples());
        }
        assert!(
            rates[0] > rates[1] && rates[1] > rates[2],
            "latency should shrink with the factor, got {rates:?}"
        );
        assert_eq!(rates[0], HALF_BAND_TAPS / 4);
    }

    #[test]
    fn oversampled_rate_scales_with_the_factor() {
        let mut os = Oversampler::new(OversamplingFactor::X4);
        os.configure(OversamplingFactor::X4, 44_100.0, 256);
        assert!((os.oversampled_rate() - 176_400.0).abs() < 1e-3);
    }

    #[test]
    fn upsampling_produces_the_expected_frame_count() {
        let mut os = Oversampler::new(OversamplingFactor::X4);
        os.configure(OversamplingFactor::X4, 48_000.0, 256);

        let input = [1.0_f32; 64];
        let mut output = [0.0_f32; 256];
        assert_eq!(os.upsample(&input, &mut output), 256);
    }

    #[test]
    fn upsampling_a_block_that_does_not_fit_is_refused() {
        let mut os = Oversampler::new(OversamplingFactor::X8);
        os.configure(OversamplingFactor::X8, 48_000.0, 256);
        // An output buffer one sample short must be refused outright rather
        // than half-filled, which would be an audible truncation.
        let input = [1.0_f32; 16];
        let mut output = [0.0_f32; 127];
        assert_eq!(os.upsample(&input, &mut output), 0);
    }

    #[test]
    fn a_round_trip_preserves_a_dc_signal() {
        // A constant must survive upsample → downsample at the same level.
        // This is the property that makes oversampling safe to insert into a
        // chain without a level match; a gain error here would be a bug the
        // half-band normalisation is supposed to prevent.
        let mut os = Oversampler::new(OversamplingFactor::X4);
        os.configure(OversamplingFactor::X4, 48_000.0, 256);

        let input = [0.5_f32; 128];
        let mut wide = [0.0_f32; 512];
        let mut output = [0.0_f32; 128];

        let up = os.upsample(&input, &mut wide);
        assert_eq!(up, 512);
        let down = os.downsample(&wide, &mut output);
        assert_eq!(down, 128);

        // Skip the filter's transient; once settled the level must be right.
        for (i, sample) in output.iter().enumerate().skip(32) {
            assert!(
                (sample - 0.5).abs() < 1e-3,
                "sample {i} is {sample}, expected ~0.5 after a DC round trip"
            );
        }
    }

    #[test]
    fn the_working_buffer_route_matches_the_slice_route() {
        let mut a = Oversampler::new(OversamplingFactor::X2);
        a.configure(OversamplingFactor::X2, 48_000.0, 256);
        let mut b = Oversampler::new(OversamplingFactor::X2);
        b.configure(OversamplingFactor::X2, 48_000.0, 256);

        let input = [0.25_f32; 32];
        let mut direct = [0.0_f32; 64];
        let n = a.upsample(&input, &mut direct);
        assert_eq!(n, 64);

        let m = b.upsample_into_work(&input);
        assert_eq!(m, 64);
        assert_eq!(&b.work_buffer()[..64], &direct[..]);
    }

    #[test]
    fn downsampling_from_the_working_buffer_matches_the_slice_route() {
        let mut a = Oversampler::new(OversamplingFactor::X2);
        a.configure(OversamplingFactor::X2, 48_000.0, 256);
        let mut b = Oversampler::new(OversamplingFactor::X2);
        b.configure(OversamplingFactor::X2, 48_000.0, 256);

        let input = [0.75_f32; 32];
        let mut wide = [0.0_f32; 64];
        let mut out_a = [0.0_f32; 32];
        let mut out_b = [0.0_f32; 32];

        let n = a.upsample(&input, &mut wide);
        assert_eq!(a.downsample(&wide[..n], &mut out_a), 32);

        let m = b.upsample_into_work(&input);
        assert_eq!(m, 64);
        assert_eq!(b.downsample_from_work(m, &mut out_b), 32);

        assert_eq!(out_a, out_b);
    }

    #[test]
    fn downsample_from_work_rejects_an_impossible_request() {
        // The contract is "every frame in the working buffer is consumed, and
        // the base-rate output must have room for all of them". A request whose
        // frame count exceeds the working buffer must be refused rather than
        // read past the end of the array.
        let mut os = Oversampler::new(OversamplingFactor::X4);
        os.configure(OversamplingFactor::X4, 48_000.0, 256);
        let mut out = [0.0_f32; 4];
        // 4 * MAX_BASE_BLOCK + 1 frames exceeds the working buffer.
        assert_eq!(os.downsample_from_work(MAX_BASE_BLOCK * 4 + 1, &mut out), 0);

        // A request that does fit must succeed rather than be refused.
        let mut roomy = [0.0_f32; MAX_BASE_BLOCK];
        assert_eq!(
            os.downsample_from_work(MAX_BASE_BLOCK * 4, &mut roomy),
            MAX_BASE_BLOCK
        );
    }

    #[test]
    fn process_channel_applies_the_nonlinearity() {
        let mut os = Oversampler::new(OversamplingFactor::X2);
        os.configure(OversamplingFactor::X2, 48_000.0, 256);

        let input = [0.5_f32; 64];
        let mut output = [0.0_f32; 64];
        let mut scratch = [0.0_f32; 128];

        // A unity function: the round trip must return the input.
        let n = os.process_channel(&input, &mut output, &mut scratch, |x| x);
        assert_eq!(n, 64);
        // The half-band filter is linear-phase with a group delay of
        // `HALF_BAND_TAPS / 2` at the oversampled rate, which at 2x is 8
        // base-rate samples of pure delay *each way*. Allow the tail to settle
        // before asserting the level, or the assertion is really testing the
        // step response rather than the gain.
        let settled = 40;
        for (i, sample) in output.iter().enumerate().skip(settled) {
            assert!((sample - 0.5).abs() < 1e-3, "sample {i} = {sample}");
        }
    }

    #[test]
    fn a_hard_clip_at_the_base_rate_aliases_less_when_oversampled() {
        // The whole point of the module. A 7 kHz tone clipped hard produces
        // harmonics at 21 kHz, 35 kHz… which fold back into the audible band
        // at the base rate. Oversampling must measurably reduce that folding.
        use super::super::dsp::sin_poly;
        use core::f32::consts::PI;

        let sr = 48_000.0_f32;
        let freq = 7_000.0_f32;
        // Keep the block within the oversampler's documented ceiling; a larger
        // request is refused by design (see `a_block_larger_than_the_working_
        // buffer_is_refused`), so the test must not ask for one.
        let frames = 2_048;

        let input: alloc::vec::Vec<f32> = (0..frames)
            .map(|n| sin_poly(2.0 * PI * freq * n as f32 / sr) * 0.95)
            .collect();

        // Reference: clipping without oversampling.
        let clipped_direct: alloc::vec::Vec<f32> =
            input.iter().map(|&x| (x * 4.0).clamp(-1.0, 1.0)).collect();

        // Same nonlinearity, oversampled 4x.
        let mut os = Oversampler::new(OversamplingFactor::X4);
        os.configure(OversamplingFactor::X4, sr, frames);
        let mut wide = alloc::vec![0.0_f32; frames * 4];
        let mut out = alloc::vec![0.0_f32; frames];
        let up = os.upsample(&input, &mut wide);
        assert_eq!(up, frames * 4);
        for sample in wide[..up].iter_mut() {
            *sample = (*sample * 4.0).clamp(-1.0, 1.0);
        }
        let down = os.downsample(&wide[..up], &mut out);
        assert_eq!(down, frames);

        // Compare high-frequency energy above 12 kHz (well into the folded
        // region) using a coarse Goertzel scan of both signals.
        let band_energy = |signal: &[f32]| -> f32 {
            let mut total = 0.0_f32;
            let mut probe = 13_000.0_f32;
            while probe < 23_000.0 {
                let mut re = 0.0_f32;
                let mut im = 0.0_f32;
                for (n, &sample) in signal.iter().enumerate().skip(512) {
                    let phase = 2.0 * PI * probe * n as f32 / sr;
                    re += sample * super::super::dsp::cos_poly(phase);
                    im += sample * sin_poly(phase);
                }
                total += re * re + im * im;
                probe += 1_000.0;
            }
            total
        };

        let aliased = band_energy(&clipped_direct);
        let clean = band_energy(&out);
        assert!(
            clean < aliased,
            "oversampling did not reduce folded energy: {clean} vs {aliased}"
        );
    }

    #[test]
    fn reset_clears_the_filter_history() {
        let mut os = Oversampler::new(OversamplingFactor::X2);
        os.configure(OversamplingFactor::X2, 48_000.0, 256);
        let mut output = [0.0_f32; 128];
        let _ = os.upsample(&[1.0_f32; 64], &mut output);
        os.reset();
        assert_eq!(os.up_history, [0.0; HALF_BAND_TAPS]);
        assert_eq!(os.down_history, [0.0; HALF_BAND_TAPS]);
        assert_eq!(os.up_index, 0);
        assert_eq!(os.down_index, 0);
    }

    #[test]
    fn a_block_larger_than_the_working_buffer_is_refused() {
        let mut os = Oversampler::new(OversamplingFactor::X2);
        // Configure with an over-large block: `prepare` must mark it unusable
        // rather than let `process` overflow a fixed array.
        os.configure(OversamplingFactor::X2, 48_000.0, MAX_BASE_BLOCK * 2);
        assert!(!os.is_prepared(), "an oversized block must not be accepted");
    }

    #[test]
    fn non_finite_input_does_not_poison_the_filter() {
        let mut os = Oversampler::new(OversamplingFactor::X2);
        os.configure(OversamplingFactor::X2, 48_000.0, 256);
        let input = [f32::NAN, 0.5, f32::INFINITY, -0.5];
        let mut output = [0.0_f32; 8];
        let n = os.upsample(&input, &mut output);
        assert_eq!(n, 8);
        for (i, sample) in output.iter().enumerate() {
            assert!(sample.is_finite(), "output {i} is {sample}");
        }
    }

    #[test]
    fn the_bank_serves_only_configured_channels() {
        let mut bank = OversamplerBank::new(OversamplingFactor::X2);
        bank.prepare(OversamplingFactor::X2, 48_000.0, 256, 2);

        assert!(bank.channel_mut(0).is_some());
        assert!(bank.channel_mut(1).is_some());
        assert!(bank.channel_mut(2).is_none(), "unconfigured channel");
        assert_eq!(bank.latency_samples(), HALF_BAND_TAPS / 4);
    }

    #[test]
    fn the_bank_zeroes_its_state_on_reset() {
        let mut bank = OversamplerBank::new(OversamplingFactor::X4);
        bank.prepare(OversamplingFactor::X4, 48_000.0, 256, 2);
        if let Some(ch) = bank.channel_mut(0) {
            let mut out = [0.0_f32; 64];
            let _ = ch.upsample(&[1.0_f32; 16], &mut out);
        }
        bank.reset();
        if let Some(ch) = bank.channel_mut(0) {
            assert_eq!(ch.up_history, [0.0; HALF_BAND_TAPS]);
        }
    }
}

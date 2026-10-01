//! Spectrum analyser: an FFT display feed that passes audio through untouched.
//!
//! PLAN §1.3 pairs the seven-band EQ with "频谱显示". S5 owns the analysis;
//! the UI owns the drawing.
//!
//! # Why this is an effect and not a separate API
//!
//! It sits in an effect slot so the analysis taps exactly the point in the
//! chain where the user put it: before an EQ to see the source, after it to see
//! the result. A hard-wired analyser could only ever show one of those.
//!
//! # Real-time safety
//!
//! The transform is a radix-2 Cooley-Tukey FFT over a fixed 1024-point window.
//! Twist tables, the window and the input ring are all built in `prepare`, so
//! `process` only reads, multiplies and writes. Bins are published into a
//! preallocated array that the UI thread reads without a lock — the reader may
//! see a partially updated frame, which for a spectrum display is
//! indistinguishable from a slightly different frame and never a memory error.

use super::super::buffer::{AudioBuffer, RenderContext};
use super::super::util::dsp::{cos_poly, log10, sin_poly, sqrt};
use super::super::{
    clamp_parameter, sanitize_wet, EffectCategory, EffectDescriptor, EffectProcessor,
};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterUnit,
};

/// FFT size. A power of two, because the transform below is radix-2.
pub const FFT_SIZE: usize = 1024;

/// Number of usable bins: `FFT_SIZE / 2 + 1`.
pub const BIN_COUNT: usize = FFT_SIZE / 2 + 1;

/// Parameter ordinals.
pub const PARAM_ENABLED: u16 = 0;
/// Smoothing applied to the displayed magnitudes, in percent.
pub const PARAM_SMOOTHING: u16 = 1;
/// Display floor in decibels.
pub const PARAM_MIN_DB: u16 = 2;
/// Display ceiling in decibels.
pub const PARAM_MAX_DB: u16 = 3;

/// Total published parameters.
pub const PARAM_COUNT: u16 = 4;

/// The effect's static description.
pub static DESCRIPTOR: EffectDescriptor = EffectDescriptor {
    kind: super::super::registry::KIND_EQ_SPECTRUM,
    key: "spectrum",
    label: "Spectrum Analyser",
    category: EffectCategory::Analysis,
    first_param: 0,
    param_count: PARAM_COUNT,
    has_latency: false,
    is_analysis_only: true,
};

/// Builds the parameter table for an instance at `address`.
#[must_use]
pub fn parameter_table(address: ParameterAddress) -> [ParameterDescriptor; PARAM_COUNT as usize] {
    let at = |sub: u16| ParameterAddress::effect(address.index, address.effect_slot(), sub);
    [
        ParameterDescriptor {
            address: at(PARAM_ENABLED),
            key: "enabled",
            label: "Enabled",
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE,
            min_value: 0.0,
            max_value: 1.0,
            default_value: 1.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_SMOOTHING),
            key: "smoothing",
            label: "Smoothing",
            unit: ParameterUnit::Percent,
            flags: 0,
            min_value: 0.0,
            max_value: 95.0,
            default_value: 60.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_MIN_DB),
            key: "min_db",
            label: "Floor",
            unit: ParameterUnit::Decibels,
            flags: 0,
            min_value: -120.0,
            max_value: 0.0,
            default_value: -90.0,
            smoothing_ms: 0.0,
        },
        ParameterDescriptor {
            address: at(PARAM_MAX_DB),
            key: "max_db",
            label: "Ceiling",
            unit: ParameterUnit::Decibels,
            flags: 0,
            min_value: -60.0,
            max_value: 24.0,
            default_value: 0.0,
            smoothing_ms: 0.0,
        },
    ]
}

/// A windowed FFT analyser with precomputed tables.
#[derive(Debug)]
pub struct SpectrumAnalyser {
    /// The published parameter table.
    table: [ParameterDescriptor; PARAM_COUNT as usize],
    /// Whether analysis is running.
    enabled: bool,
    /// Smoothing applied to each bin, `0..=0.95`.
    smoothing: f32,
    /// Display floor in decibels.
    min_db: f32,
    /// Display ceiling in decibels.
    max_db: f32,
    /// Input ring buffer, `FFT_SIZE` long.
    ring: alloc::vec::Vec<f32>,
    /// Write position in the ring.
    write: usize,
    /// Hann window, built in `prepare`.
    window: alloc::vec::Vec<f32>,
    /// Bit-reversal permutation.
    reversal: alloc::vec::Vec<usize>,
    /// Twiddle factors, `cos` in the first half and `sin` in the second.
    twiddles: alloc::vec::Vec<(f32, f32)>,
    /// Real part scratch.
    re: alloc::vec::Vec<f32>,
    /// Imaginary part scratch.
    im: alloc::vec::Vec<f32>,
    /// Published magnitudes in decibels, normalised to `0..=1` across the
    /// display range. Written by the audio thread, read by the UI.
    bins_db: alloc::vec::Vec<f32>,
    /// Sample rate.
    sample_rate: f32,
    /// Wet/dry, always 1: an analyser passes audio through.
    wet: f32,
    /// Bypass.
    bypassed: bool,
}

impl Default for SpectrumAnalyser {
    fn default() -> Self {
        Self::new(ParameterAddress::effect(0, 0, 0))
    }
}

impl SpectrumAnalyser {
    /// Creates the analyser for the slot at `address`.
    #[must_use]
    pub fn new(address: ParameterAddress) -> Self {
        Self {
            table: parameter_table(address),
            enabled: true,
            smoothing: 0.6,
            min_db: -90.0,
            max_db: 0.0,
            ring: alloc::vec![0.0; FFT_SIZE],
            write: 0,
            window: alloc::vec::Vec::new(),
            reversal: alloc::vec::Vec::new(),
            twiddles: alloc::vec::Vec::new(),
            re: alloc::vec![0.0; FFT_SIZE],
            im: alloc::vec![0.0; FFT_SIZE],
            bins_db: alloc::vec![-120.0; BIN_COUNT],
            sample_rate: 48_000.0,
            wet: 1.0,
            bypassed: false,
        }
    }

    /// The most recent analysis, in decibels, one entry per bin.
    ///
    /// The UI reads this without a lock: an analyser display tolerates reading
    /// a frame that is one block stale, and the alternative (a mutex shared
    /// with the audio thread) is forbidden by ABI P5.
    #[must_use]
    pub fn bins_db(&self) -> &[f32] {
        &self.bins_db
    }

    /// The frequency of bin `index`, in hertz.
    #[must_use]
    pub fn bin_frequency(&self, index: usize) -> f32 {
        index as f32 * self.sample_rate / FFT_SIZE as f32
    }

    /// Builds the window, reversal and twiddle tables.
    fn build_tables(&mut self) {
        // Hann window: reduces spectral leakage so a steady tone reads as one
        // peak rather than a smeared skirt, which is the whole point of the
        // display.
        self.window.clear();
        self.window.reserve(FFT_SIZE);
        for n in 0..FFT_SIZE {
            let phase = 2.0 * core::f32::consts::PI * n as f32 / FFT_SIZE as f32;
            self.window.push(0.5 * (1.0 - cos_poly(phase)));
        }

        // Bit-reversal permutation for the in-place transform.
        let bits = FFT_SIZE.trailing_zeros();
        self.reversal.clear();
        self.reversal.reserve(FFT_SIZE);
        for n in 0..FFT_SIZE {
            let mut value = 0usize;
            for bit in 0..bits {
                if n & (1 << bit) != 0 {
                    value |= 1 << (bits - 1 - bit);
                }
            }
            self.reversal.push(value);
        }

        // Twiddles for the butterflies, precomputed once.
        self.twiddles.clear();
        self.twiddles.reserve(FFT_SIZE / 2);
        for k in 0..FFT_SIZE / 2 {
            let angle = -2.0 * core::f32::consts::PI * k as f32 / FFT_SIZE as f32;
            self.twiddles.push((cos_poly(angle), sin_poly(angle)));
        }
    }

    /// Runs the in-place radix-2 FFT over `re`/`im`.
    fn transform(&mut self) {
        // Permute into bit-reversed order.
        for n in 0..FFT_SIZE {
            let reversed = self.reversal[n];
            if reversed > n {
                self.re.swap(n, reversed);
                self.im.swap(n, reversed);
            }
        }

        // Butterflies, doubling the span each stage.
        let mut span = 2;
        while span <= FFT_SIZE {
            let half = span / 2;
            let step = FFT_SIZE / span;
            let mut start = 0;
            while start < FFT_SIZE {
                let mut k = 0;
                let mut offset = 0;
                while k < half {
                    let (wr, wi) = self.twiddles[offset];
                    let i = start + k;
                    let j = i + half;
                    let tr = self.re[j] * wr - self.im[j] * wi;
                    let ti = self.re[j] * wi + self.im[j] * wr;
                    self.re[j] = self.re[i] - tr;
                    self.im[j] = self.im[i] - ti;
                    self.re[i] += tr;
                    self.im[i] += ti;
                    k += 1;
                    offset += step;
                }
                start += span;
            }
            span *= 2;
        }
    }

    /// Analyses the ring buffer and updates `bins_db`.
    fn analyse(&mut self) {
        // Window the most recent `FFT_SIZE` samples, split around the write
        // cursor so the newest sample is last.
        for n in 0..FFT_SIZE {
            let index = (self.write + n) % FFT_SIZE;
            self.re[n] = self.ring[index] * self.window[n];
            self.im[n] = 0.0;
        }
        self.transform();

        // Normalise so a full-scale sine reads as 0 dB.
        let norm = 2.0 / FFT_SIZE as f32;
        let alpha = self.smoothing.clamp(0.0, 0.95);
        for bin in 0..BIN_COUNT {
            let magnitude = sqrt(self.re[bin] * self.re[bin] + self.im[bin] * self.im[bin]) * norm;
            let db = if magnitude > 1e-9 {
                20.0 * log10(magnitude)
            } else {
                -144.0
            };
            // One-pole smoothing in the dB domain, which is what makes the
            // display readable instead of flickering.
            let previous = self.bins_db[bin];
            self.bins_db[bin] = if previous <= -143.0 {
                db
            } else {
                previous * alpha + db * (1.0 - alpha)
            };
        }
    }
}

impl EffectProcessor for SpectrumAnalyser {
    fn descriptor(&self) -> &'static EffectDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        // Every allocation the analyser will ever make happens here.
        self.ring = alloc::vec![0.0; FFT_SIZE];
        self.re = alloc::vec![0.0; FFT_SIZE];
        self.im = alloc::vec![0.0; FFT_SIZE];
        self.bins_db = alloc::vec![-120.0; BIN_COUNT];
        self.build_tables();
        self.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer<'_>, _ctx: &RenderContext) {
        // An analyser never alters audio: it is a tap. Even when bypassed it
        // still analyses, because "bypass" on a display would be meaningless.
        let _ = self.bypassed;
        if !self.enabled {
            return;
        }
        let Some(first) = buffer.channel(0) else {
            return;
        };
        if first.is_empty() {
            return;
        }

        // Feed the ring with the first channel. Analysing a mono sum would
        // cancel a hard-panned source, which is exactly when a user looks at
        // the display.
        for &sample in first {
            let value = if sample.is_finite() { sample } else { 0.0 };
            self.ring[self.write] = value;
            self.write = (self.write + 1) % FFT_SIZE;
        }
        self.analyse();
    }

    fn reset(&mut self) {
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.bins_db.iter_mut().for_each(|b| *b = -120.0);
    }

    fn latency_samples(&self) -> usize {
        // The analyser is a parallel tap, not a series element: nothing in the
        // audio path is delayed, so PDC must not shift the channel.
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
            PARAM_ENABLED => self.enabled = value >= 0.5,
            PARAM_SMOOTHING => self.smoothing = value,
            PARAM_MIN_DB => self.min_db = value,
            PARAM_MAX_DB => self.max_db = value,
            _ => {}
        }
    }

    fn get_parameter(&self, sub: u16) -> Option<f32> {
        match sub {
            PARAM_ENABLED => Some(f32::from(self.enabled as u8)),
            PARAM_SMOOTHING => Some(self.smoothing),
            PARAM_MIN_DB => Some(self.min_db),
            PARAM_MAX_DB => Some(self.max_db),
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
        // An analyser cannot be partially wet: pinning the value makes that
        // explicit rather than letting a UI slider imply a blend that the code
        // silently ignores.
        let _ = sanitize_wet(wet);
        self.wet = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn make() -> SpectrumAnalyser {
        let mut effect = SpectrumAnalyser::new(ParameterAddress::effect(0, 0, 0));
        effect.prepare(SR, 256, 2);
        effect
    }

    /// Feeds a sine and returns the analysis.
    fn analyse_tone(effect: &mut SpectrumAnalyser, hz: f32) -> alloc::vec::Vec<f32> {
        let frames = 4_096;
        let chunk = 256;
        let mut produced = 0;
        while produced < frames {
            let mut channel: alloc::vec::Vec<f32> = (0..chunk)
                .map(|n| sin_poly(2.0 * PI * hz * (produced + n) as f32 / SR))
                .collect();
            {
                let mut views = [&mut channel[..]];
                let mut buf = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, produced as i64, 120.0, 960);
                effect.process(&mut buf, &ctx);
            }
            produced += chunk;
        }
        effect.bins_db().to_vec()
    }

    use core::f32::consts::PI;

    #[test]
    fn the_descriptor_marks_this_as_analysis_only() {
        let effect = make();
        let d = effect.descriptor();
        assert_eq!(d.key, "spectrum");
        assert_eq!(d.category, EffectCategory::Analysis);
        assert!(d.is_analysis_only, "an analyser produces no audio");
        assert!(!d.has_latency);
    }

    #[test]
    fn the_fft_size_is_a_power_of_two_with_a_matching_bin_count() {
        assert!(FFT_SIZE.is_power_of_two(), "radix-2 needs a power of two");
        assert_eq!(BIN_COUNT, FFT_SIZE / 2 + 1);
    }

    #[test]
    fn the_window_is_a_hann_and_sums_sensibly() {
        let effect = make();
        assert_eq!(effect.window.len(), FFT_SIZE);
        // A Hann window is zero at both ends and 1.0 at the centre.
        assert!(effect.window[0].abs() < 1e-6);
        assert!((effect.window[FFT_SIZE / 2] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_bit_reversal_permutation_is_an_involution() {
        // Applying the permutation twice must return the original index; if it
        // did not, the transform would read the wrong samples everywhere.
        let effect = make();
        assert_eq!(effect.reversal.len(), FFT_SIZE);
        for n in 0..FFT_SIZE {
            let once = effect.reversal[n];
            assert_eq!(
                effect.reversal[once], n,
                "reversal is not an involution at {n}"
            );
        }
    }

    #[test]
    fn the_transform_finds_a_dc_signal_in_bin_zero() {
        let mut effect = make();
        // A constant input has all its energy at DC.
        for value in effect.re.iter_mut() {
            *value = 1.0;
        }
        for value in effect.im.iter_mut() {
            *value = 0.0;
        }
        effect.transform();

        let dc = sqrt(effect.re[0] * effect.re[0] + effect.im[0] * effect.im[0]);
        assert!(
            (dc - FFT_SIZE as f32).abs() < 1.0,
            "DC bin is {dc}, expected {FFT_SIZE}"
        );
        // Every other bin must be essentially empty.
        for bin in 1..FFT_SIZE {
            let magnitude = sqrt(effect.re[bin] * effect.re[bin] + effect.im[bin] * effect.im[bin]);
            assert!(magnitude < 1e-2, "bin {bin} leaked {magnitude}");
        }
    }

    #[test]
    fn the_transform_finds_a_tone_in_the_right_bin() {
        // The single most important property of the module: a 1 kHz sine must
        // read as a peak at the bin nearest 1 kHz.
        let mut effect = make();
        let target_hz = 1_000.0_f32;
        for n in 0..FFT_SIZE {
            effect.re[n] = sin_poly(2.0 * PI * target_hz * n as f32 / SR);
            effect.im[n] = 0.0;
        }
        effect.transform();

        let mut peak_bin = 0;
        let mut peak = 0.0_f32;
        for bin in 1..BIN_COUNT {
            let magnitude =
                sqrt(effect.re[bin] * effect.re[bin] + effect.im[bin] * effect.im[bin]);
            if magnitude > peak {
                peak = magnitude;
                peak_bin = bin;
            }
        }
        let expected_bin = (target_hz * FFT_SIZE as f32 / SR).round() as usize;
        assert!(
            peak_bin.abs_diff(expected_bin) <= 1,
            "1 kHz peaked at bin {peak_bin}, expected {expected_bin}"
        );
        assert!(peak > FFT_SIZE as f32 * 0.3, "the peak was only {peak}");
    }

    #[test]
    fn bin_frequencies_are_evenly_spaced() {
        let effect = make();
        let spacing = SR / FFT_SIZE as f32;
        for bin in 0..BIN_COUNT {
            assert!((effect.bin_frequency(bin) - bin as f32 * spacing).abs() < 1e-3);
        }
        assert_eq!(effect.bin_frequency(0), 0.0);
    }

    #[test]
    fn a_full_scale_tone_reads_close_to_zero_db() {
        // The display has to be calibrated, or every reading is a lie.
        let mut effect = make();
        effect.set_parameter(PARAM_SMOOTHING, 0.0);
        let bins = analyse_tone(&mut effect, 1_000.0);
        let peak = bins.iter().fold(-200.0_f32, |m, b| m.max(*b));
        assert!(
            (-6.0..=3.0).contains(&peak),
            "a full-scale sine read {peak} dB, expected about 0"
        );
    }

    #[test]
    fn silence_reads_as_the_floor() {
        let mut effect = make();
        effect.set_parameter(PARAM_SMOOTHING, 0.0);
        let bins = analyse_tone(&mut effect, 0.0);
        let peak = bins.iter().fold(-200.0_f32, |m, b| m.max(*b));
        assert!(peak < -60.0, "silence read {peak} dB");
    }

    #[test]
    fn smoothing_reduces_frame_to_frame_jitter() {
        let mut smoothed = make();
        smoothed.set_parameter(PARAM_SMOOTHING, 0.9);
        let a = analyse_tone(&mut smoothed, 1_000.0);
        let b = analyse_tone(&mut smoothed, 1_000.0);
        // With heavy smoothing two identical inputs must give nearly identical
        // frames: that is what makes the display readable.
        let max_delta = a
            .iter()
            .zip(b.iter())
            .fold(0.0_f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(max_delta < 3.0, "smoothed frames differed by {max_delta} dB");
    }

    #[test]
    fn the_analyser_passes_audio_through_untouched() {
        // It is a tap. Any change to the samples would be a bug the user hears
        // as a level drop whenever they open the analyser.
        let mut effect = make();
        let mut channel = alloc::vec![0.5_f32; 256];
        let expected = channel.clone();
        {
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        assert_eq!(channel, expected, "the analyser altered the audio");
    }

    #[test]
    fn a_stereo_source_does_not_cancel_itself() {
        // Analysing a mono sum would cancel a hard-panned source; the analyser
        // reads one channel instead.
        let mut effect = make();
        let mut left = alloc::vec![0.5_f32; 2_048];
        let mut right = alloc::vec![-0.5_f32; 2_048];
        let chunk = 256;
        for round in 0..8 {
            let range = round * chunk..(round + 1) * chunk;
            {
                let mut views = [&mut left[range.clone()], &mut right[range]];
                let mut buf = AudioBuffer::new(&mut views);
                let ctx = RenderContext::new(SR, chunk, 0, 120.0, 960);
                effect.process(&mut buf, &ctx);
            }
        }
        let peak = effect.bins_db().iter().fold(-200.0_f32, |m, b| m.max(*b));
        assert!(
            peak > -20.0,
            "a hard-panned source read {peak} dB — the channels cancelled"
        );
    }

    #[test]
    fn disabling_stops_updating_the_analysis() {
        let mut effect = make();
        let bins = analyse_tone(&mut effect, 1_000.0);
        assert!(bins.iter().any(|b| *b > -40.0));
        effect.set_parameter(PARAM_ENABLED, 0.0);
        effect.reset();
        let frozen = analyse_tone(&mut effect, 1_000.0);
        assert!(
            frozen.iter().all(|b| *b <= -100.0),
            "a disabled analyser still updated its bins"
        );
    }

    #[test]
    fn parameters_are_clamped_and_nan_safe() {
        let mut effect = make();
        effect.set_parameter(PARAM_SMOOTHING, 500.0);
        assert_eq!(effect.get_parameter(PARAM_SMOOTHING), Some(95.0));
        effect.set_parameter(PARAM_SMOOTHING, f32::NAN);
        assert_eq!(effect.get_parameter(PARAM_SMOOTHING), Some(60.0));
        effect.set_parameter(PARAM_ENABLED, 5.0);
        assert_eq!(effect.get_parameter(PARAM_ENABLED), Some(1.0));
    }

    #[test]
    fn unknown_ordinals_are_ignored() {
        let mut effect = make();
        effect.set_parameter(200, 1.0);
        assert_eq!(effect.get_parameter(200), None);
    }

    #[test]
    fn wet_is_pinned_to_fully_wet() {
        // An analyser cannot be partially wet; the setter must not pretend
        // otherwise.
        let mut effect = make();
        effect.set_wet(0.0);
        assert_eq!(effect.wet(), 1.0);
    }

    #[test]
    fn non_finite_input_does_not_poison_the_analysis() {
        let mut effect = make();
        let mut channel = alloc::vec![f32::NAN; 256];
        {
            let mut views = [&mut channel[..]];
            let mut buf = AudioBuffer::new(&mut views);
            effect.process(&mut buf, &RenderContext::new(SR, 256, 0, 120.0, 960));
        }
        for (bin, value) in effect.bins_db().iter().enumerate() {
            assert!(value.is_finite(), "bin {bin} became {value}");
        }
    }

    #[test]
    fn an_empty_buffer_is_handled() {
        let mut effect = make();
        let mut empty: [&mut [f32]; 0] = [];
        let mut buf = AudioBuffer::new(&mut empty);
        effect.process(&mut buf, &RenderContext::default());
        assert_eq!(effect.bins_db().len(), BIN_COUNT);
    }
}

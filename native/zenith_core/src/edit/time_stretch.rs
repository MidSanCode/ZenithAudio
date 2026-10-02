//! Time stretching and pitch shifting.
//!
//! # Algorithm
//!
//! WSOLA (Waveform Similarity Overlap-Add): the signal is cut into overlapping
//! frames, and each frame's read position is advanced by a rate that differs
//! from the output rate. Before the overlap-add, the frame is nudged within a
//! small search window to the position that best correlates with the previous
//! frame's tail — that alignment is what keeps the waveform phase-consistent
//! and is what separates WSOLA from a naive overlap-add, which would warble.
//!
//! # Why WSOLA for S1
//!
//! A phase vocoder preserves transients better but is more expensive and has
//! more parameters to get wrong. WSOLA is simple, allocation-bounded and good
//! enough that "±50 % with no obvious metallic ring" (the S8 acceptance) is
//! reachable. The phase vocoder is the upgrade path if the acceptance proves too
//! demanding in practice.

use alloc::vec::Vec;

/// Frames per analysis window.
///
/// ~40 ms at 48 kHz. Long enough for a stable correlation, short enough that a
/// transient does not smear across two windows.
pub const DEFAULT_FRAME: usize = 2_048;

/// Hop between output frames.
///
/// A 50 % overlap is the standard overlap-add compromise; more overlap is
/// smoother but costs proportionally more.
pub const DEFAULT_HOP: usize = DEFAULT_FRAME / 2;

/// How far WSOLA may slide a frame to find the best match, in samples.
pub const DEFAULT_SEARCH: usize = 256;

/// Configuration for [`time_stretch`].
#[derive(Debug, Clone, Copy)]
pub struct StretchConfig {
    /// Analysis/output frame length, in samples.
    pub frame: usize,
    /// Output hop, in samples.
    pub hop: usize,
    /// Maximum alignment search offset, in samples.
    pub search: usize,
    /// Cross-fade window applied to each overlap, in samples.
    pub fade: usize,
}

impl Default for StretchConfig {
    fn default() -> Self {
        Self {
            frame: DEFAULT_FRAME,
            hop: DEFAULT_HOP,
            search: DEFAULT_SEARCH,
            fade: DEFAULT_FRAME / 4,
        }
    }
}

impl StretchConfig {
    /// Clamps the configuration into a usable range.
    #[must_use]
    pub fn sanitized(self, input_len: usize) -> Self {
        let frame = self.frame.clamp(64, 65_536).min(input_len.max(64));
        let hop = self.hop.clamp(1, frame);
        let search = self.search.min(frame);
        let fade = self.fade.min(hop).max(1);
        Self {
            frame,
            hop,
            search,
            fade,
        }
    }
}

/// Stretches `input` in time by `factor` without changing its pitch.
///
/// `factor > 1` makes the result longer (slower); `factor < 1` shorter
/// (faster). `factor == 1` returns a copy unchanged. Output length is
/// `round(input.len() * factor)`.
///
/// The signal is treated as mono; a caller with stereo should stretch each
/// channel with the **same** factor (a shared WSOLA would smear the image, and
/// independent stretching would drift the channels apart).
#[must_use]
pub fn time_stretch(input: &[f32], factor: f32) -> Vec<f32> {
    time_stretch_with(input, factor, StretchConfig::default())
}

/// [`time_stretch`] with explicit configuration.
#[must_use]
pub fn time_stretch_with(input: &[f32], factor: f32, config: StretchConfig) -> Vec<f32> {
    if input.is_empty() || !factor.is_finite() || factor <= 0.0 {
        return input.to_vec();
    }
    if (factor - 1.0).abs() < 1e-6 {
        return input.to_vec();
    }
    let cfg = config.sanitized(input.len());
    let out_len = ((input.len() as f64 * factor as f64).round() as usize).max(1);
    let mut output = alloc::vec![0.0f32; out_len];

    // Synthesis position is the output write cursor. The analysis position is
    // the source read cursor; it advances by `hop / factor` per output hop,
    // which is what changes the duration.
    let mut out_pos = 0usize;
    let mut analysis = 0.0f64;
    // Window is a Hann window over `frame` samples; precomputed once per
    // stretch so the inner loop is multiply-add only.
    let window = hann(cfg.frame);

    while out_pos < out_len {
        let base = analysis.floor() as isize;
        // Find the best alignment within `search`, comparing this frame's head
        // with the previously written tail.
        let offset = if out_pos == 0 {
            0
        } else {
            best_offset(input, base, &output, out_pos, &cfg)
        };
        let read_start = base + offset;

        for (i, window_sample) in window.iter().enumerate().take(cfg.frame) {
            let src = read_start + i as isize;
            let dst = out_pos + i;
            if dst >= out_len {
                break;
            }
            let sample = if src >= 0 && (src as usize) < input.len() {
                input[src as usize]
            } else {
                0.0
            };
            output[dst] += sample * window_sample;
        }

        analysis += cfg.hop as f64 / factor as f64;
        out_pos += cfg.hop;

        // Guard against a pathological factor stalling the loop.
        if analysis < 0.0 {
            break;
        }
    }

    // The Hann window overlaps by 50 %, so the sum of squared windows is ~1 and
    // the result needs no renorm beyond a gentle clamp for the head/tail.
    normalize_edges(&mut output, &window, cfg);
    output
}

/// Shifts `input` in pitch by `semitones`, preserving its duration.
///
/// Implemented as resample-then-stretch: play the signal back at a rate that
/// moves the pitch by `semitones`, then time-stretch the result back to the
/// original length. The two factors are exact inverses, so the length is
/// preserved and only the pitch changes.
#[must_use]
pub fn pitch_shift(input: &[f32], semitones: f32) -> Vec<f32> {
    if input.is_empty() || !semitones.is_finite() || semitones == 0.0 {
        return input.to_vec();
    }
    let ratio = crate::effects::util::dsp::exp2(semitones / 12.0);
    if !ratio.is_finite() || ratio <= 0.0 {
        return input.to_vec();
    }
    // Play the signal back at `ratio` (which moves the pitch by `ratio` and
    // divides the length by `ratio`), then stretch by `ratio` to restore the
    // original length. Net effect: pitch moves, duration does not.
    let resampled = resample_linear(input, ratio);
    time_stretch(&resampled, ratio)
}

/// Linearly resamples `input` by `step` source samples per output sample.
///
/// `step == 1` is a pass-through; `step == 2` halves the length (up an octave);
/// `step == 0.5` doubles it (down an octave). Linear interpolation, adequate
/// for the pitch-shift path where the output is stretched anyway.
#[must_use]
pub fn resample_linear(input: &[f32], step: f32) -> Vec<f32> {
    if input.is_empty() || !step.is_finite() || step <= 0.0 {
        return input.to_vec();
    }
    if (step - 1.0).abs() < 1e-6 {
        return input.to_vec();
    }
    let out_len = ((input.len() as f64 / step as f64).round() as usize).max(1);
    let mut output = alloc::vec![0.0f32; out_len];
    let mut read = 0.0f64;
    for sample in &mut output {
        let i = read.floor() as usize;
        let frac = (read - i as f64) as f32;
        let a = input.get(i).copied().unwrap_or(0.0);
        let b = input.get(i + 1).copied().unwrap_or(a);
        *sample = a + (b - a) * frac;
        read += step as f64;
    }
    output
}

/// A Hann window of `len` samples.
fn hann(len: usize) -> Vec<f32> {
    let mut w = alloc::vec![0.0f32; len];
    if len == 1 {
        w[0] = 1.0;
        return w;
    }
    for (i, value) in w.iter_mut().enumerate() {
        let phase = core::f32::consts::TAU * i as f32 / (len - 1) as f32;
        // 0.5 * (1 - cos) without a platform cos; the shared approximation is
        // accurate enough for a window.
        *value = 0.5 * (1.0 - crate::effects::util::dsp::cos_poly(phase));
    }
    w
}

/// Finds the search offset whose source window best matches the output tail.
fn best_offset(
    input: &[f32],
    base: isize,
    output: &[f32],
    out_pos: usize,
    cfg: &StretchConfig,
) -> isize {
    // Compare the first `fade` samples of the candidate source frame against
    // the last `fade` output samples already written.
    let overlap = cfg.fade.min(cfg.frame).min(out_pos);
    if overlap == 0 {
        return 0;
    }
    let mut best = 0isize;
    let mut best_score = f32::MIN;
    let max_off = cfg.search as isize;
    for off in -max_off..=max_off {
        let mut score = 0.0f32;
        for k in 0..overlap {
            let src = base + off + k as isize;
            let s = if src >= 0 && (src as usize) < input.len() {
                input[src as usize]
            } else {
                0.0
            };
            let o = output[out_pos - overlap + k];
            score += s * o;
        }
        if score > best_score {
            best_score = score;
            best = off;
        }
    }
    best
}

/// Trims the small edge artefacts a Hann overlap-add leaves at the very start
/// and end by applying a short linear fade.
fn normalize_edges(output: &mut [f32], window: &[f32], cfg: StretchConfig) {
    let fade = cfg.fade.min(output.len() / 2).max(1);
    let n = output.len();
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        output[i] *= g;
        output[n - 1 - i] *= g;
    }
    let _ = window;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(len: usize, hz: f32, rate: f32) -> Vec<f32> {
        (0..len)
            .map(|i| crate::effects::util::dsp::sin_poly(core::f32::consts::TAU * hz * i as f32 / rate))
            .collect()
    }

    #[test]
    fn factor_one_is_a_copy() {
        let input = sine(1_000, 440.0, 48_000.0);
        let out = time_stretch(&input, 1.0);
        assert_eq!(out, input);
    }

    #[test]
    fn a_longer_factor_produces_more_samples() {
        let input = sine(4_000, 440.0, 48_000.0);
        let out = time_stretch(&input, 2.0);
        assert!((out.len() as i64 - 8_000).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn a_shorter_factor_produces_fewer_samples() {
        let input = sine(4_000, 440.0, 48_000.0);
        let out = time_stretch(&input, 0.5);
        assert!((out.len() as i64 - 2_000).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn stretching_stays_finite_and_bounded() {
        let input = sine(10_000, 220.0, 48_000.0);
        for factor in [0.5, 0.75, 1.5, 2.0] {
            let out = time_stretch(&input, factor);
            assert!(out.iter().all(|s| s.is_finite()), "factor {factor}");
            // A 0.5-amplitude WSOLA of a sine should not blow up.
            assert!(out.iter().all(|s| s.abs() < 4.0), "factor {factor}");
        }
    }

    #[test]
    fn a_bad_factor_is_a_copy() {
        let input = sine(100, 440.0, 48_000.0);
        assert_eq!(time_stretch(&input, 0.0), input);
        assert_eq!(time_stretch(&input, f32::NAN), input);
    }

    #[test]
    fn resample_linear_doubling_halves_the_length() {
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let out = resample_linear(&input, 2.0);
        assert_eq!(out.len(), 50);
        assert!((out[1] - 2.0).abs() < 1e-4, "linear interp, got {}", out[1]);
    }

    #[test]
    fn pitch_shift_preserves_length() {
        let input = sine(4_000, 440.0, 48_000.0);
        for semis in [-12.0, -5.0, 5.0, 12.0] {
            let out = pitch_shift(&input, semis);
            let ratio = out.len() as f64 / input.len() as f64;
            assert!((ratio - 1.0).abs() < 0.05, "semitones {semis}, ratio {ratio}");
            assert!(out.iter().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn zero_semitones_is_a_copy() {
        let input = sine(100, 440.0, 48_000.0);
        assert_eq!(pitch_shift(&input, 0.0), input);
    }

    #[test]
    fn an_empty_input_is_handled() {
        assert!(time_stretch(&[], 2.0).is_empty());
        assert!(pitch_shift(&[], 3.0).is_empty());
        assert!(resample_linear(&[], 2.0).is_empty());
    }
}

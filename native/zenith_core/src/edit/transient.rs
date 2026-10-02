//! Transient detection and slice mapping (PLAN §3.S8).
//!
//! # Detection
//!
//! The signal is split into short frames, each frame's mean-square energy is
//! computed, and a transient is marked where a frame's energy exceeds the
//! running **median** energy by more than the configured threshold. This is a
//! standard, robust percussion detector and it needs no FFT, which keeps it
//! affordable on `wasm32`.
//!
//! # Why energy-vs-median, not flux-vs-mean
//!
//! Comparing frame energy against a local median makes the detector sensitive
//! to *onsets above the local level*, while a sustained tone — whose frame
//! energy only ripples by a fraction of a dB — stays quiet. A plain energy
//! derivative would fire on that ripple, and a mean (rather than median)
//! reference would be dragged upward by the very transients being detected, so
//! a loud passage would raise its own bar until its onsets stopped matching.

use alloc::vec::Vec;

/// Configuration for [`detect_transients`].
#[derive(Debug, Clone, Copy)]
pub struct TransientConfig {
    /// Analysis frame length, in samples.
    pub frame: usize,
    /// Hop between frames, in samples.
    pub hop: usize,
    /// How far above the local median the energy change must be, in dB.
    pub threshold_db: f32,
    /// Minimum number of frames between two transients, to avoid double-hits.
    pub min_gap_frames: usize,
    /// Half-width of the median window, in frames.
    pub median_window: usize,
}

impl Default for TransientConfig {
    fn default() -> Self {
        Self {
            frame: 1_024,
            hop: 256,
            threshold_db: 6.0,
            min_gap_frames: 3,
            median_window: 12,
        }
    }
}

/// Detects transient positions, in samples, within `input`.
///
/// The returned positions are frame-aligned starts sorted ascending. A silent
/// input yields no transients.
#[must_use]
pub fn detect_transients(input: &[f32]) -> Vec<usize> {
    detect_transients_with(input, TransientConfig::default())
}

/// [`detect_transients`] with explicit configuration.
#[must_use]
pub fn detect_transients_with(input: &[f32], config: TransientConfig) -> Vec<usize> {
    if input.len() < config.frame || config.hop == 0 {
        return Vec::new();
    }
    let frames = (input.len() - config.frame) / config.hop + 1;
    if frames == 0 {
        return Vec::new();
    }

    // Per-frame energy (mean square).
    let mut energy = alloc::vec![0.0f32; frames];
    for (f, e) in energy.iter_mut().enumerate() {
        let start = f * config.hop;
        let mut sum = 0.0f32;
        for s in &input[start..start + config.frame] {
            sum += s * s;
        }
        *e = sum / config.frame as f32;
    }

    let gain = 10f32.powf(config.threshold_db / 10.0);
    // A floor below which "energy" is numerical noise, so a digitally silent
    // region is never treated as having onsets.
    const ENERGY_FLOOR: f32 = 1e-12;

    let mut transients: Vec<usize> = Vec::new();
    let mut last_frame: isize = -(config.min_gap_frames as isize) - 1;

    for f in 1..frames {
        // Reference level: the local median frame energy. A median (not a mean)
        // is what keeps a loud passage from raising its own detection bar until
        // its onsets stop registering.
        let lo = f.saturating_sub(config.median_window);
        let hi = (f + config.median_window + 1).min(frames);
        let mut window: Vec<f32> = energy[lo..hi].to_vec();
        window.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let median = window[window.len() / 2];
        let reference = median.max(ENERGY_FLOOR) * gain;

        // A transient is a frame whose energy exceeds the local reference by the
        // threshold. Comparing *energy* to a local energy median — rather than a
        // flux to a flux median — is what makes a sustained tone, whose frame
        // energy only ripples by a fraction of a dB, stay quiet.
        if energy[f] > reference && (f as isize - last_frame) >= config.min_gap_frames as isize {
            transients.push(f * config.hop);
            last_frame = f as isize;
        }
    }
    transients
}

/// Slices `input` at `positions`, returning the segments between them.
///
/// `positions` should be sorted ascending and start with `0`; the first segment
/// begins at 0 regardless, so a caller need not prepend it. Each segment runs to
/// the next position, with the final segment ending at `input.len()`. Empty
/// segments (two cuts at the same sample) are dropped.
#[must_use]
pub fn slice_at(input: &[f32], positions: &[usize]) -> Vec<Vec<f32>> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut cuts: Vec<usize> = positions.iter().copied().filter(|p| *p <= input.len()).collect();
    if cuts.first() != Some(&0) {
        cuts.insert(0, 0);
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut segments = Vec::new();
    for (i, &start) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).copied().unwrap_or(input.len());
        if end > start {
            segments.push(input[start..end].to_vec());
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signal of silence with impulses at the given sample positions.
    fn pulses(len: usize, positions: &[usize]) -> Vec<f32> {
        let mut v = alloc::vec![0.0f32; len];
        for &p in positions {
            if p < len {
                v[p] = 1.0;
            }
        }
        v
    }

    #[test]
    fn silence_has_no_transients() {
        let input = alloc::vec![0.0f32; 8_000];
        assert!(detect_transients(&input).is_empty());
    }

    #[test]
    fn pulses_are_detected_near_their_positions() {
        let input = pulses(16_000, &[0, 4_000, 8_000, 12_000]);
        let transients = detect_transients(&input);
        assert!(transients.len() >= 3, "found {:?}", transients);
        // Each detected position should be within one analysis frame of a pulse.
        for &expected in &[4_000usize, 8_000, 12_000] {
            let near = transients
                .iter()
                .any(|&t| (t as i64 - expected as i64).abs() < 1_024);
            assert!(near, "no transient near {expected} in {transients:?}");
        }
    }

    #[test]
    fn a_sustained_tone_does_not_trigger_repeatedly() {
        // A constant-amplitude tone has no energy change after onset, so the
        // min-gap logic plus a flat flux should not fire per frame.
        let input: Vec<f32> = (0..16_000)
            .map(|i| {
                crate::effects::util::dsp::sin_poly(core::f32::consts::TAU * 440.0 * i as f32 / 48_000.0)
            })
            .collect();
        let transients = detect_transients(&input);
        assert!(
            transients.len() <= 2,
            "a steady tone should yield at most an onset, got {transients:?}"
        );
    }

    #[test]
    fn slicing_between_cuts_produces_the_segments() {
        let input: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let segments = slice_at(&input, &[0, 3, 7]);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], alloc::vec![0.0, 1.0, 2.0]);
        assert_eq!(segments[1], alloc::vec![3.0, 4.0, 5.0, 6.0]);
        assert_eq!(segments[2], alloc::vec![7.0, 8.0, 9.0]);
    }

    #[test]
    fn slicing_without_a_leading_zero_prepends_it() {
        let input: Vec<f32> = (0..6).map(|i| i as f32).collect();
        let segments = slice_at(&input, &[4]);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].len(), 4);
        assert_eq!(segments[1].len(), 2);
    }

    #[test]
    fn slicing_an_empty_signal_yields_nothing() {
        assert!(slice_at(&[], &[0, 1]).is_empty());
    }

    #[test]
    fn duplicate_cuts_do_not_produce_empty_segments() {
        let input: Vec<f32> = (0..6).map(|i| i as f32).collect();
        let segments = slice_at(&input, &[0, 0, 3, 3]);
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().all(|s| !s.is_empty()));
    }
}

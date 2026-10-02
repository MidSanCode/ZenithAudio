//! Per-block render context shared by the engine's DSP nodes.
//!
//! This is the engine-level counterpart to
//! [`crate::effects::buffer::RenderContext`]. It carries the same musical
//! information but is defined here so the engine's node trait does not depend on
//! the effect suite's storage types.

/// Information every engine DSP node needs but none of them owns.
///
/// Passed by value (it is small and `Copy`) so a node cannot hold a reference
/// into engine state across blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderContext {
    /// Sample rate in hertz.
    pub sample_rate: f32,
    /// Transport position at the start of the block, in frames.
    pub frame: i64,
    /// Tempo in beats per minute.
    pub bpm: f32,
    /// Pulses per quarter note.
    pub ppq: u32,
    /// Frames in this block.
    pub frames: usize,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            frame: 0,
            bpm: 120.0,
            ppq: 960,
            frames: 0,
        }
    }
}

impl RenderContext {
    /// Creates a context for one block.
    #[must_use]
    pub const fn new(sample_rate: f32, frames: usize, frame: i64, bpm: f32, ppq: u32) -> Self {
        Self {
            sample_rate,
            frame,
            bpm,
            ppq,
            frames,
        }
    }

    /// Block duration in seconds.
    #[must_use]
    pub fn block_seconds(&self) -> f32 {
        if self.sample_rate > 0.0 {
            self.frames as f32 / self.sample_rate
        } else {
            0.0
        }
    }

    /// Converts a duration in beats to a number of samples.
    #[must_use]
    pub fn beats_to_samples(&self, beats: f32) -> f32 {
        if self.bpm <= 0.0 || self.sample_rate <= 0.0 {
            return 0.0;
        }
        beats * 60.0 / self.bpm * self.sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_seconds_and_beats_convert_sensibly() {
        let ctx = RenderContext::new(48_000.0, 256, 0, 120.0, 960);
        assert!((ctx.block_seconds() - 256.0 / 48_000.0).abs() < 1e-9);
        // At 120 BPM a beat is 0.5 s = 24 000 samples.
        assert!((ctx.beats_to_samples(1.0) - 24_000.0).abs() < 1e-3);
    }

    #[test]
    fn a_zero_sample_rate_does_not_divide_by_zero() {
        let ctx = RenderContext::new(0.0, 256, 0, 120.0, 960);
        assert_eq!(ctx.block_seconds(), 0.0);
        assert_eq!(ctx.beats_to_samples(1.0), 0.0);
    }
}

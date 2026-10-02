//! Plugin/effect delay compensation (PDC): aligning channels that carry
//! latency-bearing effects (PLAN §3.S4 item 1).
//!
//! # The problem
//!
//! A convolution reverb, a look-ahead compressor or an oversampled saturator
//! delays its channel by N samples. If channel A runs through such an effect
//! and channel B does not, the two are no longer sample-aligned — the mix
//! develops a comb-filtered, phasey character and a transient no longer lines
//! up with its own reverb.
//!
//! # The fix: relative alignment
//!
//! Every channel is delayed until it lines up with the **deepest** channel, by
//! `max_latency - channel_latency` samples. The absolute pipeline latency is
//! then `max_latency` for every channel, so nothing is misaligned relative to
//! anything else.
//!
//! Absolute latency is deliberately *not* removed: the offline renderer runs
//! the exact same graph, so it has the same absolute delay, and the two agree
//! sample for sample (PLAN §3.S4 item 5). Removing it in one path and not the
//! other is exactly the drift that requirement forbids.
//!
//! # Control thread vs audio thread
//!
//! [`PdcPlan::recompute`] allocates the delay lines and is **control-thread
//! only**; it is called alongside [`crate::engine::EffectRack::sync`].
//! [`PdcPlan::apply`] is the audio-thread path and allocates nothing.

use alloc::vec::Vec;

/// A per-channel fractional-free delay line.
///
/// Whole-sample only: PDC compensates reported integer latencies, and an
/// effect that reports a fractional latency is reporting a bug.
struct DelayLine {
    /// Interleaved stereo storage, `channels * capacity`.
    buffer: Vec<f32>,
    /// Frames the storage holds.
    capacity: usize,
    /// Next write frame index.
    write: usize,
    /// Frames of delay, `0..capacity`.
    delay: usize,
    /// Number of channels stored (always 2 for the engine).
    channels: usize,
}

impl DelayLine {
    fn new(channels: usize, capacity: usize) -> Self {
        Self {
            buffer: alloc::vec![0.0; channels * capacity.max(1)],
            capacity: capacity.max(1),
            write: 0,
            delay: 0,
            channels,
        }
    }

    /// Ensures the line can hold `delay + block` frames, reallocating if not.
    ///
    /// Control thread only.
    fn resize(&mut self, delay: usize, block: usize) {
        let needed = delay + block + 1;
        if needed > self.capacity {
            self.capacity = needed;
            self.buffer = alloc::vec![0.0; self.channels * needed];
            self.write = 0;
        }
        self.delay = delay.min(self.capacity.saturating_sub(1));
    }

    fn reset(&mut self) {
        self.buffer.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }

    /// Delays one interleaved stereo block in place.
    ///
    /// Real-time safe. With `delay == 0` the input is passed through unchanged
    /// and no history is written, so an un-compensated channel costs nothing.
    fn process(&mut self, block: &mut [f32], frames: usize) {
        if self.delay == 0 || self.channels == 0 {
            return;
        }
        let n = frames.min(block.len() / self.channels).min(self.capacity);
        for i in 0..n {
            let read = (self.write + self.capacity - self.delay) % self.capacity;
            for c in 0..self.channels {
                let input = block[i * self.channels + c];
                let delayed = self.buffer[read * self.channels + c];
                self.buffer[self.write * self.channels + c] = input;
                block[i * self.channels + c] = delayed;
            }
            self.write = (self.write + 1) % self.capacity;
        }
        // Frames beyond `n` are left untouched (the caller only reads `frames`).
    }
}

/// The compensation delays for every channel.
pub struct PdcPlan {
    /// One delay line per channel id. Indexed by channel id; a channel that has
    /// been removed keeps a zero-delay line, which is a no-op.
    lines: Vec<DelayLine>,
    /// The deepest channel latency, in frames.
    max_latency: usize,
    /// Channels stored per line.
    channels: usize,
    /// Block size the plan was prepared for.
    block: usize,
}

impl PdcPlan {
    /// Creates an empty plan.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            max_latency: 0,
            channels: 2,
            block: 0,
        }
    }

    /// Prepares the plan for a block size.
    ///
    /// Control thread. Existing lines are kept but resized on demand.
    pub fn prepare(&mut self, block: usize, channels: usize) {
        self.block = block;
        self.channels = channels.max(1);
    }

    /// The deepest channel latency, in frames.
    #[must_use]
    pub const fn max_latency(&self) -> usize {
        self.max_latency
    }

    /// Recomputes each channel's compensation delay from its effect latency.
    ///
    /// `latencies[id]` is channel `id`'s total effect-chain latency. A channel
    /// with no entry (or `0`) is treated as zero-latency and gets the full
    /// `max_latency` of compensation.
    ///
    /// Control thread only: this allocates and resizes delay lines.
    pub fn recompute(&mut self, latencies: &[usize], live_channels: impl Iterator<Item = u32>) {
        self.max_latency = latencies.iter().copied().max().unwrap_or(0);

        // Make sure there is a line for every channel id we might see. Channels
        // are never reused, so indexing by id is stable.
        let highest = live_channels.max().unwrap_or(0) as usize;
        while self.lines.len() <= highest {
            self.lines.push(DelayLine::new(self.channels, self.block + 1));
        }

        for (id, line) in self.lines.iter_mut().enumerate() {
            let channel_latency = latencies.get(id).copied().unwrap_or(0);
            let compensation = self.max_latency.saturating_sub(channel_latency);
            line.resize(compensation, self.block);
        }
    }

    /// Applies channel `id`'s compensation delay to `block` in place.
    ///
    /// Real-time safe.
    pub fn apply(&mut self, id: u32, block: &mut [f32], frames: usize) {
        if let Some(line) = self.lines.get_mut(id as usize) {
            line.process(block, frames);
        }
    }

    /// Clears every line's history; call on seek.
    pub fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
    }
}

impl Default for PdcPlan {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_delay_is_a_pass_through() {
        let mut line = DelayLine::new(2, 16);
        line.resize(0, 8);
        let mut block = [1.0f32, 2.0, 3.0, 4.0];
        let before = block;
        line.process(&mut block, 2);
        assert_eq!(block, before, "zero delay must not change the signal");
    }

    #[test]
    fn a_delay_holds_the_signal_back_by_exactly_that_many_frames() {
        let mut line = DelayLine::new(2, 16);
        line.resize(2, 8);
        // Feed an impulse at frame 0.
        let mut block = [1.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        line.process(&mut block, 4);
        // With a 2-frame delay the first two frames are silent and the impulse
        // emerges at frames 2 and 3 (interleaved: indices 4..8).
        assert_eq!(block[0], 0.0);
        assert_eq!(block[1], 0.0);
        assert_eq!(block[4], 1.0, "impulse should emerge at frame 2");
        assert_eq!(block[5], 1.0);
    }

    #[test]
    fn recompute_aligns_a_fast_channel_to_a_slow_one() {
        // Channel 0 has 4 samples of latency, channel 1 has none. Channel 1
        // must be delayed 4 to line up with channel 0; channel 0 is not delayed.
        let mut plan = PdcPlan::new();
        plan.prepare(8, 2);
        let latencies = [4usize, 0usize];
        plan.recompute(&latencies, [0u32, 1].into_iter());
        assert_eq!(plan.max_latency(), 4);

        // Channel 0 is the deepest, so it gets no compensation: an impulse
        // passes straight through.
        let mut block0 = [0.0f32; 16];
        block0[0] = 1.0;
        block0[1] = 1.0;
        plan.apply(0, &mut block0, 8);
        assert_eq!(block0[0], 1.0, "the deepest channel must not be delayed");

        // Channel 1 is 4 samples early, so it is delayed by 4. Feed an impulse
        // and confirm it emerges in the fifth frame (interleaved index 8).
        let mut block = [0.0f32; 16];
        block[0] = 1.0;
        block[1] = 1.0;
        plan.apply(1, &mut block, 8);
        assert_eq!(
            &block[0..8],
            &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "the first four frames must be silent"
        );
        assert_eq!(block[8], 1.0, "channel 1 must emerge exactly 4 frames late");
        assert_eq!(block[9], 1.0);
    }

    #[test]
    fn the_deepest_channel_is_not_delayed_but_others_are() {
        let mut plan = PdcPlan::new();
        plan.prepare(4, 2);
        let latencies = [10usize, 4usize, 0usize];
        plan.recompute(&latencies, [0u32, 1, 2].into_iter());
        assert_eq!(plan.max_latency(), 10);
        // Channel 0 gets 0 compensation, channel 1 gets 6, channel 2 gets 10.
        // Observable: a channel-2 impulse is delayed by 10 frames.
        let mut block = [0.0f32; 8];
        block[0] = 1.0;
        block[1] = 1.0;
        plan.apply(2, &mut block, 4);
        assert_eq!(block[0], 0.0);
    }

    #[test]
    fn an_out_of_range_channel_id_is_ignored() {
        let mut plan = PdcPlan::new();
        plan.prepare(8, 2);
        plan.recompute(&[0], [0u32].into_iter());
        let mut block = [1.0f32, 1.0];
        plan.apply(999, &mut block, 1); // must not panic
        assert_eq!(block[0], 1.0);
    }
}

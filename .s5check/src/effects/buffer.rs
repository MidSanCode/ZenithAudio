//! The audio buffer handed to effects, plus the per-block render context.
//!
//! # Why a custom buffer instead of `Vec<f32>`
//!
//! Effects must be able to process **in place** without allocating. A
//! `Vec<Vec<f32>>` would invite per-block allocation and hides the stride, so
//! the buffer here is a borrowed, non-owning view: a pointer to an interleaved
//! or planar block plus its shape.
//!
//! # Layout
//!
//! Storage is **planar**: one contiguous region per channel
//! (`channel c` occupies `data[c * stride .. c * stride + frames]`). Planar is
//! the right default for a mixing engine because per-channel DSP (a filter, a
//! compressor) then walks a contiguous array, which is what vectorizes; an
//! interleaved layout would force every effect to stride by the channel count.
//!
//! # Real-time safety
//!
//! Nothing in this module allocates or locks. It is a view type: `prepare`
//! allocates the *storage* once on the control thread, and every subsequent
//! block just re-points a slice at it.

/// A borrowed, planar block of `f32` audio.
///
/// The buffer does not own its samples. It borrows from a [`SampleStorage`]
/// owned by the engine, so effects cannot outlive their audio.
#[derive(Debug)]
pub struct AudioBuffer<'a> {
    /// One slice per channel, each exactly `frames` long.
    channels: &'a mut [&'a mut [f32]],
    /// Number of frames in this block.
    frames: usize,
}

impl<'a> AudioBuffer<'a> {
    /// Builds a buffer over `channels`.
    ///
    /// # Panics
    ///
    /// Panics when the slices do not all have the same length. A ragged block
    /// is a programming error in the engine, not a user-reachable condition,
    /// and silently truncating to the shortest channel would produce a
    /// click at the block boundary that is very hard to trace back here.
    #[must_use]
    pub fn new(channels: &'a mut [&'a mut [f32]]) -> Self {
        let frames = channels.first().map_or(0, |c| c.len());
        assert!(
            channels.iter().all(|c| c.len() == frames),
            "AudioBuffer requires every channel to have the same length"
        );
        Self { channels, frames }
    }

    /// Number of frames in this block.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// Number of channels in this block.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Whether this block carries no frames.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Immutable access to channel `index`, if it exists.
    #[must_use]
    pub fn channel(&self, index: usize) -> Option<&[f32]> {
        self.channels.get(index).map(|c| &c[..])
    }

    /// Mutable access to channel `index`, if it exists.
    pub fn channel_mut(&mut self, index: usize) -> Option<&mut [f32]> {
        self.channels.get_mut(index).map(|c| &mut c[..])
    }

    /// Iterates the channels immutably.
    pub fn iter(&self) -> impl Iterator<Item = &[f32]> + '_ {
        self.channels.iter().map(|c| &c[..])
    }

    /// Iterates the channels mutably.
    ///
    /// Returns `core::slice::IterMut` rather than `impl Iterator`: the
    /// anonymous form cannot express that the inner `&mut [f32]` borrows for
    /// as long as the outer `&mut self`, and the compiler rejects it. Naming
    /// the concrete type is what makes the borrow legitimate.
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, &'a mut [f32]> {
        self.channels.iter_mut()
    }
    /// Applies `gain` to every sample of every channel.
    ///
    /// A convenience for wet/dry mixing that keeps the loop in one place.
    pub fn apply_gain(&mut self, gain: f32) {
        if gain == 1.0 {
            return;
        }
        for channel in self.channels.iter_mut() {
            for sample in channel.iter_mut() {
                *sample *= gain;
            }
        }
    }

    /// Replaces non-finite samples with `0.0`, returning how many were fixed.
    ///
    /// Effects that contain feedback (reverb, delay, filter with resonance) can
    /// go non-finite if a parameter is driven to an extreme. A single `NaN`
    /// silences the whole bus and is effectively impossible to trace from the
    /// speaker back to its source, so the engine sanitises at the effect
    /// boundary and reports the count for diagnostics.
    pub fn sanitize(&mut self) -> usize {
        let mut repaired = 0;
        for channel in self.channels.iter_mut() {
            for sample in channel.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                    repaired += 1;
                }
            }
        }
        repaired
    }

    /// Copies every channel of `source` into `self`.
    ///
    /// Used by effects that need a dry copy for their wet/dry mix. Returns
    /// `false` when the shapes differ, rather than copying a partial block.
    pub fn copy_from(&mut self, source: &AudioBuffer<'_>) -> bool {
        if self.channel_count() != source.channel_count() || self.frames != source.frames {
            return false;
        }
        for (dst, src) in self.channels.iter_mut().zip(source.iter()) {
            dst[..src.len()].copy_from_slice(src);
        }
        true
    }
}

/// Owned, preallocated storage for one effect's working buffers.
///
/// Allocated once in [`crate::effects::EffectProcessor::prepare`] on the
/// control thread. `process` never resizes it.
///
/// The storage is *planar* to match [`AudioBuffer`]: one `Vec` holding
/// `channels * capacity` samples, sliced per channel. Using a single
/// allocation rather than `Vec<Vec<f32>>` means the per-channel slices stay
/// contiguous in memory and there is exactly one reallocation to reason about.
#[derive(Debug, Default, Clone)]
pub struct SampleStorage {
    /// `channels * capacity` samples, laid out channel-major.
    data: alloc::vec::Vec<f32>,
    /// Number of channels.
    channels: usize,
    /// Frames per channel.
    capacity: usize,
}

impl SampleStorage {
    /// Allocates storage for `channels * capacity` samples.
    #[must_use]
    pub fn new(channels: usize, capacity: usize) -> Self {
        Self {
            data: alloc::vec![0.0; channels * capacity],
            channels,
            capacity,
        }
    }

    /// The number of channels this storage was sized for.
    #[must_use]
    pub const fn channel_count(&self) -> usize {
        self.channels
    }

    /// The number of frames per channel this storage was sized for.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether `channels`/`capacity` fit within this storage.
    #[must_use]
    pub const fn fits(&self, channels: usize, capacity: usize) -> bool {
        channels <= self.channels && capacity <= self.capacity
    }

    /// Reallocates when the requested shape does not fit.
    ///
    /// Called from `prepare` only — never from `process`.
    pub fn resize(&mut self, channels: usize, capacity: usize) {
        if !self.fits(channels, capacity) {
            self.channels = channels;
            self.capacity = capacity;
            self.data = alloc::vec![0.0; channels * capacity];
        }
    }

    /// Zeroes every sample.
    pub fn clear(&mut self) {
        self.data.iter_mut().for_each(|s| *s = 0.0);
    }

    /// Borrows channel `index` for `frames` samples, if in range.
    #[must_use]
    pub fn channel(&self, index: usize, frames: usize) -> Option<&[f32]> {
        if index >= self.channels || frames > self.capacity {
            return None;
        }
        let start = index * self.capacity;
        self.data.get(start..start + frames)
    }

    /// Mutably borrows channel `index` for `frames` samples, if in range.
    pub fn channel_mut(&mut self, index: usize, frames: usize) -> Option<&mut [f32]> {
        if index >= self.channels || frames > self.capacity {
            return None;
        }
        let start = index * self.capacity;
        self.data.get_mut(start..start + frames)
    }

    /// Splits out `channels` mutable slices of `frames` samples each.
    ///
    /// This is the bridge from owned storage to [`AudioBuffer`]. Returns `None`
    /// when the requested shape does not fit, so a mis-sized request cannot
    /// panic on a slice bound in the audio thread.
    ///
    /// The split is done with `chunks_mut` over the whole region rather than by
    /// indexing `data` once per channel: indexing in a loop creates `channels`
    /// overlapping mutable borrows of the same `Vec`, which the borrow checker
    /// rejects (correctly — it is the shape of a real aliasing bug).
    pub fn as_channel_views(
        &mut self,
        channels: usize,
        frames: usize,
    ) -> Option<alloc::vec::Vec<&mut [f32]>> {
        if !self.fits(channels, frames) {
            return None;
        }
        let stride = self.capacity;
        // Each chunk is one channel's region; truncate it to `frames`.
        let mut views = alloc::vec::Vec::with_capacity(channels);
        for chunk in self.data.chunks_mut(stride).take(channels) {
            views.push(chunk.get_mut(..frames)?);
        }
        Some(views)
    }
}

/// Per-block information every effect needs but none of them owns.
///
/// Passed by value (it is small and `Copy`) so an effect cannot hold a
/// reference into engine state across blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderContext {
    /// Sample rate in hertz.
    pub sample_rate: f32,
    /// Transport position at the *start* of this block, in frames.
    ///
    /// Effects that must align to musical time (a tempo-synced delay) derive
    /// their delay length from the tempo below rather than from this offset;
    /// it is provided for effects that follow the transport (a tape stop).
    pub frame: i64,
    /// Tempo in beats per minute, for tempo-synced effects.
    pub bpm: f32,
    /// Pulses per quarter note, so a delay can convert beats to samples
    /// without a second round trip through the transport.
    pub ppq: u32,
    /// Total frames in this block.
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
    /// Creates a context for a block.
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

    /// Block duration in milliseconds.
    #[must_use]
    pub fn block_ms(&self) -> f32 {
        self.block_seconds() * 1000.0
    }

    /// Converts a musical duration in beats to a number of samples.
    ///
    /// This is the one place the tempo-to-samples conversion lives, so a
    /// tempo-synced delay and a tempo-synced LFO cannot disagree.
    #[must_use]
    pub fn beats_to_samples(&self, beats: f32) -> f32 {
        if self.bpm <= 0.0 || self.sample_rate <= 0.0 {
            return 0.0;
        }
        beats * 60.0 / self.bpm * self.sample_rate
    }

    /// A context with the transport values replaced, for tests and for
    /// offline rendering where the tempo may differ from the live session.
    #[must_use]
    pub const fn with_tempo(self, bpm: f32, ppq: u32) -> Self {
        Self { bpm, ppq, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_exposes_its_shape_and_channels() {
        let mut left = [1.0_f32, 2.0, 3.0, 4.0];
        let mut right = [5.0_f32, 6.0, 7.0, 8.0];
        let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
        let buffer = AudioBuffer::new(&mut channels);

        assert_eq!(buffer.frames(), 4);
        assert_eq!(buffer.channel_count(), 2);
        assert!(!buffer.is_empty());
        assert_eq!(buffer.channel(0), Some(&[1.0, 2.0, 3.0, 4.0][..]));
        assert_eq!(buffer.channel(1), Some(&[5.0, 6.0, 7.0, 8.0][..]));
        assert_eq!(buffer.channel(2), None);
    }

    #[test]
    fn applying_gain_scales_every_sample() {
        let mut left = [1.0_f32, -2.0];
        let mut right = [0.5_f32, 4.0];
        let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
        let mut buffer = AudioBuffer::new(&mut channels);

        buffer.apply_gain(0.5);
        assert_eq!(buffer.channel(0), Some(&[0.5, -1.0][..]));
        assert_eq!(buffer.channel(1), Some(&[0.25, 2.0][..]));

        // Unity gain is a no-op and must not perturb the samples.
        buffer.apply_gain(1.0);
        assert_eq!(buffer.channel(0), Some(&[0.5, -1.0][..]));
    }

    #[test]
    fn sanitize_replaces_non_finite_samples_only() {
        let mut left = [f32::NAN, 1.0, f32::INFINITY, f32::NEG_INFINITY];
        let mut channels: [&mut [f32]; 1] = [&mut left];
        let mut buffer = AudioBuffer::new(&mut channels);

        assert_eq!(buffer.sanitize(), 3);
        assert_eq!(buffer.channel(0), Some(&[0.0, 1.0, 0.0, 0.0][..]));
    }

    #[test]
    fn copy_from_refuses_mismatched_shapes() {
        let mut a = [0.0_f32; 4];
        let mut b = [1.0_f32; 4];
        let mut c = [2.0_f32; 8];
        let mut src_channels: [&mut [f32]; 1] = [&mut b];
        let source = AudioBuffer::new(&mut src_channels);

        {
            let mut dst_channels: [&mut [f32]; 1] = [&mut a];
            let mut dest = AudioBuffer::new(&mut dst_channels);
            assert!(dest.copy_from(&source));
            assert_eq!(dest.channel(0), Some(&[1.0, 1.0, 1.0, 1.0][..]));
        }

        // A different frame count must be refused outright, not partially
        // copied: a half-filled wet buffer would be an audible glitch.
        let mut wide_channels: [&mut [f32]; 1] = [&mut c];
        let mut wide = AudioBuffer::new(&mut wide_channels);
        assert!(!wide.copy_from(&source));
        assert_eq!(wide.channel(0), Some(&[2.0; 8][..]));
    }

    #[test]
    fn storage_yields_channel_views_that_fit() {
        let mut storage = SampleStorage::new(2, 8);
        assert_eq!(storage.channel_count(), 2);
        assert_eq!(storage.capacity(), 8);
        assert!(storage.fits(2, 8));
        assert!(!storage.fits(3, 8));
        assert!(!storage.fits(2, 9));

        {
            let mut views = storage.as_channel_views(2, 4).expect("shape fits");
            assert_eq!(views.len(), 2);
            assert_eq!(views[0].len(), 4);
            // The two channels must be distinct regions, or an effect would
            // process the same memory twice.
            views[0][0] = 1.0;
            views[1][0] = 2.0;
        }
        assert_eq!(storage.channel(0, 1), Some(&[1.0][..]));
        assert_eq!(storage.channel(1, 1), Some(&[2.0][..]));
    }

    #[test]
    fn storage_refuses_a_shape_it_cannot_serve() {
        let mut storage = SampleStorage::new(2, 4);
        assert!(storage.as_channel_views(4, 4).is_none());
        assert!(storage.as_channel_views(2, 5).is_none());
        assert!(storage.channel(4, 4).is_none());
        assert!(storage.channel_mut(0, 5).is_none());
    }

    #[test]
    fn resizing_grows_but_never_shrinks_a_sufficient_region() {
        let mut storage = SampleStorage::new(2, 4);
        // A request that fits must leave the existing samples alone.
        storage.channel_mut(0, 1).expect("in range")[0] = 9.0;
        storage.resize(2, 4);
        assert_eq!(storage.channel(0, 1), Some(&[9.0][..]));

        // A request that does not fit reallocates (and zero-fills).
        storage.resize(4, 8);
        assert_eq!(storage.channel_count(), 4);
        assert_eq!(storage.capacity(), 8);
        assert_eq!(storage.channel(0, 1), Some(&[0.0][..]));
    }

    #[test]
    fn clearing_zeroes_every_sample() {
        let mut storage = SampleStorage::new(1, 4);
        storage.channel_mut(0, 4).expect("in range")[0] = 5.0;
        storage.clear();
        assert_eq!(storage.channel(0, 4), Some(&[0.0, 0.0, 0.0, 0.0][..]));
    }

    #[test]
    fn beats_to_samples_follows_the_tempo() {
        let ctx = RenderContext::new(48_000.0, 256, 0, 120.0, 960);
        // At 120 BPM one beat is 0.5 s = 24 000 samples.
        assert!((ctx.beats_to_samples(1.0) - 24_000.0).abs() < 1e-3);
        // A quarter-note triplet is 2/3 of a beat.
        assert!((ctx.beats_to_samples(2.0 / 3.0) - 16_000.0).abs() < 1e-3);
        // Half-time feels half as fast.
        let slow = ctx.with_tempo(60.0, 960);
        assert!((slow.beats_to_samples(1.0) - 48_000.0).abs() < 1e-3);
    }

    #[test]
    fn a_degenerate_tempo_yields_zero_rather_than_infinity() {
        // Guarding here keeps "divide by bpm" out of every tempo-synced effect.
        let ctx = RenderContext::new(48_000.0, 256, 0, 0.0, 960);
        assert_eq!(ctx.beats_to_samples(1.0), 0.0);
        let silent = RenderContext::new(0.0, 256, 0, 120.0, 960);
        assert_eq!(silent.block_seconds(), 0.0);
        assert_eq!(silent.beats_to_samples(1.0), 0.0);
    }

    #[test]
    fn block_duration_is_reported_in_seconds_and_milliseconds() {
        let ctx = RenderContext::new(48_000.0, 256, 0, 120.0, 960);
        assert!((ctx.block_seconds() - 256.0 / 48_000.0).abs() < 1e-9);
        assert!((ctx.block_ms() - 5.333_333).abs() < 1e-3);
    }

    #[test]
    fn an_empty_block_is_still_usable() {
        let mut empty: [&mut [f32]; 0] = [];
        let buffer = AudioBuffer::new(&mut empty);
        assert!(buffer.is_empty());
        assert_eq!(buffer.frames(), 0);
        assert_eq!(buffer.channel_count(), 0);
        assert_eq!(buffer.channel(0), None);
    }

    #[test]
    #[should_panic(expected = "same length")]
    fn a_ragged_block_is_rejected_rather_than_truncated() {
        let mut left = [0.0_f32; 4];
        let mut right = [0.0_f32; 3];
        let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
        let _ = AudioBuffer::new(&mut channels);
    }
}

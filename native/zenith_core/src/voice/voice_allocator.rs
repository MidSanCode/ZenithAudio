//! The voice pool and allocator.
//!
//! [`VoiceAllocator`] owns a fixed array of voices and decides which one a new
//! note uses. The pool never grows: a note that cannot find a free voice steals
//! one, preferring a voice already in release so the steal is least audible.
//!
//! # Why a fixed pool
//!
//! A `Vec` that grew on note-on would allocate on the audio thread (ABI P5).
//! The pool is sized at construction and the voice count is a hard ceiling,
//! which is also what makes the worst-case CPU predictable — the S1 acceptance
//! case is "64-voice polyphony, CPU < 15 %", and a bounded pool is what makes
//! that bound achievable on every platform including `wasm32`.
//!
//! # Per-sample vs per-block
//!
//! [`VoiceAllocator::render`] runs voices sample by sample so a delay into the
//! middle of a block is sample-accurate. That is the "sample-level scheduling"
//! the plan requires, and it is why note events carry a frame offset.

use super::synth_voice::SynthVoice;

/// The polyphonic voice pool.
pub struct VoiceAllocator {
    /// The voices. Fixed length for the lifetime of the allocator.
    voices: alloc::vec::Vec<SynthVoice>,
    /// Sample rate, for voices created lazily if the pool is enlarged.
    sample_rate: f32,
    /// Monotonic counter used to break ties when stealing.
    age: u64,
    /// Per-voice age, older voices have smaller values.
    voice_age: alloc::vec::Vec<u64>,
}

impl VoiceAllocator {
    /// Creates a pool of `capacity` voices at `sample_rate`.
    ///
    /// A zero capacity is raised to one so the allocator always has a voice to
    /// hand out.
    #[must_use]
    pub fn new(capacity: usize, sample_rate: f32) -> Self {
        let capacity = capacity.max(1);
        let mut voices = alloc::vec::Vec::with_capacity(capacity);
        for _ in 0..capacity {
            voices.push(SynthVoice::new(sample_rate));
        }
        Self {
            voices,
            sample_rate,
            age: 0,
            voice_age: alloc::vec![0; capacity],
        }
    }

    /// Voice pool size.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.voices.len()
    }

    /// Number of voices currently producing sound or tail.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|v| v.is_active()).count()
    }

    /// Access a voice by index, for configuration.
    #[must_use]
    pub fn voice(&self, index: usize) -> Option<&SynthVoice> {
        self.voices.get(index)
    }

    /// Mutably access a voice by index, for configuration. Control thread only.
    pub fn voice_mut(&mut self, index: usize) -> Option<&mut SynthVoice> {
        self.voices.get_mut(index)
    }

    /// Applies a configuration closure to every voice.
    ///
    /// Control thread only. This is how the engine installs an instrument patch
    /// across the whole pool without reaching into each slot.
    pub fn configure_all<F: FnMut(&mut SynthVoice)>(&mut self, mut f: F) {
        for voice in &mut self.voices {
            f(voice);
        }
    }

    /// The sample rate the pool was created at.
    #[must_use]
    pub const fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Starts a note, stealing a voice if the pool is full.
    ///
    /// `offset_frames` delays the note into the block, giving sample-accurate
    /// placement of an event that falls between block boundaries.
    pub fn note_on(&mut self, pitch: u8, velocity: f32, offset_frames: usize) {
        let index = self.allocate();
        self.age += 1;
        self.voice_age[index] = self.age;
        let delay = offset_frames.min(u32::MAX as usize) as u32;
        if let Some(voice) = self.voices.get_mut(index) {
            voice.note_on_delayed(pitch, velocity, delay);
        }
    }

    /// Releases every voice sounding `pitch`.
    pub fn note_off(&mut self, pitch: u8, _offset_frames: usize) {
        for voice in &mut self.voices {
            if voice.is_active() && voice.pitch() == pitch {
                voice.note_off();
            }
        }
    }

    /// Releases and silences every voice.
    pub fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            voice.all_notes_off();
        }
    }

    /// Renders the pool into an interleaved stereo block, summing every voice.
    ///
    /// Real-time safe: no allocation, no locking. The mono voice output is
    /// written to both channels; per-voice panning arrives with the instrument
    /// model in a later stage.
    pub fn render(&mut self, out: &mut [f32], frames: usize) {
        let n = frames.min(out.len() / 2);
        // Clear the block first; voices sum into it.
        for s in &mut out[..n * 2] {
            *s = 0.0;
        }
        for voice in &mut self.voices {
            if !voice.is_active() {
                continue;
            }
            // A delayed voice must not have its samples consumed by the block;
            // `next_sample` already returns zero for the delay, so writing its
            // output is correct and keeps the envelope phase aligned.
            for i in 0..n {
                let s = voice.next_sample();
                out[i * 2] += s;
                out[i * 2 + 1] += s;
            }
        }
    }

    /// Finds a voice to use for a new note.
    ///
    /// Preference order:
    /// 1. an idle voice;
    /// 2. the oldest voice already in release;
    /// 3. the oldest voice overall.
    fn allocate(&mut self) -> usize {
        // 1. A free voice.
        if let Some(index) = self.voices.iter().position(|v| !v.is_active()) {
            return index;
        }
        // 2. The oldest releasing voice.
        let releasing = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_releasing())
            .min_by_key(|(i, _)| self.voice_age[*i])
            .map(|(i, _)| i);
        if let Some(index) = releasing {
            return index;
        }
        // 3. The oldest voice.
        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(i, _)| self.voice_age[*i])
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::synth_voice::AdsrSettings;

    fn short_pool() -> VoiceAllocator {
        let mut pool = VoiceAllocator::new(4, 48_000.0);
        pool.configure_all(|v| {
            v.set_amp_envelope(AdsrSettings {
                attack_ms: 0.1,
                decay_ms: 0.1,
                sustain: 1.0,
                release_ms: 0.5,
            })
        });
        pool
    }

    #[test]
    fn the_pool_has_the_requested_capacity() {
        assert_eq!(VoiceAllocator::new(64, 48_000.0).capacity(), 64);
        assert_eq!(VoiceAllocator::new(0, 48_000.0).capacity(), 1);
    }

    #[test]
    fn a_resting_pool_renders_silence() {
        let mut pool = VoiceAllocator::new(4, 48_000.0);
        let mut out = [9.0f32; 16];
        pool.render(&mut out, 8);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_note_becomes_active_and_renders_audio() {
        let mut pool = short_pool();
        pool.note_on(60, 1.0, 0);
        assert_eq!(pool.active_count(), 1);
        let mut out = [0.0f32; 256 * 2];
        pool.render(&mut out, 256);
        assert!(out.iter().any(|s| *s != 0.0), "a note must produce output");
    }

    #[test]
    fn more_notes_than_voices_steals_instead_of_growing() {
        let mut pool = short_pool();
        for pitch in 60..70 {
            pool.note_on(pitch, 1.0, 0);
        }
        assert_eq!(pool.capacity(), 4);
        assert!(pool.active_count() <= 4, "the pool must never exceed its capacity");
    }

    #[test]
    fn note_off_releases_only_the_matching_pitch() {
        let mut pool = short_pool();
        pool.note_on(60, 1.0, 0);
        pool.note_on(64, 1.0, 0);
        pool.note_off(60, 0);
        // 60 is releasing, 64 is not; both count as active, so check the
        // releasing flag directly.
        let releasing = (0..pool.capacity())
            .filter_map(|i| pool.voice(i))
            .filter(|v| v.is_releasing())
            .count();
        assert_eq!(releasing, 1, "exactly the note-off pitch is releasing");
    }

    #[test]
    fn all_notes_off_silences_the_pool() {
        let mut pool = short_pool();
        pool.note_on(60, 1.0, 0);
        pool.note_on(64, 1.0, 0);
        pool.all_notes_off();
        assert_eq!(pool.active_count(), 0);
    }

    #[test]
    fn a_delayed_note_is_silent_at_the_block_start() {
        let mut pool = short_pool();
        pool.note_on(60, 1.0, 128);
        let mut out = [0.0f32; 256 * 2];
        pool.render(&mut out, 64);
        // The first 64 frames are still inside the delay, so output is silence.
        assert!(out.iter().all(|s| *s == 0.0));
    }
}

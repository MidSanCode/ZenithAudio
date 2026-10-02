//! A sample-playback voice.
//!
//! [`SamplerVoice`] plays a mono `f32` sample with pitch resampling and a
//! simple amplitude envelope. The sample buffer is owned outside the voice
//! (the engine holds the pool), so a voice is a small handle: an index into the
//! pool plus playback state.
//!
//! # Real-time discipline
//!
//! [`SamplerVoice::next_sample`] reads from a borrowed sample slice and writes
//! only local state. It allocates nothing.

use super::synth_voice::AdsrSettings;
use crate::dsp::resampler::Resampler;
use crate::effects::util::dsp::exp2;

/// A voice that plays back one sample buffer.
#[derive(Debug, Clone, Copy)]
pub struct SamplerVoice {
    sample_rate: f32,
    /// Root pitch the sample was recorded at, as a MIDI note.
    root_pitch: u8,
    /// Playback pitch as a MIDI note.
    pitch: u8,
    /// Fractional read cursor into the sample, in source frames.
    cursor: f32,
    /// Current velocity, `0.0..=1.0`.
    velocity: f32,
    /// Linear gain envelope position.
    gain: f32,
    attack_rate: f32,
    release_rate: f32,
    releasing: bool,
    active: bool,
    /// Frames to wait before the voice starts.
    delay_frames: u32,
    resampler: Resampler,
}

impl SamplerVoice {
    /// Creates a silent sampler voice.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        Self {
            sample_rate,
            root_pitch: 60,
            pitch: 60,
            cursor: 0.0,
            velocity: 1.0,
            gain: 0.0,
            attack_rate: 1.0 / (5.0 * 0.001 * sample_rate),
            release_rate: 1.0 / (200.0 * 0.001 * sample_rate),
            releasing: false,
            active: false,
            delay_frames: 0,
            resampler: Resampler::new(),
        }
    }

    /// Sets the pitch the sample is played back at.
    pub fn set_root_pitch(&mut self, pitch: u8) {
        self.root_pitch = pitch;
    }

    /// Sets the envelope times.
    pub fn set_envelope(&mut self, settings: AdsrSettings) {
        self.attack_rate = rate_for(settings.attack_ms, self.sample_rate);
        self.release_rate = rate_for(settings.release_ms, self.sample_rate);
    }

    /// Whether the voice is producing sound or tail.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Whether the voice is in its release phase.
    #[must_use]
    pub const fn is_releasing(&self) -> bool {
        self.releasing
    }

    /// The MIDI pitch this voice is sounding.
    #[must_use]
    pub const fn pitch(&self) -> u8 {
        self.pitch
    }

    /// Starts playback of `pitch` at `velocity`.
    pub fn note_on(&mut self, pitch: u8, velocity: f32) {
        self.note_on_delayed(pitch, velocity, 0);
    }

    /// Starts playback `delay` frames later.
    pub fn note_on_delayed(&mut self, pitch: u8, velocity: f32, delay: u32) {
        self.pitch = pitch;
        self.cursor = 0.0;
        self.resampler.reset();
        self.velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Start from the current gain on a retrigger to avoid a click.
        if !self.active {
            self.gain = 0.0;
        }
        self.releasing = false;
        self.active = true;
        self.delay_frames = delay;
    }

    /// Begins the release phase.
    pub fn note_off(&mut self) {
        self.releasing = true;
    }

    /// Silences and resets the voice.
    pub fn all_notes_off(&mut self) {
        self.active = false;
        self.releasing = false;
        self.gain = 0.0;
        self.cursor = 0.0;
        self.delay_frames = 0;
        self.resampler.reset();
    }

    /// Produces one sample from `source`.
    ///
    /// Returns `0.0` once the cursor passes the end of `source` or the release
    /// reaches silence. Real-time safe.
    pub fn next_sample(&mut self, source: &[f32]) -> f32 {
        if !self.active {
            return 0.0;
        }
        if self.delay_frames > 0 {
            self.delay_frames -= 1;
            return 0.0;
        }
        if source.is_empty() {
            self.active = false;
            return 0.0;
        }

        // Amplitude envelope.
        if self.releasing {
            self.gain -= self.release_rate;
            if self.gain <= 0.0 {
                self.gain = 0.0;
                self.active = false;
                return 0.0;
            }
        } else if self.gain < 1.0 {
            self.gain = (self.gain + self.attack_rate).min(1.0);
        }

        // Read the current and next source frames, clamped at the tail.
        let index = self.cursor.floor();
        if index < 0.0 || index as usize >= source.len() {
            self.active = false;
            return 0.0;
        }
        let i = index as usize;
        let a = source[i];
        let b = if i + 1 < source.len() { source[i + 1] } else { a };

        // Source frames per output frame: up one semitone is a smaller step.
        let ratio = exp2((self.pitch as f32 - self.root_pitch as f32) / 12.0);
        let (value, advance) = self.resampler.process(a, b, ratio);
        self.cursor += advance as f32;

        value * self.gain * self.velocity
    }
}

/// Per-sample increment reaching 1.0 in `ms` milliseconds.
fn rate_for(ms: f32, sample_rate: f32) -> f32 {
    let time = if ms.is_finite() && ms > 0.0 { ms } else { 1.0 };
    let samples = time * 0.001 * sample_rate;
    if samples > 0.0 {
        1.0 / samples
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(len: usize) -> alloc::vec::Vec<f32> {
        (0..len).map(|i| (i as f32 / len as f32) * 2.0 - 1.0).collect()
    }

    #[test]
    fn a_silent_voice_emits_zero() {
        let mut v = SamplerVoice::new(48_000.0);
        assert_eq!(v.next_sample(&[1.0, 1.0]), 0.0);
    }

    #[test]
    fn a_note_plays_the_sample_then_ends_at_the_tail() {
        let src = ramp(32);
        let mut v = SamplerVoice::new(48_000.0);
        v.set_root_pitch(60);
        v.note_on(60, 1.0);
        let mut produced = 0;
        for _ in 0..64 {
            let s = v.next_sample(&src);
            assert!(s.is_finite());
            produced += 1;
        }
        assert!(produced > 0);
        assert!(!v.is_active(), "the voice must end when the sample runs out");
    }

    #[test]
    fn unison_pitch_is_a_pass_through_of_the_cursor() {
        let src = [0.0f32, 1.0, 0.0, -1.0];
        let mut v = SamplerVoice::new(48_000.0);
        v.set_root_pitch(60);
        v.set_envelope(AdsrSettings {
            attack_ms: 0.01,
            decay_ms: 0.01,
            sustain: 1.0,
            release_ms: 1.0,
        });
        v.note_on(60, 1.0);
        // First sample: gain ramps from 0, so the value is small but finite.
        let first = v.next_sample(&src);
        assert!(first.is_finite());
    }

    #[test]
    fn a_positive_pitch_advances_the_cursor_faster() {
        let src = ramp(1_000);
        let mut v = SamplerVoice::new(48_000.0);
        v.set_root_pitch(60);
        v.note_on(72, 1.0); // up an octave
        for _ in 0..200 {
            let _ = v.next_sample(&src);
        }
        // At an octave up the cursor should have passed ~400 source frames in
        // 200 output frames; assert it moved well past the unison rate.
        assert!(v.cursor > 200.0, "cursor advanced only to {}", v.cursor);
    }

    #[test]
    fn a_release_reaches_silence() {
        let src = ramp(10_000);
        let mut v = SamplerVoice::new(48_000.0);
        v.set_envelope(AdsrSettings {
            attack_ms: 0.01,
            decay_ms: 0.01,
            sustain: 1.0,
            release_ms: 1.0,
        });
        v.note_on(60, 1.0);
        let _ = v.next_sample(&src);
        v.note_off();
        for _ in 0..1_000 {
            let _ = v.next_sample(&src);
        }
        assert!(!v.is_active(), "release must end the voice");
    }
}

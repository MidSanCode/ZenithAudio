//! The built-in subtractive synthesis voice.
//!
//! One voice is one note: an oscillator into an amplitude envelope, through a
//! resonant state-variable filter that a second envelope sweeps. This is the
//! Rust port of the offline Dart synth the project shipped before S1, kept
//! intentionally small: the acceptance criterion is 64-note polyphony with a
//! predictable cost, not a modular synth.
//!
//! # Real-time discipline
//!
//! A voice owns no heap. [`SynthVoice::next_sample`] is a fixed sequence of
//! arithmetic and is safe to call from the audio thread. Parameters are plain
//! fields a control thread may overwrite between blocks.

use crate::dsp::svf::{Svf, SvfMode};
use crate::effects::util::dsp::exp2;

/// Oscillator waveforms the voice can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Oscillator {
    /// Pure sine.
    Sine,
    /// Naive falling-edge saw.
    #[default]
    Saw,
    /// 50 % duty square.
    Square,
    /// Triangle derived from the saw.
    Triangle,
}

impl Oscillator {
    /// Evaluates the waveform at `phase` in `0.0..1.0`.
    #[must_use]
    pub fn sample(self, phase: f32) -> f32 {
        let p = phase - phase.floor();
        match self {
            Self::Sine => crate::effects::util::dsp::sin_poly(core::f32::consts::TAU * p),
            Self::Saw => 2.0 * p - 1.0,
            Self::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Triangle => 4.0 * (p - 0.5).abs() - 1.0,
        }
    }
}

/// ADSR envelope settings, in milliseconds and a sustain level in `0.0..1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrSettings {
    /// Attack time in milliseconds.
    pub attack_ms: f32,
    /// Decay time in milliseconds.
    pub decay_ms: f32,
    /// Sustain level, `0.0..=1.0`.
    pub sustain: f32,
    /// Release time in milliseconds.
    pub release_ms: f32,
}

impl Default for AdsrSettings {
    fn default() -> Self {
        Self {
            attack_ms: 5.0,
            decay_ms: 120.0,
            sustain: 0.7,
            release_ms: 200.0,
        }
    }
}

/// The stage an ADSR envelope is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdsrStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A per-sample ADSR envelope.
#[derive(Debug, Clone, Copy)]
struct Adsr {
    settings: AdsrSettings,
    stage: AdsrStage,
    level: f32,
    sample_rate: f32,
}

impl Adsr {
    fn new(sample_rate: f32) -> Self {
        Self {
            settings: AdsrSettings::default(),
            stage: AdsrStage::Idle,
            level: 0.0,
            sample_rate,
        }
    }

    fn gate_on(&mut self, from_current: bool) {
        // Starting from the current level on a retrigger avoids the click an
        // attack-from-zero would produce when a key is repeated.
        if !from_current {
            self.level = 0.0;
        }
        self.stage = AdsrStage::Attack;
    }

    fn gate_off(&mut self) {
        if self.stage != AdsrStage::Idle {
            self.stage = AdsrStage::Release;
        }
    }

    fn is_active(&self) -> bool {
        self.stage != AdsrStage::Idle
    }

    fn next(&mut self) -> f32 {
        // Increments are per-sample and derived from the times; a zero time
        // would divide by zero, so a floor keeps the envelope moving.
        match self.stage {
            AdsrStage::Idle => return 0.0,
            AdsrStage::Attack => {
                let inc = self.increment(self.settings.attack_ms);
                self.level += inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                let target = self.settings.sustain.clamp(0.0, 1.0);
                let inc = self.increment(self.settings.decay_ms);
                if self.level > target + inc {
                    self.level -= inc;
                } else {
                    self.level = target;
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {
                self.level = self.settings.sustain.clamp(0.0, 1.0);
            }
            AdsrStage::Release => {
                let inc = self.increment(self.settings.release_ms);
                self.level -= inc;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
        }
        self.level
    }

    fn increment(&self, ms: f32) -> f32 {
        let time = if ms.is_finite() && ms > 0.0 { ms } else { 1.0 };
        let samples = time * 0.001 * self.sample_rate;
        if samples > 0.0 {
            1.0 / samples
        } else {
            1.0
        }
    }
}

/// One subtractive voice.
#[derive(Debug, Clone, Copy)]
pub struct SynthVoice {
    sample_rate: f32,
    oscillator: Oscillator,
    phase: f32,
    frequency: f32,
    amp_env: Adsr,
    filter_env: Adsr,
    filter: Svf,
    /// Filter cutoff when the envelope is at rest, in hertz.
    base_cutoff: f32,
    /// How far the filter envelope sweeps the cutoff, in octaves.
    filter_env_octaves: f32,
    resonance: f32,
    velocity: f32,
    pitch: u8,
    active: bool,
    /// Frames to wait before this voice starts, used for mid-block note-ons.
    delay_frames: u32,
}

impl SynthVoice {
    /// Creates a silent voice.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        let mut filter = Svf::new(sample_rate);
        filter.set_cutoff(8_000.0);
        Self {
            sample_rate,
            oscillator: Oscillator::Saw,
            phase: 0.0,
            frequency: 440.0,
            amp_env: Adsr::new(sample_rate),
            filter_env: Adsr::new(sample_rate),
            filter,
            base_cutoff: 8_000.0,
            filter_env_octaves: 2.0,
            resonance: 0.2,
            velocity: 1.0,
            pitch: 60,
            active: false,
            delay_frames: 0,
        }
    }

    /// Sets the oscillator waveform.
    pub fn set_oscillator(&mut self, oscillator: Oscillator) {
        self.oscillator = oscillator;
    }

    /// Sets the amplitude envelope.
    pub fn set_amp_envelope(&mut self, settings: AdsrSettings) {
        self.amp_env.settings = settings;
    }

    /// Sets the filter envelope.
    pub fn set_filter_envelope(&mut self, settings: AdsrSettings) {
        self.filter_env.settings = settings;
    }

    /// Sets the filter cutoff at rest and the sweep depth in octaves.
    pub fn set_filter(&mut self, base_cutoff_hz: f32, env_octaves: f32, resonance: f32) {
        if base_cutoff_hz.is_finite() && base_cutoff_hz > 0.0 {
            self.base_cutoff = base_cutoff_hz;
        }
        if env_octaves.is_finite() {
            self.filter_env_octaves = env_octaves.clamp(0.0, 6.0);
        }
        if resonance.is_finite() {
            self.resonance = resonance.clamp(0.0, 0.99);
        }
        self.filter.set_resonance(self.resonance);
    }

    /// The MIDI pitch this voice is sounding.
    #[must_use]
    pub const fn pitch(&self) -> u8 {
        self.pitch
    }

    /// Whether the voice is currently producing sound or tail.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Whether the voice can be stolen immediately.
    #[must_use]
    pub const fn is_releasing(&self) -> bool {
        matches!(self.amp_env.stage, AdsrStage::Release)
    }

    /// Starts a note at the start of the next block.
    pub fn note_on(&mut self, pitch: u8, velocity: f32) {
        self.note_on_delayed(pitch, velocity, 0);
    }

    /// Starts a note `delay` frames into the next block.
    pub fn note_on_delayed(&mut self, pitch: u8, velocity: f32, delay: u32) {
        self.pitch = pitch;
        self.frequency = midi_to_hz(pitch);
        // A retrigger of a still-sounding voice glides from its level;
        // a fresh voice starts from zero.
        let from_current = self.active;
        self.amp_env.gate_on(from_current);
        self.filter_env.gate_on(false);
        self.velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.active = true;
        self.delay_frames = delay;
    }

    /// Releases the note.
    pub fn note_off(&mut self) {
        self.amp_env.gate_off();
        self.filter_env.gate_off();
    }

    /// Silences the voice immediately and resets its state.
    pub fn all_notes_off(&mut self) {
        self.active = false;
        self.delay_frames = 0;
        self.phase = 0.0;
        self.amp_env.stage = AdsrStage::Idle;
        self.amp_env.level = 0.0;
        self.filter_env.stage = AdsrStage::Idle;
        self.filter_env.level = 0.0;
        self.filter.reset();
    }

    /// Produces one mono sample.
    ///
    /// Real-time safe. Returns `0.0` while the voice is delayed or finished.
    pub fn next_sample(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        if self.delay_frames > 0 {
            self.delay_frames -= 1;
            return 0.0;
        }

        let amp = self.amp_env.next();
        if !self.amp_env.is_active() {
            self.active = false;
            return 0.0;
        }

        // Oscillator.
        let raw = self.oscillator.sample(self.phase);
        self.phase += self.frequency / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }

        // Filter envelope: sweep the cutoff in octaves above the base.
        let fenv = self.filter_env.next();
        let cutoff = self.base_cutoff * exp2(self.filter_env_octaves * fenv);
        self.filter.set_cutoff(cutoff);

        let filtered = self.filter.process(raw, SvfMode::LowPass);
        filtered * amp * self.velocity
    }
}

/// Converts a MIDI note number to a frequency in hertz, A4 = 440 Hz.
#[must_use]
pub fn midi_to_hz(pitch: u8) -> f32 {
    440.0 * exp2((pitch as f32 - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_to_hz_matches_concert_pitch() {
        assert!((midi_to_hz(69) - 440.0).abs() < 0.5);
        assert!((midi_to_hz(81) - 880.0).abs() < 1.0);
        assert!((midi_to_hz(57) - 220.0).abs() < 0.5);
    }

    #[test]
    fn a_silent_voice_emits_zero() {
        let mut v = SynthVoice::new(48_000.0);
        assert_eq!(v.next_sample(), 0.0);
    }

    #[test]
    fn a_note_on_eventually_sounds_then_releases_to_silence() {
        let mut v = SynthVoice::new(48_000.0);
        v.set_amp_envelope(AdsrSettings {
            attack_ms: 1.0,
            decay_ms: 1.0,
            sustain: 1.0,
            release_ms: 1.0,
        });
        v.note_on(60, 1.0);
        let mut energy = 0.0_f32;
        for _ in 0..1_000 {
            let s = v.next_sample();
            energy += s * s;
        }
        assert!(energy > 0.0, "a note must produce sound");
        assert!(v.is_active());

        v.note_off();
        for _ in 0..1_000 {
            let _ = v.next_sample();
        }
        assert!(!v.is_active(), "release must end the voice");
    }

    #[test]
    fn a_delayed_note_stays_silent_for_the_delay() {
        let mut v = SynthVoice::new(48_000.0);
        v.set_amp_envelope(AdsrSettings {
            attack_ms: 0.1,
            decay_ms: 0.1,
            sustain: 1.0,
            release_ms: 1.0,
        });
        v.note_on_delayed(60, 1.0, 3);
        assert_eq!(v.next_sample(), 0.0);
        assert_eq!(v.next_sample(), 0.0);
        assert_eq!(v.next_sample(), 0.0);
        // The fourth sample is the first the voice may produce.
        let _ = v.next_sample();
        assert!(v.is_active());
    }

    #[test]
    fn oscillator_waveforms_are_bounded() {
        for osc in [
            Oscillator::Sine,
            Oscillator::Saw,
            Oscillator::Square,
            Oscillator::Triangle,
        ] {
            for i in 0..100 {
                let p = i as f32 / 100.0;
                let s = osc.sample(p);
                assert!(s.abs() <= 1.0001, "{osc:?} produced {s}");
            }
        }
    }

    #[test]
    fn a_long_voice_never_emits_non_finite_samples() {
        let mut v = SynthVoice::new(48_000.0);
        v.note_on(36, 1.0);
        for _ in 0..48_000 {
            let s = v.next_sample();
            assert!(s.is_finite(), "voice produced a non-finite sample");
        }
    }
}

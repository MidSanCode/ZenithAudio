//! Transport: playhead, tempo, loop region and the tick/frame conversion.
//!
//! The transport is authoritative for *where* playback is. It stores the
//! playhead in **frames** rather than seconds because frames are what the audio
//! callback advances by and what the ABI passes around; seconds are a derived
//! view and a lossy `double` round trip is exactly what caused drift before S0.
//!
//! # Ticks vs frames
//!
//! Musical positions are stored in **ticks** (PPQ = 960, PLAN §3.S0) and
//! converted to frames only at the edge, using the current tempo:
//!
//! ```text
//!   frames = ticks * sample_rate * 60 / (bpm * ppq)
//! ```
//!
//! Tempo changes therefore re-derive frame positions without resampling audio
//! (PLAN §3.S1 / §1.5). The conversion lives in exactly one place so the
//! sequencer and the UI cannot disagree.

/// What the transport is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportState {
    /// Stopped and rewound.
    #[default]
    Stopped,
    /// Playing forward.
    Playing,
    /// Paused in place.
    Paused,
}

impl TransportState {
    /// The ABI state code: `0` stopped, `1` playing, `2` paused.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Stopped => 0,
            Self::Playing => 1,
            Self::Paused => 2,
        }
    }
}

/// Default pulses per quarter note (PLAN §3.S0).
pub const DEFAULT_PPQ: u32 = 960;

/// Default tempo in beats per minute.
pub const DEFAULT_BPM: f32 = 120.0;

/// The playhead and its time base.
#[derive(Debug, Clone, Copy)]
pub struct Transport {
    /// Sample rate in hertz.
    sample_rate: u32,
    /// Playhead position, in frames.
    position_frames: i64,
    /// Tempo in beats per minute.
    bpm: f32,
    /// Pulses per quarter note.
    ppq: u32,
    /// Current state.
    state: TransportState,
    /// Loop start, in frames, when looping.
    loop_start: i64,
    /// Loop end, in frames, when looping.
    loop_end: i64,
    /// Whether the loop region is active.
    loop_enabled: bool,
    /// Time signature numerator.
    time_signature_num: u32,
    /// Time signature denominator.
    time_signature_den: u32,
}

impl Transport {
    /// Creates a stopped transport at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: if sample_rate > 0 { sample_rate } else { 48_000 },
            position_frames: 0,
            bpm: DEFAULT_BPM,
            ppq: DEFAULT_PPQ,
            state: TransportState::Stopped,
            loop_start: 0,
            loop_end: 0,
            loop_enabled: false,
            time_signature_num: 4,
            time_signature_den: 4,
        }
    }

    /// The current playhead, in frames.
    #[must_use]
    pub const fn position_frames(&self) -> i64 {
        self.position_frames
    }

    /// The current playhead, in ticks.
    #[must_use]
    pub fn position_ticks(&self) -> i64 {
        self.frames_to_ticks(self.position_frames)
    }

    /// Tempo, in beats per minute.
    #[must_use]
    pub const fn bpm(&self) -> f32 {
        self.bpm
    }

    /// Pulses per quarter note.
    #[must_use]
    pub const fn ppq(&self) -> u32 {
        self.ppq
    }

    /// Sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The transport state.
    #[must_use]
    pub const fn state(&self) -> TransportState {
        self.state
    }

    /// The ABI state code.
    #[must_use]
    pub const fn state_code(&self) -> u32 {
        self.state.code()
    }

    /// Whether the transport is producing audio.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        matches!(self.state, TransportState::Playing)
    }

    /// Whether a loop region is active.
    #[must_use]
    pub const fn is_looping(&self) -> bool {
        self.loop_enabled
    }

    /// Starts playback.
    pub fn play(&mut self) {
        self.state = TransportState::Playing;
    }

    /// Pauses in place.
    pub fn pause(&mut self) {
        if matches!(self.state, TransportState::Playing) {
            self.state = TransportState::Paused;
        }
    }

    /// Stops and rewinds to zero.
    pub fn stop(&mut self) {
        self.state = TransportState::Stopped;
        self.position_frames = 0;
    }

    /// Seeks to an absolute frame.
    pub fn seek_frames(&mut self, frame: i64) {
        self.position_frames = frame.max(0);
    }

    /// Seeks to an absolute tick.
    pub fn seek_ticks(&mut self, ticks: i64) {
        self.position_frames = self.ticks_to_frames(ticks).max(0);
    }

    /// Sets the tempo, clamped to a musical range.
    ///
    /// The playhead is preserved in ticks across the change, so slowing down
    /// does not jump the position to a different musical point.
    pub fn set_tempo(&mut self, bpm: f32) {
        if !bpm.is_finite() || bpm <= 0.0 {
            return;
        }
        let ticks = self.position_ticks();
        self.bpm = bpm.clamp(1.0, 999.0);
        self.position_frames = self.ticks_to_frames(ticks).max(0);
    }

    /// Sets the loop region, in ticks, and enables or disables it.
    ///
    /// An end before the start disables the loop rather than accepting a
    /// nonsensical region.
    pub fn set_loop(&mut self, start_ticks: i64, end_ticks: i64, enabled: bool) {
        let start = self.ticks_to_frames(start_ticks).max(0);
        let end = self.ticks_to_frames(end_ticks).max(0);
        if end <= start {
            self.loop_enabled = false;
            return;
        }
        self.loop_start = start;
        self.loop_end = end;
        self.loop_enabled = enabled;
    }

    /// Sets the time signature.
    pub fn set_time_signature(&mut self, num: u32, den: u32) {
        if num > 0 && den > 0 {
            self.time_signature_num = num;
            self.time_signature_den = den;
        }
    }

    /// The time signature as `(numerator, denominator)`.
    #[must_use]
    pub const fn time_signature(&self) -> (u32, u32) {
        (self.time_signature_num, self.time_signature_den)
    }

    /// Advances the playhead by `frames`, wrapping within an active loop.
    pub fn advance_frames(&mut self, frames: i64) {
        if !self.is_playing() || frames <= 0 {
            return;
        }
        self.position_frames += frames;
        if self.loop_enabled && self.loop_end > self.loop_start {
            while self.position_frames >= self.loop_end {
                let span = self.loop_end - self.loop_start;
                self.position_frames -= span;
            }
        }
    }

    /// Converts ticks to frames at the current tempo and sample rate.
    #[must_use]
    pub fn ticks_to_frames(&self, ticks: i64) -> i64 {
        if self.bpm <= 0.0 || self.ppq == 0 {
            return 0;
        }
        let frames = ticks as f64 * self.sample_rate as f64 * 60.0
            / (self.bpm as f64 * self.ppq as f64);
        frames.round() as i64
    }

    /// Converts frames to ticks at the current tempo and sample rate.
    #[must_use]
    pub fn frames_to_ticks(&self, frames: i64) -> i64 {
        if self.sample_rate == 0 {
            return 0;
        }
        let ticks = frames as f64 * self.bpm as f64 * self.ppq as f64
            / (self.sample_rate as f64 * 60.0);
        ticks.round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_transport_is_stopped_at_zero() {
        let t = Transport::new(48_000);
        assert_eq!(t.position_frames(), 0);
        assert_eq!(t.state(), TransportState::Stopped);
        assert_eq!(t.state_code(), 0);
    }

    #[test]
    fn play_advances_and_stop_rewinds() {
        let mut t = Transport::new(48_000);
        t.play();
        t.advance_frames(256);
        assert_eq!(t.position_frames(), 256);
        assert_eq!(t.state_code(), 1);
        t.stop();
        assert_eq!(t.position_frames(), 0);
        assert_eq!(t.state_code(), 0);
    }

    #[test]
    fn a_paused_transport_does_not_advance() {
        let mut t = Transport::new(48_000);
        t.play();
        t.advance_frames(100);
        t.pause();
        t.advance_frames(100);
        assert_eq!(t.position_frames(), 100);
        assert_eq!(t.state_code(), 2);
    }

    #[test]
    fn ticks_and_frames_round_trip_at_a_known_tempo() {
        let mut t = Transport::new(48_000);
        t.set_tempo(120.0);
        // One beat at 120 BPM is 0.5 s = 24 000 frames = 960 ticks.
        assert_eq!(t.ticks_to_frames(960), 24_000);
        assert_eq!(t.frames_to_ticks(24_000), 960);
    }

    #[test]
    fn changing_tempo_preserves_the_musical_position() {
        let mut t = Transport::new(48_000);
        t.set_tempo(120.0);
        t.seek_ticks(960);
        let before = t.position_ticks();
        t.set_tempo(60.0);
        assert_eq!(t.position_ticks(), before, "the musical position must not jump");
        // At half tempo the same tick is twice as many frames.
        assert_eq!(t.position_frames(), 48_000);
    }

    #[test]
    fn a_loop_wraps_the_playhead() {
        let mut t = Transport::new(48_000);
        t.set_tempo(120.0);
        t.set_loop(0, 960, true); // 0..24 000 frames
        t.play();
        t.advance_frames(25_000);
        assert!(t.position_frames() < 24_000, "the playhead must wrap inside the loop");
    }

    #[test]
    fn a_reversed_loop_region_is_disabled() {
        let mut t = Transport::new(48_000);
        t.set_loop(960, 0, true);
        assert!(!t.is_looping(), "an end before the start must not loop");
    }
}

//! The note sequencer: turns scheduled notes into sample-accurate events.
//!
//! The sequencer holds a list of note events in **ticks** and, on each block,
//! emits the events that fall inside the block window. An event carries a frame
//! offset so an event landing in the middle of a block starts at the right
//! sample rather than at the block boundary — that offset is what makes the
//! scheduling "sample-accurate" (PLAN §3.S1 item 3).
//!
//! # Real-time discipline
//!
//! [`Sequencer::collect_due`] walks a borrowed slice and writes into a
//! caller-supplied fixed array. It allocates nothing and holds no lock. Editing
//! the note list is a control-thread operation.

use alloc::vec::Vec;

/// What an event asks the engine to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Start a note.
    NoteOn,
    /// Release a note.
    NoteOff,
    /// Release every note.
    AllNotesOff,
}

impl EventKind {
    /// A sort key that orders note-offs before note-ons at the same tick.
    ///
    /// Without this, a repeated note at one tick would start its new voice
    /// before releasing the old one, and a pool at its limit would steal the
    /// note it just started.
    const fn cmp_key(self) -> u8 {
        match self {
            Self::NoteOff => 0,
            Self::AllNotesOff => 1,
            Self::NoteOn => 2,
        }
    }
}

/// A note event on the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledEvent {
    /// What to do.
    pub kind: EventKind,
    /// MIDI pitch.
    pub pitch: u8,
    /// Velocity, `0.0..=1.0`.
    pub velocity: f32,
    /// Event position, in ticks from the project origin.
    pub tick: i64,
    /// How far into the block the event falls, in frames.
    ///
    /// Filled in by [`Sequencer::collect_due`]; ignored when a caller pushes an
    /// event.
    pub offset_frames: u32,
}

impl ScheduledEvent {
    /// An empty placeholder, used to initialise the caller's fixed buffer.
    pub const EMPTY: Self = Self {
        kind: EventKind::NoteOn,
        pitch: 0,
        velocity: 0.0,
        tick: 0,
        offset_frames: 0,
    };
}

/// A note schedule, kept sorted by tick.
#[derive(Debug, Default)]
pub struct Sequencer {
    /// Events sorted ascending by tick.
    events: Vec<ScheduledEvent>,
    /// Index of the first event not yet consumed.
    ///
    /// A cursor rather than removal: dropping the front of a `Vec` would shift
    /// memory on the audio thread. The list is compacted on the control thread
    /// when it is edited.
    cursor: usize,
    /// Whether the list has changed and must be re-sorted.
    dirty: bool,
}

impl Sequencer {
    /// Builds an empty sequencer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            cursor: 0,
            dirty: false,
        }
    }

    /// Number of events in the schedule.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the schedule is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Whether every event has been consumed.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.cursor >= self.events.len()
    }

    /// Removes every event and rewinds the cursor.
    pub fn clear(&mut self) {
        self.events.clear();
        self.cursor = 0;
        self.dirty = false;
    }

    /// Rewinds the cursor to the start, for a restart from the top.
    pub fn rewind(&mut self) {
        self.cursor = 0;
    }

    /// Adds a note pair (on and off) to the schedule.
    ///
    /// `start_tick` and `length_ticks` are musical positions; the sequencer
    /// stores both events and sorts lazily. Control thread only.
    pub fn push_note(&mut self, pitch: u8, velocity: f32, start_tick: i64, length_ticks: i64) {
        let velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.events.push(ScheduledEvent {
            kind: EventKind::NoteOn,
            pitch,
            velocity,
            tick: start_tick,
            offset_frames: 0,
        });
        self.events.push(ScheduledEvent {
            kind: EventKind::NoteOff,
            pitch,
            velocity: 0.0,
            // Place the release exactly at the end of the declared length. A
            // zero-length note puts on and off at the same tick; the sort
            // orders note-offs first, so the voice is released before the new
            // one starts.
            tick: start_tick + length_ticks.max(0),
            offset_frames: 0,
        });
        self.dirty = true;
        self.sort();
    }

    /// Adds an event, sorting lazily. Control thread only.
    pub fn push(&mut self, event: ScheduledEvent) {
        self.events.push(event);
        self.dirty = true;
    }

    /// Sorts the schedule by tick, note-offs first at a shared tick. Control
    /// thread only.
    pub fn sort(&mut self) {
        if self.dirty {
            self.events.sort_by(|a, b| {
                a.tick
                    .cmp(&b.tick)
                    .then(a.kind.cmp_key().cmp(&b.kind.cmp_key()))
            });
            self.dirty = false;
        }
    }

    /// Replaces the whole schedule. Control thread only.
    pub fn set_events(&mut self, mut events: Vec<ScheduledEvent>) {
        events.sort_by(|a, b| {
            a.tick
                .cmp(&b.tick)
                .then(a.kind.cmp_key().cmp(&b.kind.cmp_key()))
        });
        self.events = events;
        self.cursor = 0;
        self.dirty = false;
    }

    /// Collects events that fall within `[start_frame, start_frame + frames)`
    /// into `out`, returning how many were written.
    ///
    /// `sample_rate`, `bpm` and `ppq` convert ticks to frames. The block window
    /// in frames is derived from the frame span, so a tempo change is handled
    /// per block.
    ///
    /// Real-time safe: no allocation. If `out` is too small the surplus events
    /// are left for the next block rather than dropped, so a dense passage is
    /// delayed, not lost.
    pub fn collect_due(
        &mut self,
        start_frame: i64,
        frames: usize,
        sample_rate: u32,
        bpm: f32,
        ppq: u32,
        out: &mut [ScheduledEvent],
    ) -> usize {
        if bpm <= 0.0 || ppq == 0 || frames == 0 || sample_rate == 0 {
            return 0;
        }
        self.dirty = false;

        let end_frame = start_frame + frames as i64;
        let mut written = 0usize;
        while self.cursor < self.events.len() {
            let event = self.events[self.cursor];
            let event_frame = tick_to_frame(event.tick, sample_rate, bpm, ppq);
            if event_frame >= end_frame {
                break;
            }
            if event_frame >= start_frame {
                if written < out.len() {
                    let mut due = event;
                    due.offset_frames = (event_frame - start_frame).max(0) as u32;
                    out[written] = due;
                    written += 1;
                    self.cursor += 1;
                } else {
                    // The caller's buffer is full; stop so the event is
                    // delivered next block rather than dropped.
                    break;
                }
            } else {
                // The event is behind the playhead (a seek landed past it);
                // skip it.
                self.cursor += 1;
            }
        }
        written
    }
}

/// Converts a tick position to a frame position.
///
/// Shared with the transport's conversion so they cannot drift apart.
#[must_use]
pub fn tick_to_frame(tick: i64, sample_rate: u32, bpm: f32, ppq: u32) -> i64 {
    if bpm <= 0.0 || ppq == 0 || sample_rate == 0 {
        return 0;
    }
    let frames =
        tick as f64 * sample_rate as f64 * 60.0 / (bpm as f64 * ppq as f64);
    frames.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_to_frame_matches_the_transport_conversion() {
        use crate::transport::Transport;
        let mut t = Transport::new(48_000);
        t.set_tempo(120.0);
        for tick in [0, 240, 480, 960, 3_840] {
            assert_eq!(
                tick_to_frame(tick, 48_000, 120.0, 960),
                t.ticks_to_frames(tick),
                "tick {tick} converted differently"
            );
        }
    }

    #[test]
    fn an_empty_sequencer_produces_no_events() {
        let mut seq = Sequencer::new();
        let mut out = [ScheduledEvent::EMPTY; 8];
        let n = seq.collect_due(0, 256, 48_000, 120.0, 960, &mut out);
        assert_eq!(n, 0);
        assert!(seq.is_finished());
    }

    #[test]
    fn a_note_is_emitted_in_the_block_it_falls_in() {
        let mut seq = Sequencer::new();
        seq.push_note(60, 1.0, 0, 960);
        let mut out = [ScheduledEvent::EMPTY; 8];
        // First block covers tick 0..960: the note-on is due, the note-off is
        // exactly on the boundary and belongs to the next block.
        let n = seq.collect_due(0, 24_000, 48_000, 120.0, 960, &mut out);
        assert_eq!(n, 1, "the note-on falls in the first beat");
        assert_eq!(out[0].kind, EventKind::NoteOn);
        assert_eq!(out[0].pitch, 60);

        // Second block covers tick 960..: the note-off is now due.
        let n = seq.collect_due(24_000, 24_000, 48_000, 120.0, 960, &mut out);
        assert_eq!(n, 1, "the note-off falls on the next beat boundary");
        assert_eq!(out[0].kind, EventKind::NoteOff);
    }

    #[test]
    fn an_event_landing_mid_block_carries_its_offset() {
        let mut seq = Sequencer::new();
        seq.push(ScheduledEvent {
            kind: EventKind::NoteOn,
            pitch: 64,
            velocity: 1.0,
            tick: 480,
            offset_frames: 0,
        });
        let mut out = [ScheduledEvent::EMPTY; 4];
        let n = seq.collect_due(0, 24_000, 48_000, 120.0, 960, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0].offset_frames, 12_000, "offset must point into the block");
    }

    #[test]
    fn events_before_the_playhead_are_skipped_on_a_seek() {
        let mut seq = Sequencer::new();
        seq.push(ScheduledEvent {
            kind: EventKind::NoteOn,
            pitch: 60,
            velocity: 1.0,
            tick: 0,
            offset_frames: 0,
        });
        seq.push(ScheduledEvent {
            kind: EventKind::NoteOn,
            pitch: 62,
            velocity: 1.0,
            tick: 960,
            offset_frames: 0,
        });
        let mut out = [ScheduledEvent::EMPTY; 4];
        let n = seq.collect_due(24_000, 100, 48_000, 120.0, 960, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0].pitch, 62);
    }

    #[test]
    fn a_full_output_buffer_defers_rather_than_drops() {
        let mut seq = Sequencer::new();
        for i in 0..4 {
            seq.push(ScheduledEvent {
                kind: EventKind::NoteOn,
                pitch: 60 + i,
                velocity: 1.0,
                tick: 0,
                offset_frames: 0,
            });
        }
        let mut out = [ScheduledEvent::EMPTY; 2];
        let first = seq.collect_due(0, 24_000, 48_000, 120.0, 960, &mut out);
        assert_eq!(first, 2);
        let second = seq.collect_due(0, 24_000, 48_000, 120.0, 960, &mut out);
        assert_eq!(second, 2, "the deferred events must arrive next call");
    }

    #[test]
    fn rewind_replays_from_the_top() {
        let mut seq = Sequencer::new();
        seq.push_note(60, 1.0, 0, 0);
        let mut out = [ScheduledEvent::EMPTY; 4];
        seq.collect_due(0, 24_000, 48_000, 120.0, 960, &mut out);
        assert!(seq.is_finished());
        seq.rewind();
        assert!(!seq.is_finished());
    }
}

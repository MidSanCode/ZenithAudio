//! A no-lock event queue for the transport boundary.
//!
//! Control-thread callers (the UI, MIDI input, automation recording) enqueue
//! note events for the audio thread; the engine drains them at block boundaries.
//! The queue is backed by [`crate::engine::realtime::SpscQueue`], so a full
//! queue returns the event to the sender rather than blocking the UI or the
//! audio thread (ABI §6.6).
//!
//! # Real-time discipline
//!
//! [`EventQueue::drain_into`] is the audio-thread side and allocates nothing.

use alloc::vec::Vec;

use super::sequencer::ScheduledEvent;
use crate::engine::realtime::SpscQueue;

/// Default queue depth; a few hundred events is far more than a block needs.
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 1024;

/// A single-producer / single-consumer note event queue.
pub struct EventQueue {
    /// The underlying ring buffer.
    queue: SpscQueue<ScheduledEvent>,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new(DEFAULT_EVENT_QUEUE_CAPACITY)
    }
}

impl EventQueue {
    /// Creates a queue with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: SpscQueue::new(capacity),
        }
    }

    /// Enqueues an event. Returns it back when the queue is full.
    ///
    /// Control thread. A full queue is a caller-visible condition, not a silent
    /// drop, so a UI can report "input overloaded" instead of losing notes
    /// without a trace.
    pub fn push(&self, event: ScheduledEvent) -> Result<(), ScheduledEvent> {
        self.queue.push(event)
    }

    /// Whether the queue holds no events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Drains up to `out.len()` events into `out`, returning how many.
    ///
    /// Audio thread. Real-time safe.
    pub fn drain_into(&self, out: &mut [ScheduledEvent]) -> usize {
        let mut count = 0;
        while count < out.len() {
            match self.queue.pop() {
                Some(event) => {
                    out[count] = event;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    /// Drains every queued event into a `Vec`. Control thread / tests only;
    /// this allocates, so it must not run on the audio thread.
    pub fn drain_all(&self) -> Vec<ScheduledEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.queue.pop() {
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::sequencer::{EventKind, ScheduledEvent};

    fn event(pitch: u8) -> ScheduledEvent {
        ScheduledEvent {
            kind: EventKind::NoteOn,
            pitch,
            velocity: 1.0,
            tick: 0,
            offset_frames: 0,
        }
    }

    #[test]
    fn events_round_trip_in_order() {
        let q = EventQueue::new(8);
        q.push(event(60)).ok();
        q.push(event(62)).ok();
        let mut out = [ScheduledEvent::EMPTY; 8];
        let n = q.drain_into(&mut out);
        assert_eq!(n, 2);
        assert_eq!(out[0].pitch, 60);
        assert_eq!(out[1].pitch, 62);
        assert!(q.is_empty());
    }

    #[test]
    fn a_full_queue_returns_the_event_rather_than_blocking() {
        let q = EventQueue::new(2); // usable capacity 1
        assert!(q.push(event(60)).is_ok());
        assert!(q.push(event(62)).is_err(), "a full queue must not block");
    }

    #[test]
    fn draining_respects_the_output_length() {
        let q = EventQueue::new(8);
        for pitch in 60..66 {
            q.push(event(pitch)).ok();
        }
        let mut out = [ScheduledEvent::EMPTY; 3];
        assert_eq!(q.drain_into(&mut out), 3);
        assert_eq!(q.drain_into(&mut out), 3);
    }
}

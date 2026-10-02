//! Transport: playhead, tempo, loop region, sequencing and the event queue.
//!
//! This module is the time base of the engine. It stores the playhead in
//! **frames** and musical positions in **ticks** (PPQ = 960), and converts
//! between them at the edge. The conversion lives in one place so the sequencer
//! and the UI cannot disagree.
//!
//! ```text
//! transport/
//! ├── transport.rs    playhead, tempo, loop, tick<->frame conversion
//! ├── sequencer.rs    the note schedule and sample-accurate delivery
//! └── event_queue.rs  lock-free control -> audio note injection
//! ```

pub mod event_queue;
pub mod sequencer;
#[allow(clippy::module_inception)]
pub mod transport;

pub use event_queue::{EventQueue, DEFAULT_EVENT_QUEUE_CAPACITY};
pub use sequencer::{tick_to_frame, EventKind, ScheduledEvent, Sequencer};
pub use transport::{Transport, TransportState, DEFAULT_BPM, DEFAULT_PPQ};

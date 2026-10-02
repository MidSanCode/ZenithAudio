//! The real-time audio engine (PLAN §3.S1).
//!
//! # What this module owns
//!
//! The engine is the object that turns "the transport is playing" into audio:
//!
//! ```text
//!   transport position
//!        │
//!        ▼
//!   sequencer ──▶ event queue ──▶ voice allocator ──▶ source bus
//!                                                        │
//!                                       automation ───────┤
//!                                                        ▼
//!                                                   mixer graph
//!                                                        │
//!                                                   master output
//! ```
//!
//! It composes the pieces rather than reimplementing them: [`crate::transport`]
//! owns the playhead and the note schedule, [`crate::voice`] owns the
//! instruments, [`crate::mixer`] owns the console, and [`crate::automation`]
//! owns parameter values. The engine is the glue and the clock.
//!
//! # Device drivers
//!
//! The engine never names `cpal`. It renders when a [`crate::driver::AudioDriver`]
//! asks it to, which is what lets the same core run on desktop (a device
//! callback), the web (an `AudioWorklet`) and offline (a plain loop, see
//! [`crate::driver::OfflineDriver`]). The default build ships only the offline
//! driver because `cpal` is an external crate dependency; `zenith_engine_start`
//! reports [`crate::Status::Unsupported`] when no device driver is compiled in
//! rather than pretending to play.
//!
//! # Real-time discipline
//!
//! [`Engine::render_block`] is the audio-thread entry point. It allocates
//! nothing: every buffer is sized in [`Engine::new`], the automation pass is
//! allocation-free, and graph edits are control-thread-only (ABI §7.3).

pub mod graph;
pub mod node;
pub mod realtime;
pub mod render_context;

pub use graph::{DspGraph, GraphError};
pub use node::DspNode;
pub use render_context::RenderContext;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use crate::automation::lane::LaneSet;
use crate::automation::modulator::ModulatorBank;
use crate::automation::player::AutomationPlayer;
use crate::automation::store::ParameterStore;
use crate::effects::buffer::AudioBuffer;
use crate::mixer::graph::MixerGraph;
use crate::transport::{EventKind, ScheduledEvent, Sequencer, Transport};
use crate::voice::VoiceAllocator;

/// Default frames per block: 256 at 48 kHz is about 5.3 ms (PLAN §3.S1 item 4).
pub const DEFAULT_BLOCK_SIZE: usize = 256;

/// Default voice count: enough polyphony for a dense pad without a pool so
/// large that a search for a free voice is measurably slow.
pub const DEFAULT_MAX_VOICES: usize = 64;

/// How the engine was created, mirrored across the ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// Target sample rate in hertz.
    pub sample_rate: u32,
    /// Frames per block; the engine never assumes this is constant (P6).
    pub block_size: u32,
    /// Preallocated mixer channels.
    pub max_channels: u32,
    /// Preallocated tracks.
    pub max_tracks: u32,
    /// Which driver to use; see `ZenithDriverKind`.
    pub driver_kind: u32,
    /// Feature flags; see `ZENITH_FLAG_*`.
    pub flags: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            block_size: DEFAULT_BLOCK_SIZE as u32,
            max_channels: 64,
            max_tracks: 128,
            driver_kind: 0,
            flags: 0,
        }
    }
}

impl EngineConfig {
    /// Validates and normalises a configuration into the range the engine
    /// supports. Returns `None` when the sample rate is unusable.
    #[must_use]
    pub fn sanitized(self) -> Option<Self> {
        if self.sample_rate < 8_000 || self.sample_rate > 384_000 {
            return None;
        }
        Some(Self {
            // PLAN §3.S1 item 4: 64..=2048.
            block_size: self.block_size.clamp(64, 2048),
            max_channels: self.max_channels.max(1),
            max_tracks: self.max_tracks.max(1),
            ..self
        })
    }
}

/// The engine's live transport state, mirrored to Dart as an atomic snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EngineStatus {
    /// Playhead position, in frames.
    pub playhead_frames: i64,
    /// Tempo, in beats per minute.
    pub bpm: f32,
    /// Real-time load `0.0..1.0`; `0.0` until a driver reports it.
    pub cpu_load: f32,
    /// Buffer underruns accumulated by the driver.
    pub xrun_count: u32,
    /// Voices currently sounding.
    pub active_voices: u32,
    /// Voice pool size.
    pub max_voices: u32,
    /// Web degradation tier: `0` full, `1` reduced, `2` minimal.
    pub degrade_level: u32,
    /// `0` stopped, `1` playing, `2` paused.
    pub state: u32,
    /// Driver kind actually in use.
    pub driver_kind: u32,
    /// Negotiated sample rate.
    pub sample_rate: u32,
    /// Current block size.
    pub block_size: u32,
}

/// A lock-free snapshot the audio thread writes and the UI thread reads.
///
/// ABI §5.3 forbids callbacks into Dart and §6.3 requires the status to be a
/// single coherent view rather than a torn read. Every field is an atomic, and
/// the reader may see values one block old, which for a position/level display
/// is preferable to a lock on the audio thread.
#[derive(Debug)]
pub struct EngineSnapshot {
    /// Playhead in frames.
    playhead: AtomicI64,
    /// Tempo as `f32` bits.
    bpm_bits: AtomicU32,
    /// Real-time load as `f32` bits, `0.0..1.0`.
    cpu_bits: AtomicU32,
    /// Buffer underruns.
    xrun: AtomicU32,
    /// Active voices.
    active_voices: AtomicU32,
    /// Voice pool size.
    max_voices: AtomicU32,
    /// Web degradation tier.
    degrade: AtomicU32,
    /// Transport state code.
    state: AtomicU32,
    /// Driver kind.
    driver_kind: AtomicU32,
    /// Sample rate.
    sample_rate: AtomicU32,
    /// Block size.
    block_size: AtomicU32,
}

impl EngineSnapshot {
    /// Creates a snapshot initialised from `status`.
    #[must_use]
    pub fn new(status: &EngineStatus) -> Arc<Self> {
        Arc::new(Self {
            playhead: AtomicI64::new(status.playhead_frames),
            bpm_bits: AtomicU32::new(status.bpm.to_bits()),
            cpu_bits: AtomicU32::new(status.cpu_load.to_bits()),
            xrun: AtomicU32::new(status.xrun_count),
            active_voices: AtomicU32::new(status.active_voices),
            max_voices: AtomicU32::new(status.max_voices),
            degrade: AtomicU32::new(status.degrade_level),
            state: AtomicU32::new(status.state),
            driver_kind: AtomicU32::new(status.driver_kind),
            sample_rate: AtomicU32::new(status.sample_rate),
            block_size: AtomicU32::new(status.block_size),
        })
    }

    /// Publishes a status view.
    pub fn publish(&self, status: &EngineStatus) {
        self.playhead.store(status.playhead_frames, Ordering::Relaxed);
        self.bpm_bits.store(status.bpm.to_bits(), Ordering::Relaxed);
        self.cpu_bits
            .store(status.cpu_load.to_bits(), Ordering::Relaxed);
        self.xrun.store(status.xrun_count, Ordering::Relaxed);
        self.active_voices
            .store(status.active_voices, Ordering::Relaxed);
        self.max_voices.store(status.max_voices, Ordering::Relaxed);
        self.degrade.store(status.degrade_level, Ordering::Relaxed);
        self.state.store(status.state, Ordering::Relaxed);
        self.driver_kind
            .store(status.driver_kind, Ordering::Relaxed);
        self.sample_rate.store(status.sample_rate, Ordering::Relaxed);
        self.block_size.store(status.block_size, Ordering::Relaxed);
    }

    /// Reads a coherent status view.
    #[must_use]
    pub fn read(&self) -> EngineStatus {
        EngineStatus {
            playhead_frames: self.playhead.load(Ordering::Relaxed),
            bpm: f32::from_bits(self.bpm_bits.load(Ordering::Relaxed)),
            cpu_load: f32::from_bits(self.cpu_bits.load(Ordering::Relaxed)),
            xrun_count: self.xrun.load(Ordering::Relaxed),
            active_voices: self.active_voices.load(Ordering::Relaxed),
            max_voices: self.max_voices.load(Ordering::Relaxed),
            degrade_level: self.degrade.load(Ordering::Relaxed),
            state: self.state.load(Ordering::Relaxed),
            driver_kind: self.driver_kind.load(Ordering::Relaxed),
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            block_size: self.block_size.load(Ordering::Relaxed),
        }
    }
}

/// The automation state the engine owns.
///
/// Kept as a named struct rather than four loose fields so it can be handed to
/// the player as one bundle and so the FFI layer has a single place to reach.
pub struct AutomationLayer {
    /// Registered parameters and their live values.
    pub store: ParameterStore,
    /// The automation lanes.
    pub lanes: LaneSet,
    /// The real-time evaluator.
    pub player: AutomationPlayer,
    /// Modulation sources.
    pub modulators: ModulatorBank,
}

impl AutomationLayer {
    /// Creates an empty automation layer at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut player = AutomationPlayer::new();
        player.prepare(sample_rate, 0);
        Self {
            store: ParameterStore::default(),
            lanes: LaneSet::new(),
            player,
            modulators: ModulatorBank::new(),
        }
    }
}

/// The real-time engine.
pub struct Engine {
    /// Validated configuration.
    config: EngineConfig,
    /// Playhead and loop state.
    transport: Transport,
    /// Pre-scheduled note events.
    sequencer: Sequencer,
    /// Instrument voices.
    voices: VoiceAllocator,
    /// The console topology.
    mixer: MixerGraph,
    /// Parameter registry, lanes and modulation sources.
    automation: AutomationLayer,
    /// Post-mixer serial DSP chain.
    master_chain: DspGraph,
    /// Preallocated source bus fed by the voices, interleaved stereo.
    source: Vec<f32>,
    /// Preallocated master output, interleaved stereo.
    output: Vec<f32>,
    /// Planar scratch for the master chain, stereo, `2 * block` samples.
    chain_planar: Vec<f32>,
    /// Scratch for the mixer's per-channel summing, `block` frames stereo.
    sum_scratch: Vec<f32>,
    /// Scratch copy of the mixer's topological order, preallocated so
    /// `process_mixer` never allocates on the audio thread (ABI P5).
    order_scratch: Vec<u32>,
    /// Set once a device driver has been attached.
    driver_attached: bool,
    /// Lock-free status published at the end of every block.
    snapshot: Arc<EngineSnapshot>,
    /// Buffer underruns reported by the driver.
    xrun_count: u32,
    /// The insert channel the voice bus feeds, wired to master at creation.
    source_channel: u32,
}

impl Engine {
    /// Creates a prepared engine.
    ///
    /// Returns `None` for a configuration the engine cannot honour, so the FFI
    /// layer can report `ZENITH_ERR_INVALID_ARG` instead of constructing a
    /// half-valid engine.
    #[must_use]
    pub fn new(config: EngineConfig) -> Option<Self> {
        let config = config.sanitized()?;
        let max_block = config.block_size as usize;
        let mut mixer = MixerGraph::new(max_block);
        let order_capacity = mixer.len();
        // The mixer's default channels are created unrouted (a project decides
        // the topology). S1's audible path needs one insert feeding master, so
        // wire the first insert here; S6/S3 will build the project's real
        // routing on top.
        let source_channel = crate::mixer::DEFAULT_RETURN_BUSES as u32 + 1;
        let master = crate::mixer::ChannelId::MASTER.get();
        let _ = mixer.connect(source_channel, master);
        mixer.publish_audibility();
        let mut master_chain = DspGraph::new();
        master_chain.prepare(config.sample_rate as f32, max_block, 2);
        let snapshot = EngineSnapshot::new(&EngineStatus {
            max_voices: DEFAULT_MAX_VOICES as u32,
            sample_rate: config.sample_rate,
            block_size: config.block_size,
            ..EngineStatus::default()
        });
        Some(Self {
            config,
            transport: Transport::new(config.sample_rate),
            sequencer: Sequencer::new(),
            voices: VoiceAllocator::new(DEFAULT_MAX_VOICES, config.sample_rate as f32),
            mixer,
            automation: AutomationLayer::new(config.sample_rate as f32),
            master_chain,
            source: alloc::vec![0.0; max_block * 2],
            output: alloc::vec![0.0; max_block * 2],
            chain_planar: alloc::vec![0.0; max_block * 2],
            sum_scratch: alloc::vec![0.0; max_block * 2],
            order_scratch: alloc::vec![0; order_capacity],
            driver_attached: false,
            snapshot,
            xrun_count: 0,
            source_channel,
        })
    }

    /// The shared lock-free status snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Arc<EngineSnapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Records a buffer underrun reported by the driver.
    pub fn note_xrun(&mut self) {
        self.xrun_count = self.xrun_count.saturating_add(1);
    }

    /// The validated configuration.
    #[must_use]
    pub const fn config(&self) -> EngineConfig {
        self.config
    }

    /// The transport.
    #[must_use]
    pub const fn transport(&self) -> &Transport {
        &self.transport
    }

    /// The transport, mutably. Control thread only.
    pub fn transport_mut(&mut self) -> &mut Transport {
        &mut self.transport
    }

    /// The sequencer.
    #[must_use]
    pub const fn sequencer(&self) -> &Sequencer {
        &self.sequencer
    }

    /// The sequencer, mutably. Control thread only.
    pub fn sequencer_mut(&mut self) -> &mut Sequencer {
        &mut self.sequencer
    }

    /// The voice allocator.
    #[must_use]
    pub const fn voices(&self) -> &VoiceAllocator {
        &self.voices
    }

    /// The voice allocator, mutably.
    pub fn voices_mut(&mut self) -> &mut VoiceAllocator {
        &mut self.voices
    }

    /// The mixer topology.
    #[must_use]
    pub const fn mixer(&self) -> &MixerGraph {
        &self.mixer
    }

    /// The mixer topology, mutably. Control thread only.
    pub fn mixer_mut(&mut self) -> &mut MixerGraph {
        &mut self.mixer
    }

    /// The automation layer.
    #[must_use]
    pub const fn automation(&self) -> &AutomationLayer {
        &self.automation
    }

    /// The automation layer, mutably.
    pub fn automation_mut(&mut self) -> &mut AutomationLayer {
        &mut self.automation
    }

    /// The post-mixer master chain.
    #[must_use]
    pub const fn master_chain(&self) -> &DspGraph {
        &self.master_chain
    }

    /// The post-mixer master chain, mutably. Control thread only.
    pub fn master_chain_mut(&mut self) -> &mut DspGraph {
        &mut self.master_chain
    }

    /// Records that a device driver has been attached.
    pub fn attach_driver(&mut self) {
        self.driver_attached = true;
    }

    /// Whether a device driver has been attached.
    #[must_use]
    pub const fn has_driver(&self) -> bool {
        self.driver_attached
    }

    /// Renders one block of audio into `out`, which holds `frames * 2`
    /// interleaved samples.
    ///
    /// This is the audio-thread entry point. It is allocation-free: all buffers
    /// were sized in [`Self::new`], and the automation pass is allocation-free
    /// by construction (`crate::automation`'s watching-allocator test enforces
    /// it).
    ///
    /// `frames` is authoritative and is clamped to the preallocated capacity
    /// (P6): a driver handing over a smaller or larger block never writes past
    /// the end.
    pub fn render_block(&mut self, out: &mut [f32], frames: usize) {
        let cap = self.config.block_size as usize;
        let n = frames.min(cap).min(out.len() / 2).min(self.output.len() / 2);
        if n == 0 {
            return;
        }

        let start_frame = self.transport.position_frames();
        let playing = self.transport.is_playing();

        // 1. Advance the transport.
        if playing {
            self.transport.advance_frames(n as i64);
        }

        // 2. Drain scheduled events into the voices.
        if playing {
            self.drain_sequencer(start_frame, n);
        }

        // 3. Evaluate automation once per block, at the block start.
        self.automation.player.advance_block(
            self.automation.lanes.lanes(),
            &mut self.automation.modulators,
            &self.automation.store,
            start_frame,
            n,
        );

        // 4. Render voices into the source bus.
        for s in &mut self.source[..n * 2] {
            *s = 0.0;
        }
        if playing {
            self.voices.render(&mut self.source[..n * 2], n);
        }

        // 5. Run the mixer, feeding the source bus into the first insert.
        self.mixer.silence_all();
        self.mixer_input(n);
        self.process_mixer(n);

        // 6. Copy the master bus out, then run the master chain.
        let master_copy = self.copy_master_out(n);
        if !self.master_chain.is_empty() {
            self.run_master_chain(n);
        }

        // 7. Publish to the caller.
        out[..master_copy].copy_from_slice(&self.output[..master_copy]);
        for s in out[master_copy..n * 2].iter_mut() {
            *s = 0.0;
        }

        // 8. Publish a lock-free status snapshot for the UI thread.
        let published = self.build_status();
        self.snapshot.publish(&published);
    }

    /// Moves events due in this block from the sequencer into the voices.
    fn drain_sequencer(&mut self, start_frame: i64, frames: usize) {
        let bpm = self.transport.bpm();
        let ppq = self.transport.ppq();
        let sample_rate = self.transport.sample_rate();
        let mut events = [ScheduledEvent::EMPTY; 64];
        let count = self.sequencer.collect_due(
            start_frame,
            frames,
            sample_rate,
            bpm,
            ppq,
            &mut events,
        );
        for event in events.iter().take(count) {
            match event.kind {
                EventKind::NoteOn => {
                    self.voices
                        .note_on(event.pitch, event.velocity, event.offset_frames as usize);
                }
                EventKind::NoteOff => {
                    self.voices
                        .note_off(event.pitch, event.offset_frames as usize);
                }
                EventKind::AllNotesOff => {
                    self.voices.all_notes_off();
                }
            }
        }
    }

    /// Sums the voice bus into the first insert channel's input.
    ///
    /// Track-to-channel routing arrives with S6; for S1 the source bus feeds
    /// the first insert channel, which already routes to master. This keeps the
    /// audible path simple and correct for a single instrument while leaving
    /// the console fully usable.
    fn mixer_input(&mut self, frames: usize) {
        let target = self.first_insert_channel();
        let Some(node) = self.mixer.node_mut(target) else {
            return;
        };
        let copy = (frames * 2).min(node.buffer.len()).min(self.source.len());
        for i in 0..copy {
            node.buffer[i] += self.source[i];
        }
    }

    /// The first live insert channel, or master as a fallback.
    fn first_insert_channel(&self) -> u32 {
        // The engine wired `source_channel` to master at creation, so that is
        // the one the voice bus must feed. A project's real routing (S6/S3) may
        // rewire it; until then this is the audible path.
        if self.mixer.exists(self.source_channel) {
            self.source_channel
        } else {
            crate::mixer::ChannelId::MASTER.get()
        }
    }

    /// Applies each channel's phase/fader/pan, meters it, and sums it into its
    /// destination, in topological order.
    ///
    /// Effect processors are applied by the caller (S5 wiring); this method is
    /// the console summing path, which is what the S1 acceptance ("internal
    /// mixing bus, per-sample summing") requires.
    ///
    /// Real-time safe: the only copy is into the preallocated `sum_scratch`.
    fn process_mixer(&mut self, frames: usize) {
        // Copy the order into the preallocated scratch so the borrow of
        // `self.mixer` ends before mutation and no allocation happens here.
        //
        // The scratch is sized to the mixer's channel capacity at creation;
        // adding channels is a control-thread operation that must be followed by
        // re-preparing the engine, so the capacity is stable on the audio path.
        let count = {
            let order = self.mixer.order();
            let count = order.len().min(self.order_scratch.len());
            self.order_scratch[..count].copy_from_slice(&order[..count]);
            count
        };
        for index in 0..count {
            let id = self.order_scratch[index];
            let (level, pan, phase_invert, audible, output) = {
                let Some(node) = self.mixer.node(id) else {
                    continue;
                };
                (
                    node.channel.linear_gain(),
                    node.channel.pan,
                    node.channel.phase_invert,
                    node.channel.audible,
                    node.output,
                )
            };

            let level = if audible { level } else { 0.0 };
            let phase = if phase_invert { -1.0 } else { 1.0 };
            let (pl, pr) = crate::mixer::PanLaw::ConstantPower3Db.gains(pan);
            let gl = level * phase * pl;
            let gr = level * phase * pr;

            if let Some(node) = self.mixer.node_mut(id) {
                let cap = frames.min(node.buffer.len() / 2);
                for i in 0..cap {
                    node.buffer[i * 2] *= gl;
                    node.buffer[i * 2 + 1] *= gr;
                }
                node.meter
                    .accumulate(&node.buffer[..cap * 2], cap, self.config.sample_rate);
            }

            if let Some(dst) = output {
                if dst == id {
                    continue;
                }
                // Copy the source into the preallocated scratch so the two
                // node borrows never overlap.
                let copy = {
                    let src = self
                        .mixer
                        .node(id)
                        .map(|n| n.buffer.as_slice())
                        .unwrap_or(&[]);
                    let copy = (frames * 2).min(src.len()).min(self.sum_scratch.len());
                    self.sum_scratch[..copy].copy_from_slice(&src[..copy]);
                    copy
                };
                if let Some(dst_node) = self.mixer.node_mut(dst) {
                    let copy = copy.min(dst_node.buffer.len());
                    for i in 0..copy {
                        dst_node.buffer[i] += self.sum_scratch[i];
                    }
                }
            }
        }
    }

    /// Copies the master bus into the engine's output buffer; returns how many
    /// interleaved samples were copied.
    fn copy_master_out(&mut self, frames: usize) -> usize {
        let copy = {
            let master = self
                .mixer
                .node(crate::mixer::ChannelId::MASTER.get())
                .map(|node| node.buffer.as_slice())
                .unwrap_or(&[]);
            let copy = (frames * 2).min(master.len()).min(self.output.len());
            self.output[..copy].copy_from_slice(&master[..copy]);
            copy
        };
        copy
    }

    /// Runs the post-mixer serial DSP chain on the output block.
    fn run_master_chain(&mut self, frames: usize) {
        let ctx = RenderContext::new(
            self.config.sample_rate as f32,
            frames,
            self.transport.position_frames(),
            self.transport.bpm(),
            self.transport.ppq(),
        );
        // De-interleave into the preallocated planar scratch, split into two
        // channel regions of `cap` samples each.
        let cap = self.config.block_size as usize;
        for i in 0..frames {
            self.chain_planar[i] = self.output[i * 2];
            self.chain_planar[cap + i] = self.output[i * 2 + 1];
        }
        // The chain works on an `AudioBuffer`, which borrows two slices of the
        // planar storage; splitting with `split_at_mut` gives disjoint regions
        // without a temporary Vec.
        let (left, right) = self.chain_planar.split_at_mut(cap);
        {
            let mut channel_views: [&mut [f32]; 2] = [&mut left[..frames], &mut right[..frames]];
            let mut buffer = AudioBuffer::new(&mut channel_views);
            self.master_chain.process(&mut buffer, &ctx);
        }
        for i in 0..frames {
            self.output[i * 2] = self.chain_planar[i];
            self.output[i * 2 + 1] = self.chain_planar[cap + i];
        }
    }

    /// Produces a status snapshot.
    #[must_use]
    pub fn status(&self) -> EngineStatus {
        self.build_status()
    }

    /// Builds the status view from live engine state.
    fn build_status(&self) -> EngineStatus {
        EngineStatus {
            playhead_frames: self.transport.position_frames(),
            bpm: self.transport.bpm(),
            cpu_load: 0.0,
            xrun_count: self.xrun_count,
            active_voices: self.voices.active_count() as u32,
            max_voices: self.voices.capacity() as u32,
            degrade_level: 0,
            state: self.transport.state_code(),
            driver_kind: if self.driver_attached { 1 } else { 3 },
            sample_rate: self.config.sample_rate,
            block_size: self.config.block_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_sample_rate_is_rejected() {
        assert!(Engine::new(EngineConfig {
            sample_rate: 0,
            ..EngineConfig::default()
        })
        .is_none());
        assert!(Engine::new(EngineConfig::default()).is_some());
    }

    #[test]
    fn block_size_is_clamped_to_the_documented_range() {
        let small = Engine::new(EngineConfig {
            block_size: 4,
            ..EngineConfig::default()
        })
        .unwrap();
        assert_eq!(small.config().block_size, 64);
        let large = Engine::new(EngineConfig {
            block_size: 10_000,
            ..EngineConfig::default()
        })
        .unwrap();
        assert_eq!(large.config().block_size, 2048);
    }

    #[test]
    fn a_silent_engine_renders_silence() {
        let mut e = Engine::new(EngineConfig::default()).unwrap();
        let mut out = alloc::vec![9.0f32; 256 * 2];
        e.render_block(&mut out, 256);
        assert!(out.iter().all(|s| *s == 0.0), "an idle engine must be silent");
    }

    #[test]
    fn a_zero_frame_block_is_a_no_op() {
        let mut e = Engine::new(EngineConfig::default()).unwrap();
        let mut out = [7.0f32; 8];
        e.render_block(&mut out, 0);
        assert!(out.iter().all(|s| *s == 7.0), "a zero-frame block must not write");
    }

    #[test]
    fn render_clamps_to_the_preallocated_capacity() {
        let mut e = Engine::new(EngineConfig {
            block_size: 64,
            ..EngineConfig::default()
        })
        .unwrap();
        let mut out = alloc::vec![0.0f32; 64 * 2];
        e.render_block(&mut out, 4096);
        assert_eq!(out.len(), 128);
    }

    #[test]
    fn transport_position_advances_while_playing() {
        let mut e = Engine::new(EngineConfig::default()).unwrap();
        e.transport_mut().play();
        let mut out = alloc::vec![0.0f32; 256 * 2];
        e.render_block(&mut out, 256);
        assert_eq!(e.transport().position_frames(), 256);
    }

    #[test]
    fn a_note_produces_audible_output() {
        let mut e = Engine::new(EngineConfig::default()).unwrap();
        e.sequencer_mut().push_note(60, 1.0, 0, 4_800);
        e.transport_mut().play();
        let mut out = alloc::vec![0.0f32; 256 * 2];
        e.render_block(&mut out, 256);
        let energy: f32 = out.iter().map(|s| s * s).sum();
        assert!(energy > 0.0, "a sounding note must produce output");
    }
}

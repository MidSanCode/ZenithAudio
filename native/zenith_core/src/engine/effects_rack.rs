//! The effect rack: instantiates the mixer's effect slots and runs them on the
//! audio thread (the S1 ↔ S3 ↔ S5 wiring point).
//!
//! # Why a separate rack, and not effects inside the mixer
//!
//! The mixer (`crate::mixer::EffectChain`) stores **descriptors** — a kind id
//! per slot — and deliberately depends on no particular effect being compiled
//! in. The effect suite (`crate::effects`) knows how to turn a kind id into a
//! processor, but knows nothing about channels or strips. The engine is the
//! only place that owns both, so it is the right place to hold the live
//! processors and bridge the two.
//!
//! # Control thread vs audio thread
//!
//! [`EffectRack::sync`] reconciles the live processors against the mixer's slot
//! contents. It allocates and instantiates effects, so it is
//! **control-thread only** and must be called after any mixer edit (ABI §7.3).
//! [`EffectRack::process`] is the audio-thread path: it walks the already-built
//! processors and allocates nothing.
//!
//! # Parameter flow
//!
//! Each effect publishes a descriptor table whose addresses embed the channel
//! and slot ([`ParameterAddress::effect`]). Before processing, the rack reads
//! the current value of every parameter from the S2 store and pushes it into
//! the processor, so an effect parameter is automatable without the effect
//! knowing the store exists.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::automation::parameter::ParameterAddress;
use crate::automation::store::ParameterStore;
use crate::effects::buffer::{AudioBuffer, RenderContext};
use crate::effects::{create_effect, EffectProcessor};
use crate::mixer::graph::MixerGraph;

/// One live processor, with the identity needed to detect a slot change.
struct RackSlot {
    /// Channel the slot belongs to.
    channel: u32,
    /// Slot position on the channel, `0..MAX_EFFECT_SLOTS`.
    slot: u8,
    /// Kind id currently instantiated.
    kind: u32,
    /// The processor.
    processor: Box<dyn EffectProcessor>,
}

/// The set of live effect processors, one per occupied mixer slot.
pub struct EffectRack {
    /// Processors, kept sorted by `(channel, slot)` so processing order is the
    /// strip order.
    slots: Vec<RackSlot>,
    /// Planar scratch for one stereo channel, `2 * max_block` samples, channel
    /// major. Preallocated so [`Self::process`] never allocates.
    scratch: Vec<f32>,
    /// Frame capacity of `scratch`.
    max_block: usize,
    /// Sample rate the rack was prepared at.
    sample_rate: f32,
}

impl EffectRack {
    /// Creates an empty rack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            scratch: Vec::new(),
            max_block: 0,
            sample_rate: 48_000.0,
        }
    }

    /// Prepares the rack for a sample rate and block size.
    ///
    /// Control thread. Allocates the planar scratch once.
    pub fn prepare(&mut self, sample_rate: f32, max_block: usize) {
        self.sample_rate = if sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        self.max_block = max_block;
        self.scratch = alloc::vec![0.0; 2 * max_block];
        for slot in &mut self.slots {
            slot.processor.prepare(self.sample_rate, max_block, 2);
        }
    }

    /// Number of live processors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the rack holds no processors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Total latency of `channel`'s effect chain, in samples.
    ///
    /// The sum of every processing slot's reported latency. Used by the engine's
    /// PDC to align this channel against the deepest one (PLAN §3.S4 item 1).
    #[must_use]
    pub fn channel_latency(&self, channel: u32) -> usize {
        self.slots
            .iter()
            .filter(|s| s.channel == channel)
            .map(|s| s.processor.latency_samples())
            .sum()
    }

    /// The per-channel latency vector, indexed by channel id.
    ///
    /// `channel_count` is the highest channel id plus one; index `i` is channel
    /// `i`'s total effect latency, or `0` when it has no processing slot.
    #[must_use]
    pub fn latency_by_channel(&self, channel_count: usize) -> alloc::vec::Vec<usize> {
        let mut out = alloc::vec![0usize; channel_count];
        for slot in &self.slots {
            if let Some(entry) = out.get_mut(slot.channel as usize) {
                *entry += slot.processor.latency_samples();
            }
        }
        out
    }

    /// Reconciles the live processors against `mixer`'s effect slots.
    ///
    /// Control thread only: this allocates and instantiates effects, so it must
    /// be called after mixer edits, never from the audio callback (ABI §7.3).
    ///
    /// A slot whose kind changed is rebuilt; a slot that became empty is
    /// dropped; a slot that appeared is created. An unknown kind is skipped
    /// (the mixer treats it as bypassed), so referencing an effect a newer
    /// build knows about does not break this one.
    pub fn sync(&mut self, mixer: &MixerGraph) {
        // Rebuild from scratch. The slot count is small (channels × 10), and
        // this runs on the control thread, so a fresh vector is simpler and
        // safer than in-place diffing.
        let mut rebuilt: Vec<RackSlot> = Vec::new();

        for channel in mixer.order() {
            let Some(node) = mixer.node(*channel) else {
                continue;
            };
            for (slot_index, slot) in node.effects.iter().enumerate() {
                let Some(kind) = slot.kind else {
                    continue;
                };
                if !crate::effects::is_known_effect(kind) {
                    // Unknown kind: leave it to the mixer's bypass behaviour.
                    continue;
                }
                let slot_u8 = slot_index as u8;
                // Reuse the existing processor when the kind is unchanged, so a
                // parameter edit does not reset a reverb tail.
                let existing = self
                    .slots
                    .iter()
                    .position(|s| s.channel == *channel && s.slot == slot_u8 && s.kind == kind);
                if let Some(index) = existing {
                    rebuilt.push(RackSlot {
                        channel: *channel,
                        slot: slot_u8,
                        kind,
                        // Move the processor out by swapping with a placeholder:
                        // `Vec::remove` needs an owned value and the rack is
                        // being rebuilt anyway.
                        processor: self.slots[index].processor_dummy_take(),
                    });
                } else {
                    let address = ParameterAddress::effect(*channel, slot_u8, 0);
                    if let Some(mut processor) = create_effect(kind, address) {
                        processor.prepare(self.sample_rate, self.max_block, 2);
                        rebuilt.push(RackSlot {
                            channel: *channel,
                            slot: slot_u8,
                            kind,
                            processor,
                        });
                    }
                }
            }
        }

        self.slots = rebuilt;
    }

    /// Processes `channel`'s interleaved stereo buffer through its effect slots.
    ///
    /// `store` supplies the current parameter values, pushed into each
    /// processor before it runs. Real-time safe: no allocation, no locking.
    ///
    /// `buffer` is interleaved `L, R, …` with `frames * 2` samples, exactly the
    /// layout the mixer's channel buffers use.
    pub fn process(
        &mut self,
        channel: u32,
        buffer: &mut [f32],
        frames: usize,
        store: &ParameterStore,
    ) {
        if self.slots.is_empty() || frames == 0 {
            return;
        }
        let n = frames.min(self.max_block).min(buffer.len() / 2);
        if n == 0 {
            return;
        }

        // Does this channel have any processing slot? Skip the layout
        // conversion entirely when not, which is the common case.
        let has_work = self
            .slots
            .iter()
            .any(|s| s.channel == channel && !s.processor.is_bypassed());
        if !has_work {
            return;
        }

        // De-interleave into planar scratch.
        for i in 0..n {
            self.scratch[i] = buffer[i * 2];
            self.scratch[self.max_block + i] = buffer[i * 2 + 1];
        }

        let ctx = RenderContext {
            sample_rate: self.sample_rate,
            frame: 0,
            bpm: 120.0,
            ppq: 960,
            frames: n,
        };

        for slot in self.slots.iter_mut().filter(|s| s.channel == channel) {
            // Snapshot the parameter ordinals first, so the immutable borrow of
            // `parameters()` ends before `set_parameter` takes `&mut`. The
            // parameter count is bounded and small, so a stack array avoids
            // touching the heap on the audio thread.
            let channel = slot.channel;
            let slot_index = slot.slot;
            let mut subs = [0u16; 64];
            let mut count = 0usize;
            for descriptor in slot.processor.parameters() {
                if count == subs.len() {
                    break;
                }
                subs[count] = descriptor.address.sub & 0x00FF;
                count += 1;
            }
            for sub in &subs[..count] {
                if let Some(value) = store.read(ParameterAddress::effect(channel, slot_index, *sub))
                {
                    slot.processor.set_parameter(*sub, value);
                }
            }

            // The scratch is contiguous and `max_block` is the stride, so the
            // two channel regions are disjoint slices of the same buffer.
            let stride = self.max_block;
            let (left, right) = self.scratch.split_at_mut(stride);
            let mut views: [&mut [f32]; 2] = [&mut left[..n], &mut right[..n]];
            let mut audio = AudioBuffer::new(&mut views);
            slot.processor.process(&mut audio, &ctx);
        }

        // Re-interleave.
        for i in 0..n {
            buffer[i * 2] = self.scratch[i];
            buffer[i * 2 + 1] = self.scratch[self.max_block + i];
        }
    }
}

impl Default for EffectRack {
    fn default() -> Self {
        Self::new()
    }
}

impl RackSlot {
    /// Takes the processor out, leaving a cheap placeholder.
    ///
    /// `sync` rebuilds the whole list, so the source slot is discarded
    /// immediately after; this exists only to move ownership without a clone
    /// (an `EffectProcessor` is not `Clone`).
    fn processor_dummy_take(&mut self) -> Box<dyn EffectProcessor> {
        // A zero-latency pass-through stand-in, immediately overwritten in the
        // rebuilt list. Using a real (if trivial) processor avoids `Option`
        // unwraps on the audio path.
        core::mem::replace(&mut self.processor, Box::new(PassThrough::new()))
    }
}

/// A trivial pass-through used only as a move placeholder during `sync`.
struct PassThrough {
    descriptor: &'static crate::effects::EffectDescriptor,
}

impl PassThrough {
    fn new() -> Self {
        Self {
            descriptor: &PASS_THROUGH_DESCRIPTOR,
        }
    }
}

/// The descriptor for [`PassThrough`].
static PASS_THROUGH_DESCRIPTOR: crate::effects::EffectDescriptor =
    crate::effects::EffectDescriptor {
        kind: u32::MAX,
        key: "passthrough",
        label: "Pass Through",
        category: crate::effects::EffectCategory::Utility,
        first_param: 0,
        param_count: 0,
        has_latency: false,
        is_analysis_only: false,
    };

impl EffectProcessor for PassThrough {
    fn descriptor(&self) -> &'static crate::effects::EffectDescriptor {
        self.descriptor
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn process(&mut self, _buffer: &mut AudioBuffer<'_>, _ctx: &RenderContext) {}
    fn reset(&mut self) {}
    fn latency_samples(&self) -> usize {
        0
    }
    fn parameters(&self) -> &[crate::automation::parameter::ParameterDescriptor] {
        &[]
    }
    fn set_parameter(&mut self, _sub: u16, _value: f32) {}
    fn get_parameter(&self, _sub: u16) -> Option<f32> {
        None
    }
    fn set_bypassed(&mut self, _bypassed: bool) {}
    fn set_wet(&mut self, _wet: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::parameter::ParameterKind;
    use crate::mixer::graph::MixerGraph;

    #[test]
    fn an_empty_rack_has_no_processors() {
        let rack = EffectRack::new();
        assert!(rack.is_empty());
        assert_eq!(rack.len(), 0);
    }

    #[test]
    fn sync_creates_a_processor_for_an_occupied_slot() {
        let mut mixer = MixerGraph::new(256);
        let channel = 10u32; // an insert channel
        let kind = crate::effects::registry::KIND_COMPRESSOR;
        mixer.node_mut(channel).unwrap().effects.insert(0, kind);

        let mut rack = EffectRack::new();
        rack.prepare(48_000.0, 256);
        rack.sync(&mixer);
        assert_eq!(rack.len(), 1, "one occupied slot => one processor");

        // Removing the slot and re-syncing drops it.
        mixer.node_mut(channel).unwrap().effects.remove(0);
        rack.sync(&mixer);
        assert!(rack.is_empty());
    }

    #[test]
    fn an_unknown_kind_is_skipped_rather_than_panicking() {
        let mut mixer = MixerGraph::new(256);
        mixer.node_mut(10).unwrap().effects.insert(0, 0xDEAD_BEEF);
        let mut rack = EffectRack::new();
        rack.prepare(48_000.0, 256);
        rack.sync(&mixer);
        assert!(rack.is_empty(), "an unknown kind is left to the bypass path");
    }

    #[test]
    fn processing_a_channel_with_no_slots_is_a_no_op() {
        let mut rack = EffectRack::new();
        rack.prepare(48_000.0, 256);
        let store = ParameterStore::default();
        let mut buffer = alloc::vec![0.5f32; 16];
        rack.process(10, &mut buffer, 8, &store);
        assert!(buffer.iter().all(|s| *s == 0.5));
    }

    #[test]
    fn an_effect_runs_and_changes_the_signal() {
        let mut mixer = MixerGraph::new(256);
        let channel = 10u32;
        // An algorithmic reverb at its defaults is not a pass-through: with a
        // 30% mix the dry impulse is scaled and a wet tail follows. (Saturation
        // is bypassed at 0 dB drive by design, so it would be a poor probe.)
        mixer
            .node_mut(channel)
            .unwrap()
            .effects
            .insert(0, crate::effects::registry::KIND_REVERB_ALGORITHMIC);

        let mut rack = EffectRack::new();
        rack.prepare(48_000.0, 256);
        rack.sync(&mixer);
        assert_eq!(rack.len(), 1);

        let store = ParameterStore::default();
        // An impulse in the left channel, silence elsewhere.
        let mut buffer = alloc::vec![0.0f32; 256 * 2];
        buffer[0] = 1.0;
        let before = buffer.clone();
        rack.process(channel, &mut buffer, 256, &store);

        // The reverb's wet/dry mix alone guarantees the signal changed.
        assert!(
            buffer
                .iter()
                .zip(before.iter())
                .any(|(a, b)| (a - b).abs() > 1e-6),
            "the effect should have altered the impulse"
        );
    }

    #[test]
    fn parameter_addresses_use_the_effect_kind() {
        let address = ParameterAddress::effect(7, 3, 42);
        assert_eq!(address.kind, ParameterKind::Effect);
        assert_eq!(address.index, 7);
        assert_eq!(address.effect_slot(), 3);
    }

    #[test]
    fn repeated_processing_is_stable() {
        // A smoke check on the audio path: many calls must not resize the
        // caller's buffer, grow internal state, or emit a non-finite sample.
        // The stronger zero-allocation guarantee for the automation path is
        // enforced by `crate::automation`'s watching allocator; the rack's
        // `process` only reads its preallocated scratch and writes in place.
        let mut mixer = MixerGraph::new(256);
        mixer
            .node_mut(10)
            .unwrap()
            .effects
            .insert(0, crate::effects::registry::KIND_REVERB_ALGORITHMIC);
        let mut rack = EffectRack::new();
        rack.prepare(48_000.0, 256);
        rack.sync(&mixer);

        let mut buffer = alloc::vec![0.0f32; 256 * 2];
        let store = ParameterStore::default();
        for _ in 0..64 {
            buffer[0] = 1.0; // retrigger the reverb
            rack.process(10, &mut buffer, 256, &store);
        }
        assert_eq!(buffer.len(), 512, "process must not resize the buffer");
        assert!(buffer.iter().all(|s| s.is_finite()), "output must stay finite");
    }
}

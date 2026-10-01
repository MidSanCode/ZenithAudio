//! C ABI surface for the mixer (ABI §6.5, S3).
//!
//! # Handle design
//!
//! Like S2's automation surface, the mixer exposes its own opaque handle:
//! [`ZenithMixer`]. It owns a [`MixerGraph`] — channels, sends, effect slots,
//! meters and routing — and **nothing about the audio device or the DSP graph**.
//!
//! This is what lets S3 land before S1 finishes:
//!
//! * the whole console is constructible, queryable and testable today;
//! * when S1's `ZenithEngine` exists it will *own* a `ZenithMixer` rather than
//!   duplicating channel state, so wiring is a field plus one `process_block`
//!   call at the block boundary;
//! * Dart written against these functions keeps working, because the
//!   engine-level functions will front the same data.
//!
//! # Real-time safety of each entry point (ABI §7.3)
//!
//! | Real-time safe (any thread) | Control thread only |
//! |---|---|
//! | `meter_read`, `channel_get`, `channel_ids`, `stats` | `_create`, `_destroy` |
//! | `send_get`, `effect_get`, `can_connect` | every `_*_set`, `_connect`, `_add_*` |
//!
//! Structural edits (adding channels, rerouting) reallocate and re-walk the
//! topology, so they are explicitly **not** real-time safe. The plan's
//! requirement is that cycles be rejected at *build* time, which is exactly
//! what `_connect` does — it validates before committing, so the audio thread
//! never has to check.
//!
//! Every entry point is `catch_unwind`-guarded (P4) and every pointer is
//! validated before use (P2, P10). No entry point returns `null` for failure.

// See `param_api.rs` for why this lint is allowed here: the exported functions
// keep C signatures, validate every pointer, and document safety per function.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::mem::size_of;

use crate::ffi::types::{
    zenith_channel_flags, zenith_send_tap, ZenithMeterSnapshot, ZenithMixerChannel,
    ZenithMixerEffectSlot, ZenithMixerSend, ZenithMixerStats, ZENITH_CHANNEL_NONE,
    ZENITH_KIND_NONE,
};
use crate::guard;
use crate::mixer::{
    ChannelId, MixerGraph, MixerTopologyError, SendTap, MAX_EFFECT_SLOTS, MAX_SENDS_PER_CHANNEL,
};
use crate::Status;

/// Opaque handle to a mixer.
///
/// Dart holds a `*mut` to this and never interprets its memory (P2). Created by
/// [`zenith_mixer_create`], freed by [`zenith_mixer_destroy`].
pub struct ZenithMixer {
    /// The console topology and its values.
    graph: MixerGraph,
    /// Sample rate the meters were last advanced at.
    sample_rate: u32,
}

impl ZenithMixer {
    /// Creates a mixer with the default channel counts.
    fn new(max_frames: usize, sample_rate: u32) -> Self {
        Self {
            graph: MixerGraph::new(max_frames),
            sample_rate,
        }
    }
}

/// Maps a topology error onto an ABI status code.
///
/// A cycle is `InvalidArg` rather than a bespoke code: the caller's argument
/// (the requested connection) is what is wrong, and adding a code would change
/// the enum's published discriminant set for no gain.
fn topology_status(error: MixerTopologyError) -> Status {
    match error {
        MixerTopologyError::UnknownChannel(_) => Status::NotFound,
        MixerTopologyError::WouldCycle => Status::InvalidArg,
        MixerTopologyError::TooDeep => Status::OutOfRange,
        MixerTopologyError::Capacity => Status::OutOfRange,
        MixerTopologyError::MasterIsFixed => Status::InvalidArg,
        MixerTopologyError::SelfRoute => Status::InvalidArg,
    }
}

/// Writes `value` through `out`, or reports a null pointer.
///
/// # Safety
///
/// `out` must be either null or a valid, writable, aligned pointer.
unsafe fn write_out<T>(out: *mut T, value: T) -> Status {
    if out.is_null() {
        return Status::NullPointer;
    }
    // SAFETY: the caller guarantees `out` is writable and aligned.
    unsafe { out.write(value) };
    Status::Ok
}

/// Builds the ABI mirror of a channel.
fn mirror_channel(graph: &MixerGraph, id: u32) -> Option<ZenithMixerChannel> {
    let node = graph.node(id)?;
    let channel = &node.channel;

    let mut flags = 0u32;
    if channel.mute {
        flags |= zenith_channel_flags::MUTED;
    }
    if channel.solo {
        flags |= zenith_channel_flags::SOLO;
    }
    if channel.phase_invert {
        flags |= zenith_channel_flags::PHASE_INVERT;
    }
    if channel.audible {
        flags |= zenith_channel_flags::AUDIBLE;
    }
    flags |= zenith_channel_flags::ALIVE;

    Some(ZenithMixerChannel {
        id,
        role: u32::from(channel.role),
        gain_db: channel.gain_db,
        pan: channel.pan,
        output: node.output.unwrap_or(ZENITH_CHANNEL_NONE),
        flags,
        effect_count: node.effects.occupied() as u32,
        active_sends: node.sends.active().count() as u32,
    })
}

/// Creates a mixer with the default channel counts.
///
/// `max_frames` is the largest block the mixer will be asked to process; every
/// channel's scratch buffer is allocated once, here, so the process path never
/// allocates (P5). `sample_rate` seeds the meters.
///
/// # Safety
///
/// `out` must be null or a valid pointer to a `*mut ZenithMixer`.
#[no_mangle]
pub extern "C" fn zenith_mixer_create(
    max_frames: u32,
    sample_rate: u32,
    out: *mut *mut ZenithMixer,
) -> i32 {
    guard(|| {
        if max_frames == 0 {
            return Status::OutOfRange;
        }
        let mixer = Box::new(ZenithMixer::new(max_frames as usize, sample_rate));
        // SAFETY: `out` is validated by `write_out`.
        unsafe { write_out(out, Box::into_raw(mixer)) }
    })
    .code()
}

/// Destroys a mixer created by [`zenith_mixer_create`].
///
/// Passing null is a successful no-op, so Dart can call it unconditionally in a
/// `dispose` path without a null check.
///
/// # Safety
///
/// `mixer` must be null or a pointer previously returned by
/// [`zenith_mixer_create`] that has not already been destroyed.
#[no_mangle]
pub extern "C" fn zenith_mixer_destroy(mixer: *mut ZenithMixer) -> i32 {
    guard(|| {
        if mixer.is_null() {
            return Status::Ok;
        }
        // SAFETY: the caller guarantees this pointer came from `Box::into_raw`
        // in `create` and has not been freed.
        drop(unsafe { Box::from_raw(mixer) });
        Status::Ok
    })
    .code()
}

/// Returns the number of live channels, master included.
///
/// # Safety
///
/// `mixer` must be null or a live mixer pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_channel_count(mixer: *const ZenithMixer) -> u32 {
    // SAFETY: validated below; a null pointer yields 0 rather than a crash so
    // a UI polling during teardown sees an empty mixer instead of UB.
    match unsafe { mixer.as_ref() } {
        Some(m) => m.graph.len() as u32,
        None => 0,
    }
}

/// Reads an aggregate summary of the mixer.
///
/// # Safety
///
/// `mixer` must be null or a live mixer pointer; `out` must be null or a valid
/// writable pointer to a [`ZenithMixerStats`].
#[no_mangle]
pub extern "C" fn zenith_mixer_stats(mixer: *const ZenithMixer, out: *mut ZenithMixerStats) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };

        let mut channels = 0u32;
        let mut active_sends = 0u32;
        let mut effects = 0u32;
        let mut has_solo = 0u32;
        let mut max_depth = 0u32;

        for id in m.graph.order() {
            let Some(node) = m.graph.node(*id) else {
                continue;
            };
            channels += 1;
            active_sends += node.sends.active().count() as u32;
            effects += node.effects.occupied() as u32;
            if node.channel.solo {
                has_solo = 1;
            }
            let depth = m.graph.max_depth_through(*id) as u32;
            if depth > max_depth {
                max_depth = depth;
            }
        }

        // SAFETY: caller guarantees `out` is writable and aligned.
        unsafe {
            write_out(
                out,
                ZenithMixerStats {
                    channels,
                    active_sends,
                    effects,
                    has_solo,
                    max_depth,
                    _reserved_0: 0,
                },
            )
        }
    })
    .code()
}

/// Adds an insert channel and writes its index to `out_id`.
///
/// The new channel is routed to master, the only default that cannot cycle.
///
/// # Safety
///
/// `mixer` must be a live pointer; `out_id` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_add_channel(mixer: *mut ZenithMixer, out_id: *mut u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.add_channel() {
            // SAFETY: caller guarantees `out_id` is writable and aligned.
            Ok(id) => unsafe { write_out(out_id, id) },
            Err(error) => topology_status(error),
        }
    })
    .code()
}

/// Removes a channel, rerouting its upstream peers to master.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_remove_channel(mixer: *mut ZenithMixer, id: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.remove_channel(id) {
            Ok(()) => Status::Ok,
            Err(error) => topology_status(error),
        }
    })
    .code()
}

/// Reads one channel's parameters.
///
/// # Safety
///
/// `mixer` must be a live pointer; `out` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_channel_get(
    mixer: *const ZenithMixer,
    id: u32,
    out: *mut ZenithMixerChannel,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };
        match mirror_channel(&m.graph, id) {
            // SAFETY: caller guarantees `out` is writable and aligned.
            Some(mirror) => unsafe { write_out(out, mirror) },
            None => Status::NotFound,
        }
    })
    .code()
}

/// Reads every channel's index into `out_ids`, in processing order.
///
/// Writes at most `capacity` entries and stores the total channel count (which
/// may exceed `capacity`) through `out_total`, so a caller can size a second
/// call correctly. This is the standard two-call enumeration pattern.
///
/// # Safety
///
/// `out_ids` must be null or point to `capacity` writable `u32`s; `out_total`
/// must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_channel_ids(
    mixer: *const ZenithMixer,
    out_ids: *mut u32,
    capacity: u32,
    out_total: *mut u32,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };
        let order = m.graph.order();
        let total = order.len() as u32;

        if !out_ids.is_null() {
            let n = (capacity as usize).min(order.len());
            // SAFETY: the caller guarantees `out_ids` has room for `capacity`
            // entries, and `n` never exceeds that.
            for (i, id) in order.iter().take(n).enumerate() {
                unsafe { out_ids.add(i).write(*id) };
            }
        }

        // SAFETY: caller guarantees `out_total` is writable and aligned.
        unsafe { write_out(out_total, total) }
    })
    .code()
}

/// Sets a channel's fader position in decibels.
///
/// The value is clamped to the legal fader range rather than rejected: this is
/// called from a live UI gesture, and a drag past the end of the travel should
/// stop at the end, not fail.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_set_gain_db(mixer: *mut ZenithMixer, id: u32, gain_db: f32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.node_mut(id) {
            Some(node) => {
                node.channel.set_gain_db(gain_db);
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Sets a channel's pan position, clamped to `-1.0..=1.0`.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_set_pan(mixer: *mut ZenithMixer, id: u32, pan: f32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.node_mut(id) {
            Some(node) => {
                node.channel.set_pan(pan);
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Sets a channel's mute flag and republishes audibility.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_set_mute(mixer: *mut ZenithMixer, id: u32, muted: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.set_mute(id, muted != 0) {
            Ok(()) => Status::Ok,
            Err(error) => topology_status(error),
        }
    })
    .code()
}

/// Sets a channel's solo flag and republishes audibility.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_set_solo(mixer: *mut ZenithMixer, id: u32, solo: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.set_solo(id, solo != 0) {
            Ok(()) => Status::Ok,
            Err(error) => topology_status(error),
        }
    })
    .code()
}

/// Sets a channel's polarity invert flag.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_set_phase_invert(
    mixer: *mut ZenithMixer,
    id: u32,
    inverted: u32,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.node_mut(id) {
            Some(node) => {
                node.channel.phase_invert = inverted != 0;
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Routes `src` into `dst`, rejecting any connection that would cycle.
///
/// This is the S3 acceptance check made reachable from Dart: a cycle is refused
/// with `InvalidArg` and the graph is left exactly as it was, so the caller can
/// surface the reason instead of silently producing feedback.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_connect(mixer: *mut ZenithMixer, src: u32, dst: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.connect(src, dst) {
            Ok(()) => Status::Ok,
            Err(error) => topology_status(error),
        }
    })
    .code()
}

/// Whether `src` can reach `dst` by following output links.
///
/// Lets the UI grey out a connection that would cycle *before* the user makes
/// it, rather than letting them try and then reporting a failure.
///
/// # Safety
///
/// `mixer` must be null or a live pointer; `out` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_can_connect(
    mixer: *const ZenithMixer,
    src: u32,
    dst: u32,
    out: *mut u32,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };
        let allowed = src != dst
            && m.graph.exists(src)
            && m.graph.exists(dst)
            && src != ChannelId::MASTER.get()
            // Connecting a channel to itself through a chain would cycle.
            && !m.graph.reaches(dst, src);

        // SAFETY: caller guarantees `out` is writable and aligned.
        unsafe { write_out(out, u32::from(allowed)) }
    })
    .code()
}

/// Returns a channel to master.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_disconnect(mixer: *mut ZenithMixer, id: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.disconnect(id) {
            Ok(()) => Status::Ok,
            Err(error) => topology_status(error),
        }
    })
    .code()
}

/// Configures one of a channel's four sends.
///
/// `destination` may be [`ZENITH_CHANNEL_NONE`] to leave the send unrouted, in
/// which case it stays silent however its `enabled` flag is set.
///
/// # Safety
///
/// `mixer` must be a live pointer; `destination` must be a live channel index
/// or [`ZENITH_CHANNEL_NONE`].
#[no_mangle]
pub extern "C" fn zenith_mixer_send_set(
    mixer: *mut ZenithMixer,
    id: u32,
    slot: u32,
    enabled: u32,
    tap: u32,
    level_db: f32,
    destination: u32,
) -> i32 {
    guard(|| {
        if slot as usize >= MAX_SENDS_PER_CHANNEL {
            return Status::OutOfRange;
        }
        if !matches!(
            tap,
            zenith_send_tap::POST_FADER | zenith_send_tap::PRE_FADER
        ) {
            return Status::InvalidArg;
        }
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        if destination != ZENITH_CHANNEL_NONE && !m.graph.exists(destination) {
            return Status::NotFound;
        }

        let Some(node) = m.graph.node_mut(id) else {
            return Status::NotFound;
        };
        let Some(send) = node.sends.get_mut(slot as usize) else {
            return Status::OutOfRange;
        };

        send.enabled = enabled != 0;
        send.tap = if tap == zenith_send_tap::PRE_FADER {
            SendTap::PreFader
        } else {
            SendTap::PostFader
        };
        send.set_level_db(level_db);
        send.destination = if destination == ZENITH_CHANNEL_NONE {
            None
        } else {
            Some(destination)
        };
        Status::Ok
    })
    .code()
}

/// Reads one of a channel's sends.
///
/// # Safety
///
/// `mixer` must be a live pointer; `out` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_send_get(
    mixer: *const ZenithMixer,
    id: u32,
    slot: u32,
    out: *mut ZenithMixerSend,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };
        let Some(node) = m.graph.node(id) else {
            return Status::NotFound;
        };
        let Some(send) = node.sends.get(slot as usize) else {
            return Status::OutOfRange;
        };
        let mirror = ZenithMixerSend {
            enabled: u32::from(send.is_active()),
            tap: u32::from(send.tap),
            level_db: send.level_db,
            destination: send.destination.unwrap_or(ZENITH_CHANNEL_NONE),
        };
        // SAFETY: caller guarantees `out` is writable and aligned.
        unsafe { write_out(out, mirror) }
    })
    .code()
}

/// Inserts an effect at `slot`, shifting later slots right.
///
/// Refused when the chain is full, so adding one effect too many cannot
/// silently drop the user's last effect.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_effect_insert(
    mixer: *mut ZenithMixer,
    id: u32,
    slot: u32,
    kind: u32,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        let Some(node) = m.graph.node_mut(id) else {
            return Status::NotFound;
        };
        if node.effects.insert(slot as usize, kind) {
            Status::Ok
        } else {
            Status::OutOfRange
        }
    })
    .code()
}

/// Removes the effect at `slot`, shifting later slots left.
///
/// Removing an empty slot is a no-op success: the caller's intent ("this slot
/// should be empty") already holds.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_effect_remove(mixer: *mut ZenithMixer, id: u32, slot: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        let Some(node) = m.graph.node_mut(id) else {
            return Status::NotFound;
        };
        match node.effects.remove(slot as usize) {
            Some(_) => Status::Ok,
            None => Status::OutOfRange,
        }
    })
    .code()
}

/// Moves the effect at `from` to `to`, carrying its settings with it.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_effect_move(
    mixer: *mut ZenithMixer,
    id: u32,
    from: u32,
    to: u32,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        let Some(node) = m.graph.node_mut(id) else {
            return Status::NotFound;
        };
        if node.effects.move_slot(from as usize, to as usize) {
            Status::Ok
        } else {
            Status::OutOfRange
        }
    })
    .code()
}

/// Updates one effect slot's bypass, wet and sidechain.
///
/// `kind` is not settable here: changing what an effect *is* is an insert or a
/// remove, and allowing both paths would make the two states ambiguous.
///
/// # Safety
///
/// `mixer` must be a live pointer; `sidechain` must be a live channel index or
/// [`ZENITH_CHANNEL_NONE`].
#[no_mangle]
pub extern "C" fn zenith_mixer_effect_configure(
    mixer: *mut ZenithMixer,
    id: u32,
    slot: u32,
    bypassed: u32,
    wet: f32,
    sidechain: u32,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        if sidechain != ZENITH_CHANNEL_NONE && !m.graph.exists(sidechain) {
            return Status::NotFound;
        }
        let Some(node) = m.graph.node_mut(id) else {
            return Status::NotFound;
        };
        let Some(effect) = node.effects.get_mut(slot as usize) else {
            return Status::OutOfRange;
        };
        effect.bypassed = bypassed != 0;
        effect.set_wet(wet);
        effect.sidechain_source = if sidechain == ZENITH_CHANNEL_NONE {
            None
        } else {
            Some(sidechain)
        };
        Status::Ok
    })
    .code()
}

/// Reads one effect slot.
///
/// An empty slot reads back with `kind == ZENITH_KIND_NONE` and `Status::Ok`,
/// because an empty slot is a legitimate state rather than a missing resource.
///
/// # Safety
///
/// `mixer` must be a live pointer; `out` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_effect_get(
    mixer: *const ZenithMixer,
    id: u32,
    slot: u32,
    out: *mut ZenithMixerEffectSlot,
) -> i32 {
    guard(|| {
        if slot as usize >= MAX_EFFECT_SLOTS {
            return Status::OutOfRange;
        }
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };
        let Some(node) = m.graph.node(id) else {
            return Status::NotFound;
        };
        let Some(effect) = node.effects.get(slot as usize) else {
            return Status::OutOfRange;
        };
        // SAFETY: caller guarantees `out` is writable and aligned.
        unsafe {
            write_out(
                out,
                ZenithMixerEffectSlot {
                    index: slot,
                    kind: effect.kind.unwrap_or(ZENITH_KIND_NONE),
                    bypassed: u32::from(effect.bypassed),
                    wet: effect.wet,
                    sidechain: effect.sidechain_source.unwrap_or(ZENITH_CHANNEL_NONE),
                },
            )
        }
    })
    .code()
}

/// Reads a channel's current level meter.
///
/// Lock-free and safe to call at any time, including while audio runs (ABI
/// §7.3): the audio thread publishes into plain fields and this reads a
/// snapshot that may be one block old. For a meter that is not merely
/// acceptable but preferable — taking a lock here would risk a dropout.
///
/// # Safety
///
/// `mixer` must be a live pointer; `out` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_mixer_meter_read(
    mixer: *const ZenithMixer,
    id: u32,
    out: *mut ZenithMeterSnapshot,
) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_ref() }) else {
            return Status::NullPointer;
        };
        match m.graph.meter(id) {
            // SAFETY: caller guarantees `out` is writable and aligned.
            Some(snapshot) => unsafe { write_out(out, snapshot.into()) },
            None => Status::NotFound,
        }
    })
    .code()
}

/// Resets a channel's meter.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_meter_reset(mixer: *mut ZenithMixer, id: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        match m.graph.node_mut(id) {
            Some(node) => {
                node.meter.reset();
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Advances every meter by one block of silence.
///
/// The engine calls this for channels it did not write this block, so a stopped
/// channel's meter falls instead of freezing. Exposed so the Dart-side meter
/// can be exercised without a running engine.
///
/// # Safety
///
/// `mixer` must be a live pointer.
#[no_mangle]
pub extern "C" fn zenith_mixer_meter_age(mixer: *mut ZenithMixer, frames: u32) -> i32 {
    guard(|| {
        let Some(m) = (unsafe { mixer.as_mut() }) else {
            return Status::NullPointer;
        };
        if frames == 0 {
            return Status::OutOfRange;
        }
        m.graph.age_meters(frames as usize, m.sample_rate);
        Status::Ok
    })
    .code()
}

/// Reports the size of [`ZenithMixerChannel`] for the ABI size assertion.
///
/// ABI §2.3 requires every `#[repr(C)]` struct to have a `zenith_sizeof_<T>`
/// export so Dart can compare its own `sizeOf<T>()` and fail loudly on drift
/// rather than silently misreading memory (P8).
#[no_mangle]
pub extern "C" fn zenith_sizeof_mixer_channel() -> usize {
    size_of::<ZenithMixerChannel>()
}

/// Reports the size of [`ZenithMixerSend`]. See [`zenith_sizeof_mixer_channel`].
#[no_mangle]
pub extern "C" fn zenith_sizeof_mixer_send() -> usize {
    size_of::<ZenithMixerSend>()
}

/// Reports the size of [`ZenithMixerEffectSlot`].
/// See [`zenith_sizeof_mixer_channel`].
#[no_mangle]
pub extern "C" fn zenith_sizeof_mixer_effect_slot() -> usize {
    size_of::<ZenithMixerEffectSlot>()
}

/// Reports the size of [`ZenithMeterSnapshot`].
/// See [`zenith_sizeof_mixer_channel`].
#[no_mangle]
pub extern "C" fn zenith_sizeof_meter_snapshot() -> usize {
    size_of::<ZenithMeterSnapshot>()
}

/// Reports the size of [`ZenithMixerStats`]. See [`zenith_sizeof_mixer_channel`].
#[no_mangle]
pub extern "C" fn zenith_sizeof_mixer_stats() -> usize {
    size_of::<ZenithMixerStats>()
}

/// Reports the send slots available on every channel.
#[no_mangle]
pub extern "C" fn zenith_mixer_max_sends() -> u32 {
    MAX_SENDS_PER_CHANNEL as u32
}

/// Reports the insert slots available on every channel.
#[no_mangle]
pub extern "C" fn zenith_mixer_max_effect_slots() -> u32 {
    MAX_EFFECT_SLOTS as u32
}

/// Internal helper exercised by the unit tests below.
///
/// Kept out of the exported surface: it exists so the tests can inspect a
/// channel without going through raw pointers, which keeps a failure pointing
/// at logic rather than at pointer plumbing.
#[cfg(test)]
impl ZenithMixer {
    fn channel_mirror_for_test(&self, id: u32) -> Option<ZenithMixerChannel> {
        mirror_channel(&self.graph, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::types::zenith_channel_role;

    fn mixer() -> ZenithMixer {
        ZenithMixer::new(256, 48_000)
    }

    #[test]
    fn a_new_mixer_has_the_default_channel_counts() {
        let m = mixer();
        assert_eq!(
            m.graph.len(),
            64 + 8 + 1,
            "64 inserts + 8 returns + 1 master"
        );
    }

    #[test]
    fn the_master_channel_mirrors_with_the_master_role() {
        let m = mixer();
        let mirror = m
            .channel_mirror_for_test(ChannelId::MASTER.get())
            .expect("master exists");
        assert_eq!(mirror.role, zenith_channel_role::MASTER);
        assert_eq!(mirror.output, ZENITH_CHANNEL_NONE, "master feeds nothing");
        assert_ne!(mirror.flags & zenith_channel_flags::ALIVE, 0);
    }

    #[test]
    fn an_unknown_channel_mirrors_to_none() {
        let m = mixer();
        assert!(m.channel_mirror_for_test(99_999).is_none());
    }

    #[test]
    fn the_none_sentinel_is_distinguishable_from_master() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        let mirror = m.channel_mirror_for_test(id).expect("just added");
        assert_eq!(
            mirror.output,
            ChannelId::MASTER.get(),
            "a new channel routes to master, whose index is 0"
        );
        assert_ne!(
            mirror.output, ZENITH_CHANNEL_NONE,
            "master must not be confused with 'no output'"
        );
    }

    #[test]
    fn channel_flags_track_mute_solo_phase_and_audibility() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        m.graph.set_mute(id, true).expect("known channel");
        m.graph.set_solo(id, true).expect("known channel");
        m.graph.node_mut(id).expect("live").channel.phase_invert = true;

        let mirror = m.channel_mirror_for_test(id).expect("live");
        assert_ne!(mirror.flags & zenith_channel_flags::MUTED, 0);
        assert_ne!(mirror.flags & zenith_channel_flags::SOLO, 0);
        assert_ne!(mirror.flags & zenith_channel_flags::PHASE_INVERT, 0);
        // Soloed, so still audible despite the mute.
        assert_ne!(mirror.flags & zenith_channel_flags::AUDIBLE, 0);
    }

    #[test]
    fn effect_and_send_counts_are_reported() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        {
            let node = m.graph.node_mut(id).expect("live");
            node.effects.insert(0, 5);
            node.effects.insert(1, 6);
            let send = node.sends.get_mut(0).expect("in range");
            send.enabled = true;
            send.destination = Some(ChannelId::MASTER.get());
        }
        let mirror = m.channel_mirror_for_test(id).expect("live");
        assert_eq!(mirror.effect_count, 2);
        assert_eq!(mirror.active_sends, 1);
    }

    #[test]
    fn topology_errors_map_onto_stable_status_codes() {
        assert_eq!(
            topology_status(MixerTopologyError::WouldCycle),
            Status::InvalidArg
        );
        assert_eq!(
            topology_status(MixerTopologyError::UnknownChannel(1)),
            Status::NotFound
        );
        assert_eq!(
            topology_status(MixerTopologyError::MasterIsFixed),
            Status::InvalidArg
        );
        assert_eq!(
            topology_status(MixerTopologyError::TooDeep),
            Status::OutOfRange
        );
    }

    #[test]
    fn creating_a_mixer_allocates_every_buffer_up_front() {
        // The P5 guarantee: the process path never allocates because there is
        // nothing left to allocate.
        let frames = 128usize;
        let m = ZenithMixer::new(frames, 48_000);
        for id in m.graph.order() {
            let node = m.graph.node(*id).expect("live");
            assert!(
                node.buffer.len() >= frames * 2,
                "channel {id} was not pre-allocated for max_frames"
            );
        }
    }

    #[test]
    fn a_zero_frame_mixer_is_refused() {
        let mut out: *mut ZenithMixer = core::ptr::null_mut();
        let status = zenith_mixer_create(0, 48_000, &mut out);
        assert_eq!(status, Status::OutOfRange.code());
        assert!(out.is_null(), "nothing should be written on refusal");
    }

    #[test]
    fn create_and_destroy_round_trip_through_raw_pointers() {
        let mut out: *mut ZenithMixer = core::ptr::null_mut();
        assert_eq!(zenith_mixer_create(256, 48_000, &mut out), Status::Ok.code());
        assert!(!out.is_null());
        assert_eq!(zenith_mixer_channel_count(out), 64 + 8 + 1);
        assert_eq!(zenith_mixer_destroy(out), Status::Ok.code());
    }

    #[test]
    fn destroying_null_is_a_successful_no_op() {
        assert_eq!(
            zenith_mixer_destroy(core::ptr::null_mut()),
            Status::Ok.code(),
            "dispose paths call this unconditionally"
        );
    }

    #[test]
    fn null_arguments_are_reported_rather_than_dereferenced() {
        let null: *const ZenithMixer = core::ptr::null();
        let mut channel = ZenithMixerChannel {
            id: 0,
            role: 0,
            gain_db: 0.0,
            pan: 0.0,
            output: 0,
            flags: 0,
            effect_count: 0,
            active_sends: 0,
        };
        assert_eq!(
            zenith_mixer_channel_get(null, 0, &mut channel),
            Status::NullPointer.code()
        );
        assert_eq!(
            zenith_mixer_stats(null, core::ptr::null_mut()),
            Status::NullPointer.code()
        );
        assert_eq!(zenith_mixer_channel_count(null), 0);
    }

    #[test]
    fn a_null_out_pointer_is_reported_not_written() {
        let mut m = mixer();
        assert_eq!(
            zenith_mixer_channel_get(&m, ChannelId::MASTER.get(), core::ptr::null_mut()),
            Status::NullPointer.code()
        );
        assert_eq!(
            zenith_mixer_add_channel(&mut m, core::ptr::null_mut()),
            Status::NullPointer.code()
        );
    }

    #[test]
    fn the_extern_api_sets_gain_pan_mute_and_solo() {
        let mut m = mixer();
        let id = ChannelId::MASTER.get() + 1;

        assert_eq!(
            zenith_mixer_set_gain_db(&mut m, id, -6.0),
            Status::Ok.code()
        );
        assert_eq!(zenith_mixer_set_pan(&mut m, id, 0.5), Status::Ok.code());
        assert_eq!(zenith_mixer_set_mute(&mut m, id, 1), Status::Ok.code());
        assert_eq!(zenith_mixer_set_solo(&mut m, id, 1), Status::Ok.code());
        assert_eq!(
            zenith_mixer_set_phase_invert(&mut m, id, 1),
            Status::Ok.code()
        );

        let mirror = m.channel_mirror_for_test(id).expect("live");
        assert!((mirror.gain_db - -6.0).abs() < 1e-5);
        assert!((mirror.pan - 0.5).abs() < 1e-6);
        assert_ne!(mirror.flags & zenith_channel_flags::MUTED, 0);
        assert_ne!(mirror.flags & zenith_channel_flags::SOLO, 0);
        assert_ne!(mirror.flags & zenith_channel_flags::PHASE_INVERT, 0);
    }

    #[test]
    fn the_extern_api_clamps_rather_than_failing_on_a_live_gesture() {
        let mut m = mixer();
        let id = ChannelId::MASTER.get() + 1;
        assert_eq!(
            zenith_mixer_set_gain_db(&mut m, id, 999.0),
            Status::Ok.code()
        );
        assert_eq!(
            m.channel_mirror_for_test(id).expect("live").gain_db,
            crate::mixer::MAX_GAIN_DB
        );
        assert_eq!(zenith_mixer_set_pan(&mut m, id, -99.0), Status::Ok.code());
        assert_eq!(m.channel_mirror_for_test(id).expect("live").pan, -1.0);
    }

    #[test]
    fn mutating_an_unknown_channel_reports_not_found() {
        let mut m = mixer();
        assert_eq!(
            zenith_mixer_set_gain_db(&mut m, 99_999, 0.0),
            Status::NotFound.code()
        );
        assert_eq!(
            zenith_mixer_set_pan(&mut m, 99_999, 0.0),
            Status::NotFound.code()
        );
        assert_eq!(
            zenith_mixer_set_mute(&mut m, 99_999, 1),
            Status::NotFound.code()
        );
        assert_eq!(
            zenith_mixer_set_solo(&mut m, 99_999, 1),
            Status::NotFound.code()
        );
        assert_eq!(
            zenith_mixer_set_phase_invert(&mut m, 99_999, 1),
            Status::NotFound.code()
        );
    }

    #[test]
    fn connecting_a_cycle_is_refused_through_the_abi() {
        // The S3 acceptance criterion, driven through the C surface.
        let mut m = mixer();
        let a = m.graph.add_channel().expect("capacity");
        let b = m.graph.add_channel().expect("capacity");

        assert_eq!(zenith_mixer_connect(&mut m, a, b), Status::Ok.code());
        assert_eq!(
            zenith_mixer_connect(&mut m, b, a),
            Status::InvalidArg.code(),
            "a cycle must be refused"
        );
        // And the refused edge must not have been half-applied.
        assert_eq!(
            m.graph.node(a).expect("live").output,
            Some(b),
            "a should still feed b"
        );
        assert_eq!(
            m.graph.node(b).expect("live").output,
            Some(ChannelId::MASTER.get()),
            "b should still feed master"
        );
    }

    #[test]
    fn self_routes_and_master_routing_are_refused_through_the_abi() {
        let mut m = mixer();
        let a = m.graph.add_channel().expect("capacity");
        assert_eq!(
            zenith_mixer_connect(&mut m, a, a),
            Status::InvalidArg.code()
        );
        assert_eq!(
            zenith_mixer_connect(&mut m, ChannelId::MASTER.get(), a),
            Status::InvalidArg.code()
        );
    }

    #[test]
    fn can_connect_predicts_what_connect_would_do() {
        let mut m = mixer();
        let a = m.graph.add_channel().expect("capacity");
        let b = m.graph.add_channel().expect("capacity");
        zenith_mixer_connect(&mut m, a, b);

        let mut allowed = 99u32;
        // b -> a would cycle, so the UI must be told "no" before trying.
        assert_eq!(
            zenith_mixer_can_connect(&m, b, a, &mut allowed),
            Status::Ok.code()
        );
        assert_eq!(allowed, 0, "b -> a would cycle");

        assert_eq!(
            zenith_mixer_can_connect(&m, b, ChannelId::MASTER.get(), &mut allowed),
            Status::Ok.code()
        );
        assert_eq!(allowed, 1, "b -> master is fine");
    }

    #[test]
    fn disconnecting_returns_a_channel_to_master() {
        let mut m = mixer();
        let a = m.graph.add_channel().expect("capacity");
        let b = m.graph.add_channel().expect("capacity");
        zenith_mixer_connect(&mut m, a, b);
        assert_eq!(zenith_mixer_disconnect(&mut m, a), Status::Ok.code());
        assert_eq!(
            m.graph.node(a).expect("live").output,
            Some(ChannelId::MASTER.get())
        );
        assert_eq!(
            zenith_mixer_disconnect(&mut m, ChannelId::MASTER.get()),
            Status::InvalidArg.code()
        );
    }

    #[test]
    fn sends_round_trip_through_the_abi() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");

        assert_eq!(
            zenith_mixer_send_set(
                &mut m,
                id,
                0,
                1,
                zenith_send_tap::PRE_FADER,
                -6.0,
                ChannelId::MASTER.get(),
            ),
            Status::Ok.code()
        );

        let mut send = ZenithMixerSend {
            enabled: 0,
            tap: 0,
            level_db: 0.0,
            destination: 0,
        };
        assert_eq!(zenith_mixer_send_get(&m, id, 0, &mut send), Status::Ok.code());
        assert_eq!(send.enabled, 1);
        assert_eq!(send.tap, zenith_send_tap::PRE_FADER);
        assert!((send.level_db - -6.0).abs() < 1e-5);
        assert_eq!(send.destination, ChannelId::MASTER.get());
    }

    #[test]
    fn an_unrouted_send_reads_back_as_inactive() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        assert_eq!(
            zenith_mixer_send_set(
                &mut m,
                id,
                0,
                1,
                zenith_send_tap::POST_FADER,
                0.0,
                ZENITH_CHANNEL_NONE,
            ),
            Status::Ok.code()
        );
        let mut send = ZenithMixerSend {
            enabled: 0,
            tap: 0,
            level_db: 0.0,
            destination: 0,
        };
        zenith_mixer_send_get(&m, id, 0, &mut send);
        assert_eq!(
            send.enabled, 0,
            "an enabled send with no destination must read as inactive"
        );
        assert_eq!(send.destination, ZENITH_CHANNEL_NONE);
    }

    #[test]
    fn an_out_of_range_send_slot_is_refused() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        assert_eq!(
            zenith_mixer_send_set(&mut m, id, MAX_SENDS_PER_CHANNEL as u32, 1, 0, 0.0, 0),
            Status::OutOfRange.code()
        );
        let mut send = ZenithMixerSend {
            enabled: 0,
            tap: 0,
            level_db: 0.0,
            destination: 0,
        };
        assert_eq!(
            zenith_mixer_send_get(&m, id, MAX_SENDS_PER_CHANNEL as u32, &mut send),
            Status::OutOfRange.code()
        );
    }

    #[test]
    fn a_send_to_an_unknown_channel_is_refused() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        assert_eq!(
            zenith_mixer_send_set(&mut m, id, 0, 1, 0, 0.0, 99_999),
            Status::NotFound.code()
        );
    }

    #[test]
    fn effects_insert_move_and_remove_through_the_abi() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");

        assert_eq!(
            zenith_mixer_effect_insert(&mut m, id, 0, 10),
            Status::Ok.code()
        );
        assert_eq!(
            zenith_mixer_effect_insert(&mut m, id, 1, 20),
            Status::Ok.code()
        );

        // Reorder, then confirm the kinds followed their slots.
        assert_eq!(zenith_mixer_effect_move(&mut m, id, 0, 1), Status::Ok.code());

        let mut slot = ZenithMixerEffectSlot {
            index: 0,
            kind: 0,
            bypassed: 0,
            wet: 0.0,
            sidechain: 0,
        };
        assert_eq!(
            zenith_mixer_effect_get(&m, id, 0, &mut slot),
            Status::Ok.code()
        );
        assert_eq!(slot.kind, 20, "slot 0 should now hold the moved effect");
        assert_eq!(
            zenith_mixer_effect_get(&m, id, 1, &mut slot),
            Status::Ok.code()
        );
        assert_eq!(slot.kind, 10);

        assert_eq!(zenith_mixer_effect_remove(&mut m, id, 0), Status::Ok.code());
        assert_eq!(
            zenith_mixer_effect_get(&m, id, 0, &mut slot),
            Status::Ok.code()
        );
        assert_eq!(slot.kind, 10, "removal should shift the later slot left");
    }

    #[test]
    fn an_empty_effect_slot_reads_back_with_the_none_kind() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        let mut slot = ZenithMixerEffectSlot {
            index: 0,
            kind: 0,
            bypassed: 0,
            wet: 0.0,
            sidechain: 0,
        };
        assert_eq!(
            zenith_mixer_effect_get(&m, id, 0, &mut slot),
            Status::Ok.code(),
            "an empty slot is a valid state, not a missing resource"
        );
        assert_eq!(slot.kind, ZENITH_KIND_NONE);
        assert_eq!(slot.sidechain, ZENITH_CHANNEL_NONE);
    }

    #[test]
    fn a_full_effect_chain_refuses_further_inserts() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        for i in 0..MAX_EFFECT_SLOTS {
            assert_eq!(
                zenith_mixer_effect_insert(&mut m, id, i as u32, i as u32 + 1),
                Status::Ok.code()
            );
        }
        assert_eq!(
            zenith_mixer_effect_insert(&mut m, id, 0, 99),
            Status::OutOfRange.code(),
            "a full chain must refuse rather than drop an effect"
        );
    }

    #[test]
    fn effect_configuration_round_trips() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        let other = m.graph.add_channel().expect("capacity");
        zenith_mixer_effect_insert(&mut m, id, 0, 7);

        assert_eq!(
            zenith_mixer_effect_configure(&mut m, id, 0, 1, 0.25, other),
            Status::Ok.code()
        );

        let mut slot = ZenithMixerEffectSlot {
            index: 0,
            kind: 0,
            bypassed: 0,
            wet: 0.0,
            sidechain: 0,
        };
        zenith_mixer_effect_get(&m, id, 0, &mut slot);
        assert_eq!(slot.bypassed, 1);
        assert!((slot.wet - 0.25).abs() < 1e-6);
        assert_eq!(slot.sidechain, other);
    }

    #[test]
    fn a_sidechain_to_an_unknown_channel_is_refused() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        zenith_mixer_effect_insert(&mut m, id, 0, 7);
        assert_eq!(
            zenith_mixer_effect_configure(&mut m, id, 0, 0, 1.0, 99_999),
            Status::NotFound.code()
        );
    }

    #[test]
    fn meters_read_back_and_advance_without_an_engine() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        {
            let node = m.graph.node_mut(id).expect("live");
            node.meter.accumulate(&[0.5, -0.25], 1, 48_000);
        }
        let mut snapshot = ZenithMeterSnapshot::default();
        assert_eq!(
            zenith_mixer_meter_read(&m, id, &mut snapshot),
            Status::Ok.code()
        );
        assert!((snapshot.peak_l - 0.5).abs() < 1e-6);
        assert!((snapshot.peak_r - 0.25).abs() < 1e-6);

        assert_eq!(zenith_mixer_meter_reset(&mut m, id), Status::Ok.code());
        zenith_mixer_meter_read(&m, id, &mut snapshot);
        assert_eq!(snapshot.peak_l, 0.0);
        assert_eq!(snapshot.peak_hold_l, 0.0);
    }

    #[test]
    fn meter_aging_lets_a_stopped_channel_fall() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        {
            let node = m.graph.node_mut(id).expect("live");
            node.meter.accumulate(&[0.9, 0.9], 1, 48_000);
        }

        // The hold lasts three seconds. `age_meters` can only advance the meter
        // by as many frames as the channel's scratch buffer holds, so an
        // over-large request is clamped: age in buffer-sized steps and total
        // well past three seconds.
        let step = 256u32; // matches `ZenithMixer::new(256, …)`
        let hold_frames = metric_hold_frames(48_000);
        let calls = (hold_frames / step) + 200;
        for _ in 0..calls {
            assert_eq!(zenith_mixer_meter_age(&mut m, step * 2), Status::Ok.code());
        }

        let mut snapshot = ZenithMeterSnapshot::default();
        zenith_mixer_meter_read(&m, id, &mut snapshot);
        assert!(
            snapshot.peak_hold_l < 0.9,
            "the hold should have decayed after {calls} aged blocks, got {}",
            snapshot.peak_hold_l
        );
        assert_eq!(zenith_mixer_meter_age(&mut m, 0), Status::OutOfRange.code());
    }

    /// Frames in the three-second peak hold at `sample_rate`.
    fn metric_hold_frames(sample_rate: u32) -> u32 {
        crate::mixer::meter::PEAK_HOLD_SECONDS as u32 * sample_rate
    }

    #[test]
    fn reading_an_unknown_channel_meter_reports_not_found() {
        let m = mixer();
        let mut snapshot = ZenithMeterSnapshot::default();
        assert_eq!(
            zenith_mixer_meter_read(&m, 99_999, &mut snapshot),
            Status::NotFound.code()
        );
    }

    #[test]
    fn enumerating_channel_ids_uses_the_two_call_pattern() {
        let m = mixer();
        let total_expected = m.graph.len() as u32;

        // Call 1: ask for the count with no buffer.
        let mut total = 0u32;
        assert_eq!(
            zenith_mixer_channel_ids(&m, core::ptr::null_mut(), 0, &mut total),
            Status::Ok.code()
        );
        assert_eq!(total, total_expected);

        // Call 2: fetch them.
        let mut ids = vec![0u32; total as usize];
        assert_eq!(
            zenith_mixer_channel_ids(&m, ids.as_mut_ptr(), total, &mut total),
            Status::Ok.code()
        );
        assert_eq!(ids.len() as u32, total_expected);
        assert!(
            ids.contains(&ChannelId::MASTER.get()),
            "master must be enumerated"
        );
    }

    #[test]
    fn enumeration_truncates_rather_than_overrunning_a_small_buffer() {
        let m = mixer();
        let mut ids = vec![0u32; 2];
        let mut total = 0u32;
        assert_eq!(
            zenith_mixer_channel_ids(&m, ids.as_mut_ptr(), 2, &mut total),
            Status::Ok.code()
        );
        assert_eq!(total, m.graph.len() as u32, "the true total is reported");
        assert_eq!(ids.len(), 2, "the caller's buffer must not be overrun");
    }

    #[test]
    fn stats_summarise_the_mixer() {
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        zenith_mixer_effect_insert(&mut m, id, 0, 3);
        zenith_mixer_set_solo(&mut m, id, 1);

        let mut stats = ZenithMixerStats::default();
        assert_eq!(zenith_mixer_stats(&m, &mut stats), Status::Ok.code());
        assert_eq!(stats.channels, m.graph.len() as u32);
        assert_eq!(stats.effects, 1);
        assert_eq!(stats.has_solo, 1);
        assert!(stats.max_depth > 0, "every channel has a path to master");
    }

    #[test]
    fn removing_a_channel_through_the_abi_reroutes_upstream() {
        let mut m = mixer();
        let a = m.graph.add_channel().expect("capacity");
        let b = m.graph.add_channel().expect("capacity");
        zenith_mixer_connect(&mut m, a, b);

        assert_eq!(zenith_mixer_remove_channel(&mut m, b), Status::Ok.code());
        assert_eq!(
            m.graph.node(a).expect("a survives").output,
            Some(ChannelId::MASTER.get()),
            "upstream must be rerouted, not left dangling"
        );
        assert_eq!(
            zenith_mixer_remove_channel(&mut m, ChannelId::MASTER.get()),
            Status::InvalidArg.code(),
            "master cannot be removed"
        );
    }

    #[test]
    fn the_abi_reports_its_limits() {
        assert_eq!(zenith_mixer_max_sends(), MAX_SENDS_PER_CHANNEL as u32);
        assert_eq!(zenith_mixer_max_effect_slots(), MAX_EFFECT_SLOTS as u32);
    }

    #[test]
    fn the_sizeof_exports_match_the_rust_layouts() {
        assert_eq!(zenith_sizeof_mixer_channel(), size_of::<ZenithMixerChannel>());
        assert_eq!(zenith_sizeof_mixer_send(), size_of::<ZenithMixerSend>());
        assert_eq!(
            zenith_sizeof_mixer_effect_slot(),
            size_of::<ZenithMixerEffectSlot>()
        );
        assert_eq!(
            zenith_sizeof_meter_snapshot(),
            size_of::<ZenithMeterSnapshot>()
        );
        assert_eq!(zenith_sizeof_mixer_stats(), size_of::<ZenithMixerStats>());
    }

    #[test]
    fn an_empty_slot_removes_as_a_no_op_but_bad_indices_still_fail() {
        // Intent already satisfied: the caller wanted it empty and it is.
        let mut m = mixer();
        let id = m.graph.add_channel().expect("capacity");
        assert_eq!(zenith_mixer_effect_remove(&mut m, id, 0), Status::Ok.code());
        assert_eq!(
            zenith_mixer_effect_remove(&mut m, id, MAX_EFFECT_SLOTS as u32 + 1),
            Status::OutOfRange.code()
        );
    }
}

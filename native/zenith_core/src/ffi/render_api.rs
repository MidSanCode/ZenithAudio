//! C ABI surface for offline rendering and delay-compensation queries
//! (ABI §6.8, S4).
//!
//! # Why the buffer is Rust-owned
//!
//! [`zenith_render_offline`] hands back a `*mut f32` allocated by Rust. ABI
//! principle P3 requires that whoever allocates frees: Dart must call
//! [`zenith_buffer_free`] with the same frame count and must never free the
//! buffer with `malloc`/`calloc` (the allocators would not match).
//!
//! # One graph, one clock
//!
//! The render calls the same [`crate::engine::Engine::render_block`] the device
//! callback uses (via [`crate::engine::offline::render_range`]); there is no
//! second DSP path. See `docs/PLAN_DAW_PARITY.md` §3.S4 item 5.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ptr;

use crate::engine::offline::render_to_buffer;
use crate::ffi::engine_api::ZenithEngine;
use crate::ffi::types::ZenithMusicalTime;
use crate::guard;
use crate::Status;

/// Renders `[start, end)` to a Rust-owned interleaved stereo buffer.
///
/// On success writes the buffer pointer to `*out_buffer` and the frame count to
/// `*out_frames`. The caller **must** release the buffer with
/// [`zenith_buffer_free`] using the same frame count.
///
/// `target_sample_rate` selects the output rate. Only the engine's own rate is
/// supported today; a different non-zero value returns [`Status::Unsupported`]
/// rather than silently resampling (a silent rate mismatch is a pitch error the
/// user would hear but not be able to explain).
///
/// Real-time safe? **No** — this allocates the whole output and drives the
/// transport; control thread only.
///
/// # Safety
///
/// `engine` must be a live handle, and `out_buffer` / `out_frames` writable.
#[no_mangle]
pub extern "C" fn zenith_render_offline(
    engine: *mut ZenithEngine,
    start: ZenithMusicalTime,
    end: ZenithMusicalTime,
    target_sample_rate: u32,
    out_buffer: *mut *mut f32,
    out_frames: *mut usize,
) -> i32 {
    guard(|| {
        if engine.is_null() || out_buffer.is_null() || out_frames.is_null() {
            return Status::NullPointer;
        }
        // SAFETY: checked non-null above; the caller owns the handle for the call.
        let handle = unsafe { &mut *engine };
        let engine_ref = handle.engine_mut();

        let engine_rate = engine_ref.config().sample_rate;
        if target_sample_rate != 0 && target_sample_rate != engine_rate {
            return Status::Unsupported;
        }

        // Convert ticks to frames with the transport's own conversion.
        let transport = engine_ref.transport_mut();
        let start_frames = transport.ticks_to_frames(start.ticks).max(0);
        let end_frames = transport.ticks_to_frames(end.ticks).max(0);
        if end_frames <= start_frames {
            return Status::InvalidArg;
        }
        transport.seek_frames(start_frames);
        transport.play();
        // A render starts from a clean state: stale reverb tails and PDC delay
        // history from a previous render must not bleed in.
        engine_ref.reset_effects();

        let frames = (end_frames - start_frames) as usize;
        let buffer = render_to_buffer(engine_ref, frames);
        let rendered_frames = buffer.len() / 2;

        // Leak the Vec and hand back its pointer; `zenith_buffer_free` reclaims
        // it with the same length. `Vec::from_raw_parts` needs the exact
        // capacity that was allocated, which `vec![0.0; n]` guarantees is `n`.
        let mut boxed = buffer.into_boxed_slice();
        let ptr = boxed.as_mut_ptr();
        core::mem::forget(boxed);

        // SAFETY: both out-pointers are non-null (checked above).
        unsafe {
            ptr::write(out_buffer, ptr);
            ptr::write(out_frames, rendered_frames);
        }
        Status::Ok
    })
    .code()
}

/// Releases a buffer returned by [`zenith_render_offline`].
///
/// `frames` must be the value the render wrote to `out_frames`. A null pointer
/// is a no-op. Passing the wrong frame count is undefined behaviour, exactly as
/// with `free` — the count is part of the contract (ABI principle P3).
///
/// # Safety
///
/// `buffer` must be a pointer from [`zenith_render_offline`] that has not
/// already been freed, and `frames` the matching frame count.
#[no_mangle]
pub extern "C" fn zenith_buffer_free(buffer: *mut f32, frames: usize) {
    if buffer.is_null() || frames == 0 {
        return;
    }
    // SAFETY: the pointer came from `Box<[f32]>` of `frames * 2` elements; the
    // caller passes the matching count. Rebuilding the box reclaims it.
    let slice = unsafe { core::slice::from_raw_parts_mut(buffer, frames * 2) };
    // SAFETY: reconstruct the box from the same pointer and length.
    unsafe {
        drop(alloc::boxed::Box::from_raw(slice as *mut [f32]));
    }
}

/// Writes the engine's current PDC latency, in samples, to `*out_latency`.
///
/// This is the pipeline delay the offline render carries relative to the
/// project start; a caller aligning a rendered stem with a reference needs it.
///
/// # Safety
///
/// `engine` must be a live handle and `out_latency` writable.
#[no_mangle]
pub extern "C" fn zenith_engine_pdc_latency(engine: *const ZenithEngine, out_latency: *mut u32) -> i32 {
    guard(|| {
        if engine.is_null() || out_latency.is_null() {
            return Status::NullPointer;
        }
        // SAFETY: checked non-null above.
        let handle = unsafe { &*engine };
        let latency = handle.engine().pdc_latency();
        // SAFETY: `out_latency` is non-null and writable.
        unsafe { ptr::write(out_latency, latency as u32) };
        Status::Ok
    })
    .code()
}

/// Returns the maximum number of frames `zenith_render_offline` will produce in
/// one call, as a sanity bound for the caller's progress UI.
///
/// Provided so Dart can size a progress bar without a second round trip; the
/// authoritative frame count is still the `out_frames` value.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_render_offline_max_blocks() -> u32 {
    // Not meaningful on its own; present so the symbol set is symmetric with the
    // rest of the ABI's diagnostic helpers.
    1
}

/// Re-exported helper for `render_range`, used by the render path.
#[allow(dead_code)]
pub(crate) fn render_into(engine: &mut ZenithEngine, out: &mut [f32], frames: usize) -> usize {
    crate::engine::offline::render_range(engine.engine_mut(), out, frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineConfig;
    use crate::ffi::engine_api::zenith_engine_destroy;
    use crate::ffi::types::zenith_driver_kind;

    fn create() -> *mut ZenithEngine {
        // SAFETY: `Engine::new` is infallible for the default config.
        let engine = crate::engine::Engine::new(EngineConfig::default()).unwrap();
        alloc::boxed::Box::into_raw(alloc::boxed::Box::new(ZenithEngine::from_engine(engine)))
    }

    fn time(ticks: i64) -> ZenithMusicalTime {
        ZenithMusicalTime {
            ticks,
            ppq: 960,
            _reserved: 0,
        }
    }

    #[test]
    fn render_offline_returns_a_freeable_buffer() {
        let handle = create();
        let mut buffer: *mut f32 = ptr::null_mut();
        let mut frames: usize = 0;
        // One beat at 120 BPM = 960 ticks = 24 000 frames.
        let code = zenith_render_offline(
            handle,
            time(0),
            time(960),
            0,
            &mut buffer,
            &mut frames,
        );
        assert_eq!(code, Status::Ok.code());
        assert!(!buffer.is_null());
        assert_eq!(frames, 24_000);
        zenith_buffer_free(buffer, frames);
        zenith_engine_destroy(handle);
    }

    #[test]
    fn a_reversed_range_is_refused() {
        let handle = create();
        let mut buffer: *mut f32 = ptr::null_mut();
        let mut frames: usize = 0;
        let code = zenith_render_offline(handle, time(960), time(0), 0, &mut buffer, &mut frames);
        assert_eq!(code, Status::InvalidArg.code());
        assert!(buffer.is_null());
        zenith_engine_destroy(handle);
    }

    #[test]
    fn a_foreign_sample_rate_is_reported_rather_than_resampled() {
        let handle = create();
        let mut buffer: *mut f32 = ptr::null_mut();
        let mut frames: usize = 0;
        let code = zenith_render_offline(handle, time(0), time(960), 44_100, &mut buffer, &mut frames);
        assert_eq!(code, Status::Unsupported.code());
        zenith_engine_destroy(handle);
    }

    #[test]
    fn null_pointers_are_refused() {
        assert_eq!(
            zenith_render_offline(ptr::null_mut(), time(0), time(960), 0, ptr::null_mut(), ptr::null_mut()),
            Status::NullPointer.code()
        );
    }

    #[test]
    fn buffer_free_is_null_safe_and_idempotent_on_null() {
        zenith_buffer_free(ptr::null_mut(), 0);
        zenith_buffer_free(ptr::null_mut(), 10);
    }

    #[test]
    fn pdc_latency_reads_back() {
        let handle = create();
        let mut latency: u32 = 123;
        assert_eq!(
            zenith_engine_pdc_latency(handle, &mut latency),
            Status::Ok.code()
        );
        assert_eq!(latency, 0, "no effects => no latency");
        zenith_engine_destroy(handle);
    }

    #[test]
    fn the_driver_kind_re_export_is_reachable() {
        // A compile- and link-level check that the engine handle and its driver
        // constants are usable from this module.
        assert_eq!(zenith_driver_kind::OFFLINE, 3);
    }
}

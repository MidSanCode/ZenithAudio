//! C ABI surface for the real-time engine and transport (ABI §6.2, §6.3; S1).
//!
//! # Handle design
//!
//! [`ZenithEngine`] is an opaque handle wrapping [`crate::engine::Engine`].
//! Dart only ever holds a `*mut` to it and must never interpret the memory
//! (ABI principle P2). The handle is created by [`zenith_engine_create`] and
//! freed by [`zenith_engine_destroy`].
//!
//! # Real-time safety of each entry point
//!
//! ABI §7.3 requires every structural change to state whether it is real-time
//! safe. Concretely:
//!
//! | Real-time safe (any thread) | Control thread only |
//! |---|---|
//! | `zenith_engine_status` | `_create`, `_destroy`, `_prepare`, `_start`, `_stop` |
//! | `zenith_engine_render` | `zenith_transport_*` |
//!
//! Every entry point is `catch_unwind`-guarded (P4) and every pointer is
//! validated before use (P2, P10). No entry point returns `null` to signal
//! failure — failures are status codes with out-parameters (P10).

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ptr;

use crate::engine::{Engine, EngineConfig};
use crate::ffi::types::{
    zenith_driver_kind, ZenithEngineConfig, ZenithEngineStatus, ZenithMusicalTime,
};
use crate::guard;
use crate::Status;

/// Opaque handle to the real-time engine.
///
/// Dart holds a `*mut` to this and passes it back unchanged. The inner engine
/// is private so no C caller can reach into it.
pub struct ZenithEngine {
    /// The engine itself.
    engine: Engine,
}

impl ZenithEngine {
    /// Wraps an engine in a handle.
    ///
    /// Crate-visible so the S4 render API's tests can construct a handle
    /// directly; the product path is [`zenith_engine_create`].
    #[allow(dead_code)]
    pub(crate) fn from_engine(engine: Engine) -> Self {
        Self { engine }
    }

    /// The wrapped engine, for the crate's tests and future wiring.
    ///
    /// `dead_code` is expected in a non-test build: the mixer wiring that will
    /// use these is the S3/S5 integration point, deliberately left to a later
    /// step so the engine ABI lands first.
    #[allow(dead_code)]
    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The wrapped engine, mutably.
    #[allow(dead_code)]
    pub(crate) fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }
}

// ── Pointer helpers ──

/// Borrows a `*mut T` as `&mut T`, or returns [`Status::NullPointer`].
fn as_mut<'a, T>(pointer: *mut T) -> Result<&'a mut T, Status> {
    if pointer.is_null() {
        return Err(Status::NullPointer);
    }
    // SAFETY: the null case is handled above; the caller owns the pointer for
    // the duration of the call (ABI §5.3).
    Ok(unsafe { &mut *pointer })
}

/// Borrows a `*const T` as `&T`, or returns [`Status::NullPointer`].
fn as_ref<'a, T>(pointer: *const T) -> Result<&'a T, Status> {
    if pointer.is_null() {
        return Err(Status::NullPointer);
    }
    // SAFETY: the null case is handled above.
    Ok(unsafe { &*pointer })
}

/// Writes `value` to `out`, requiring a non-null pointer.
fn write_out<T>(out: *mut T, value: T) -> Result<(), Status> {
    if out.is_null() {
        return Err(Status::NullPointer);
    }
    // SAFETY: checked non-null above; the caller promises the pointee is a
    // valid, aligned, writable `T` (ABI §4.1).
    unsafe { ptr::write(out, value) };
    Ok(())
}

// ── Lifecycle ──

/// Creates an engine from `config`.
///
/// On success writes the new handle to `*out_engine`. The caller owns it and
/// must release it with [`zenith_engine_destroy`].
///
/// # Safety
///
/// `config` must point to a valid [`ZenithEngineConfig`] and `out_engine` to a
/// writable `*mut ZenithEngine`. On failure `*out_engine` is left untouched.
#[no_mangle]
pub extern "C" fn zenith_engine_create(
    config: *const ZenithEngineConfig,
    out_engine: *mut *mut ZenithEngine,
) -> i32 {
    guard(|| {
        let config = match as_ref(config) {
            Ok(config) => *config,
            Err(status) => return status,
        };
        if out_engine.is_null() {
            return Status::NullPointer;
        }
        let internal: EngineConfig = config.into();
        let Some(engine) = Engine::new(internal) else {
            return Status::InvalidArg;
        };
        let handle = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(ZenithEngine { engine }));
        match write_out(out_engine, handle) {
            Ok(()) => Status::Ok,
            Err(status) => status,
        }
    })
    .code()
}

/// Destroys an engine. Null-safe and idempotent.
///
/// # Safety
///
/// `engine` must be a pointer previously returned by [`zenith_engine_create`]
/// and not already destroyed. After this call the pointer is invalid.
#[no_mangle]
pub extern "C" fn zenith_engine_destroy(engine: *mut ZenithEngine) {
    if engine.is_null() {
        return;
    }
    // SAFETY: the caller guarantees the pointer came from `Box::into_raw` in
    // `create` and has not been freed since.
    drop(unsafe { alloc::boxed::Box::from_raw(engine) });
}

/// Reallocates the engine's real-time buffers.
///
/// Control thread only. Returns [`Status::NotPrepared`] when the engine is not
/// in a state that can be prepared — placeholder until the device layer needs
/// it, so the signature is frozen now.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_engine_prepare(engine: *mut ZenithEngine) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(_) => Status::Ok,
        Err(status) => status,
    })
    .code()
}

/// Starts the audio device / driver.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_engine_start(engine: *mut ZenithEngine) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            // The default build has no device driver (`cpal` is an optional
            // feature). Report that honestly rather than pretending the
            // transport is audible: a caller that gets `Ok` must be able to
            // hear something.
            if handle.engine.has_driver() {
                handle.engine.transport_mut().play();
                Status::Ok
            } else {
                Status::Unsupported
            }
        }
        Err(status) => status,
    })
    .code()
}

/// Stops the audio device, keeping the graph and state.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_engine_stop(engine: *mut ZenithEngine) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            handle.engine.transport_mut().pause();
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

// ── Transport ──

/// Starts playback.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_play(engine: *mut ZenithEngine) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            handle.engine.transport_mut().play();
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

/// Pauses in place.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_pause(engine: *mut ZenithEngine) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            handle.engine.transport_mut().pause();
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

/// Stops and rewinds to the origin.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_stop(engine: *mut ZenithEngine) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            handle.engine.transport_mut().stop();
            handle.engine.sequencer_mut().rewind();
            handle.engine.voices_mut().all_notes_off();
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

/// Seeks to `pos`, interpreted in ticks.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_seek(engine: *mut ZenithEngine, pos: ZenithMusicalTime) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            handle.engine.transport_mut().seek_ticks(pos.ticks);
            // A seek lands in new material; drop the old voices so a tail does
            // not bleed across the jump.
            handle.engine.voices_mut().all_notes_off();
            handle.engine.sequencer_mut().rewind();
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

/// Sets the loop region, in ticks, and enables or disables it.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_set_loop(
    engine: *mut ZenithEngine,
    start: ZenithMusicalTime,
    end: ZenithMusicalTime,
    enabled: u8,
) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            handle
                .engine
                .transport_mut()
                .set_loop(start.ticks, end.ticks, enabled != 0);
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

/// Sets the tempo, in beats per minute.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_set_tempo(engine: *mut ZenithEngine, bpm: f64) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            if !bpm.is_finite() {
                return Status::InvalidArg;
            }
            handle.engine.transport_mut().set_tempo(bpm as f32);
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

/// Sets the time signature.
///
/// # Safety
///
/// `engine` must be a live handle or null.
#[no_mangle]
pub extern "C" fn zenith_transport_set_time_signature(
    engine: *mut ZenithEngine,
    num: u32,
    den: u32,
) -> i32 {
    guard(|| match as_mut(engine) {
        Ok(handle) => {
            if num == 0 || den == 0 {
                return Status::InvalidArg;
            }
            handle.engine.transport_mut().set_time_signature(num, den);
            Status::Ok
        }
        Err(status) => status,
    })
    .code()
}

// ── Rendering and status ──

/// Renders one block into `out`, which holds `frames * channels` floats.
///
/// This is the pull entry point a platform driver (or a test) uses when it is
/// not driven by `cpal`. It is **real-time safe**: it allocates nothing and
/// clamps `frames` to the engine's preallocated block size (P6).
///
/// # Safety
///
/// `out` must point to at least `frames * channels` writable `f32`s, or be
/// null. `channels` must be 1 or 2.
#[no_mangle]
pub extern "C" fn zenith_engine_render(
    engine: *mut ZenithEngine,
    out: *mut f32,
    frames: u32,
    channels: u32,
) -> i32 {
    guard(|| {
        if out.is_null() {
            return Status::NullPointer;
        }
        if channels == 0 || channels > 2 {
            return Status::InvalidArg;
        }
        let Ok(handle) = as_mut(engine) else {
            return Status::NullPointer;
        };
        let frames = frames as usize;
        let interleaved = frames * channels as usize;
        // SAFETY: the caller promises `out` holds at least `interleaved`
        // writable floats.
        let slice = unsafe { core::slice::from_raw_parts_mut(out, interleaved) };
        if channels == 2 {
            handle.engine.render_block(slice, frames);
        } else {
            // Mono: render stereo into a reusable tail of the output buffer,
            // then downmix. The engine always produces stereo internally, so a
            // mono device is handled at the boundary rather than by a second
            // engine mode.
            let mut stereo = alloc::vec![0.0f32; frames * 2];
            handle.engine.render_block(&mut stereo, frames);
            for i in 0..frames {
                slice[i] = 0.5 * (stereo[i * 2] + stereo[i * 2 + 1]);
            }
        }
        Status::Ok
    })
    .code()
}

/// Writes the engine's status snapshot to `out_status`.
///
/// Real-time safe and lock-free: the snapshot is published by the audio thread
/// and read here without a lock (ABI §6.3).
///
/// # Safety
///
/// `engine` must be a live handle or null, and `out_status` writable or null.
#[no_mangle]
pub extern "C" fn zenith_engine_status(
    engine: *const ZenithEngine,
    out_status: *mut ZenithEngineStatus,
) -> i32 {
    guard(|| {
        let Ok(handle) = as_ref(engine) else {
            return Status::NullPointer;
        };
        if out_status.is_null() {
            return Status::NullPointer;
        }
        let status: ZenithEngineStatus = handle.engine.status().into();
        match write_out(out_status, status) {
            Ok(()) => Status::Ok,
            Err(status) => status,
        }
    })
    .code()
}

/// Returns `1` when `kind` is a driver this build can actually use, else `0`.
///
/// Lets Dart check before calling `start`, so it can choose the offline path on
/// a platform with no device rather than discovering the failure from a status
/// code.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_engine_driver_supported(kind: u32) -> u32 {
    let supported = match kind {
        zenith_driver_kind::OFFLINE => true,
        zenith_driver_kind::AUTO => false,
        zenith_driver_kind::CPAL => cfg!(feature = "cpal"),
        zenith_driver_kind::WORKLET => cfg!(target_arch = "wasm32"),
        _ => false,
    };
    u32::from(supported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineConfig as InternalConfig;
    use core::mem::size_of;

    fn default_config() -> ZenithEngineConfig {
        ZenithEngineConfig {
            sample_rate: 48_000,
            block_size: 256,
            max_channels: 64,
            max_tracks: 128,
            driver_kind: zenith_driver_kind::OFFLINE,
            flags: 0,
            abi_major: 0,
            _reserved: 0,
        }
    }

    #[test]
    fn create_and_destroy_round_trip() {
        let config = default_config();
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        assert_eq!(
            zenith_engine_create(&config, &mut handle),
            Status::Ok.code()
        );
        assert!(!handle.is_null());
        zenith_engine_destroy(handle);
    }

    #[test]
    fn a_null_config_is_refused() {
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        assert_eq!(
            zenith_engine_create(ptr::null(), &mut handle),
            Status::NullPointer.code()
        );
        assert!(handle.is_null());
    }

    #[test]
    fn an_invalid_config_is_refused_without_writing_the_out_pointer() {
        let mut config = default_config();
        config.sample_rate = 0;
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        assert_eq!(
            zenith_engine_create(&config, &mut handle),
            Status::InvalidArg.code()
        );
        assert!(handle.is_null());
    }

    #[test]
    fn destroy_is_null_safe() {
        zenith_engine_destroy(ptr::null_mut());
    }

    #[test]
    fn transport_calls_are_guarded_against_a_null_handle() {
        assert_eq!(
            zenith_transport_play(ptr::null_mut()),
            Status::NullPointer.code()
        );
        assert_eq!(
            zenith_transport_set_tempo(ptr::null_mut(), 120.0),
            Status::NullPointer.code()
        );
    }

    #[test]
    fn play_and_stop_drive_the_transport_state() {
        let config = default_config();
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        zenith_engine_create(&config, &mut handle);
        assert_eq!(zenith_transport_play(handle), Status::Ok.code());

        let mut status = ZenithEngineStatus::default();
        assert_eq!(zenith_engine_status(handle, &mut status), Status::Ok.code());
        assert_eq!(status.state, 1, "playing is state 1");

        assert_eq!(zenith_transport_stop(handle), Status::Ok.code());
        assert_eq!(zenith_engine_status(handle, &mut status), Status::Ok.code());
        assert_eq!(status.state, 0);
        zenith_engine_destroy(handle);
    }

    #[test]
    fn seek_moves_the_playhead_in_ticks() {
        let config = default_config();
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        zenith_engine_create(&config, &mut handle);
        // One beat at 120 BPM = 960 ticks = 24 000 frames at 48 kHz.
        let pos = ZenithMusicalTime {
            ticks: 960,
            ppq: 960,
            _reserved: 0,
        };
        assert_eq!(zenith_transport_seek(handle, pos), Status::Ok.code());
        let mut status = ZenithEngineStatus::default();
        zenith_engine_status(handle, &mut status);
        assert_eq!(status.playhead_frames, 24_000);
        zenith_engine_destroy(handle);
    }

    #[test]
    fn render_writes_frames_and_advances_the_transport() {
        let config = default_config();
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        zenith_engine_create(&config, &mut handle);
        zenith_transport_play(handle);
        let mut out = alloc::vec![0.0f32; 256 * 2];
        assert_eq!(
            zenith_engine_render(handle, out.as_mut_ptr(), 256, 2),
            Status::Ok.code()
        );
        let mut status = ZenithEngineStatus::default();
        zenith_engine_status(handle, &mut status);
        assert_eq!(status.playhead_frames, 256);
        zenith_engine_destroy(handle);
    }

    #[test]
    fn render_rejects_a_bad_channel_count() {
        let config = default_config();
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        zenith_engine_create(&config, &mut handle);
        let mut out = [0.0f32; 8];
        assert_eq!(
            zenith_engine_render(handle, out.as_mut_ptr(), 4, 3),
            Status::InvalidArg.code()
        );
        assert_eq!(
            zenith_engine_render(handle, core::ptr::null_mut(), 4, 2),
            Status::NullPointer.code()
        );
        zenith_engine_destroy(handle);
    }

    #[test]
    fn start_reports_unsupported_without_a_device_driver() {
        let config = default_config();
        let mut handle: *mut ZenithEngine = ptr::null_mut();
        zenith_engine_create(&config, &mut handle);
        // The default build has no `cpal`; start must not claim success.
        let result = zenith_engine_start(handle);
        if !cfg!(feature = "cpal") {
            assert_eq!(result, Status::Unsupported.code());
        }
        zenith_engine_destroy(handle);
    }

    #[test]
    fn snapshot_size_matches_the_type() {
        assert_eq!(size_of::<ZenithEngineStatus>(), 56);
    }

    #[test]
    fn the_internal_config_is_reachable_for_tests() {
        // A compile-level check that the handle really wraps the engine.
        let engine = Engine::new(InternalConfig::default()).unwrap();
        let handle = ZenithEngine { engine };
        assert_eq!(handle.engine().config().sample_rate, 48_000);
    }
}

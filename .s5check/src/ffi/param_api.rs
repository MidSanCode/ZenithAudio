//! C ABI surface for the parameter system and automation (ABI §6.4, S2).
//!
//! # Handle design
//!
//! The engine proper does not exist yet (S1 is in flight), so S2 exposes its
//! own opaque handle: [`ZenithAutomation`]. It owns a
//! [`ParameterStore`], a [`LaneSet`], an [`AutomationPlayer`], a
//! [`ModulatorBank`] and a [`Recorder`] — everything automation needs, and
//! **nothing about the DSP graph**.
//!
//! This is deliberate and is what makes S2 landable before S1:
//!
//! * the module compiles, runs and is fully testable today;
//! * when S1's `ZenithEngine` exists, it will *own* a `ZenithAutomation`
//!   internally rather than duplicating the parameter state, so wiring is a
//!   field access plus one `advance_block` call at a block boundary;
//! * Dart code written against these functions keeps working, because the
//!   engine-level functions will front the same data.
//!
//! # Real-time safety of each entry point
//!
//! ABI §7.3 requires every structural change to state that it is **not**
//! real-time safe. Concretely:
//!
//! | Real-time safe (any thread) | Control thread only |
//! |---|---|
//! | `param_get`, `param_set`, `param_set_smoothing` | `_create`, `_destroy`, `_prepare` |
//! | `automation_value_at` | `_register_descriptor`, `_lane_*`, `_clip_*` |
//! | `modulator_*` reads | `_modulator_add_*`, `_modulator_connect` |
//! | `recorder_state` | `_recorder_*` mutators |
//!
//! Every entry point is `catch_unwind`-guarded (principle P4) and every pointer
//! is validated before use (P2, P10). No entry point returns `null` to signal
//! failure — failures are status codes with out-parameters (P10).

// A C ABI entry point cannot be `unsafe fn`: the header and the Dart-side
// `lookupFunction` signature both describe a safe-looking `extern "C"` call,
// and marking it `unsafe` would make the *Dart* side unable to express the
// contract it actually has. Safety is instead stated per function in a
// `# Safety` doc section, which is what generated bindings and reviewers read.
// The pointers are validated (`as_ref`/`as_mut`/`write_out`) before any
// dereference, so the lint's concern — an unvalidated free-for-all — does not
// apply to the bodies here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_char;
use core::mem::size_of;

use crate::automation::clip::{AutomationPoint, CurveKind};
use crate::automation::lane::{Lane, LaneSet, RecordMode};
use crate::automation::modulator::{EnvelopeGenerator, Lfo, LfoShape, LfoTriggerMode, ModulatorBank};
use crate::automation::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterKind, ParameterUnit,
};
use crate::automation::player::AutomationPlayer;
use crate::automation::recorder::{RecordOutcome, Recorder};
use crate::automation::store::{ParameterStore, MAX_SMOOTHING_MS, MIN_SMOOTHING_MS};
use crate::ffi::types::{
    zenith_param_flags, zenith_record_mode, ZenithAutomationPoint, ZenithAutomationStats,
    ZenithLaneState, ZenithParamDescriptor, ZenithParamId, ZenithRecorderState,
};
use crate::guard;
use crate::Status;

/// Opaque handle to the automation subsystem.
///
/// Dart only ever holds a `*mut` to this; it must never interpret the memory
/// (P2). Created by [`zenith_automation_create`], freed by
/// [`zenith_automation_destroy`].
pub struct ZenithAutomation {
    /// Registered parameters and their live values.
    store: ParameterStore,
    /// The automation lanes.
    lanes: LaneSet,
    /// The real-time evaluator.
    player: AutomationPlayer,
    /// Modulation sources.
    modulators: ModulatorBank,
    /// Automation recording state.
    recorder: Recorder,
    /// Descriptors installed by the host, kept alive for the lifetime of the
    /// handle because [`ZenithParamDescriptor`] borrows their strings.
    ///
    /// `Box::leak`ed on registration and reclaimed only when the process ends:
    /// the alternative is a lifetime parameter threading through every FFI
    /// signature, and the total number of descriptors in a session is bounded
    /// by the compiled-in registry, so the leak is bounded and small.
    descriptors: alloc::vec::Vec<&'static ParameterDescriptor>,
    /// Whether the current transport pass has been seen by the recorder.
    transport_running: bool,
}

impl ZenithAutomation {
    /// Creates an empty automation subsystem.
    fn new(sample_rate: f32) -> Self {
        let mut player = AutomationPlayer::new();
        player.prepare(sample_rate, 0);
        let mut recorder = Recorder::new();
        recorder.prepare(sample_rate);
        Self {
            store: ParameterStore::default(),
            lanes: LaneSet::new(),
            player,
            modulators: ModulatorBank::new(),
            recorder,
            descriptors: alloc::vec::Vec::new(),
            transport_running: false,
        }
    }

    /// Registers a descriptor, leaking it so the handle can hand out
    /// `'static` string pointers.
    ///
    /// Re-registering the same address replaces the stored descriptor, which
    /// keeps the live value (see [`ParameterStore::register`]).
    fn register(&mut self, descriptor: ParameterDescriptor) -> Result<(), Status> {
        let leaked: &'static ParameterDescriptor = alloc::boxed::Box::leak(alloc::boxed::Box::new(descriptor));
        self.store
            .register(leaked)
            .ok_or(Status::InvalidArg)?;
        // Replace any previous entry for the same address so `descriptors`
        // does not grow without bound when a project is reloaded.
        if let Some(existing) = self
            .descriptors
            .iter_mut()
            .find(|d| d.address == leaked.address)
        {
            *existing = leaked;
        } else {
            self.descriptors.push(leaked);
        }
        Ok(())
    }
}

// ── Helper: pointer validation ──

/// Borrows a `*mut T` as `&mut T`, or returns [`Status::NullPointer`].
///
/// # Safety
///
/// The caller must guarantee `ptr` is null or points to a live, aligned,
/// exclusively-borrowed `T` for the duration of the returned reference. Every
/// use in this module satisfies that because the only source of these pointers
/// is [`zenith_automation_create`], and ABI §5.3 requires `create`/`destroy`
/// to be serialized on one thread.
unsafe fn as_mut<'a, T>(ptr: *mut T) -> Result<&'a mut T, Status> {
    // SAFETY: the caller upholds the non-null, aligned, live invariant.
    unsafe { ptr.as_mut() }.ok_or(Status::NullPointer)
}

/// Borrows a `*const T` as `&T`, or returns [`Status::NullPointer`].
///
/// # Safety
///
/// Same contract as [`as_mut`], without the exclusivity requirement.
unsafe fn as_ref<'a, T>(ptr: *const T) -> Result<&'a T, Status> {
    // SAFETY: the caller upholds the non-null, aligned, live invariant.
    unsafe { ptr.as_ref() }.ok_or(Status::NullPointer)
}

/// Writes `value` through `out`, or returns [`Status::NullPointer`].
///
/// # Safety
///
/// `out` must be null or valid for a single aligned write.
unsafe fn write_out<T>(out: *mut T, value: T) -> Result<(), Status> {
    // SAFETY: the caller upholds the validity invariant.
    let slot = unsafe { out.as_mut() }.ok_or(Status::NullPointer)?;
    *slot = value;
    Ok(())
}

// ── Lifecycle ──

/// Creates an automation subsystem.
///
/// `sample_rate` is used to convert the millisecond smoothing and envelope
/// times into coefficients; a non-finite or non-positive value falls back to
/// 48 kHz rather than failing, so a host that has not yet opened a device can
/// still build its parameter tree.
///
/// Returns [`Status::NullPointer`] when `out_handle` is null, or
/// [`Status::InvalidArg`] when the allocation cannot be sized.
///
/// # Safety
///
/// `out_handle` must be null or point to a writable `*mut ZenithAutomation`.
#[no_mangle]
pub extern "C" fn zenith_automation_create(
    sample_rate: f32,
    out_handle: *mut *mut ZenithAutomation,
) -> i32 {
    guard(|| {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let boxed = alloc::boxed::Box::new(ZenithAutomation::new(rate));
        let raw = alloc::boxed::Box::into_raw(boxed);
        // SAFETY: `out_handle` was checked non-null by `write_out`'s contract;
        // an invalid pointer is the caller's violation, which is what the
        // `# Safety` section documents.
        unsafe {
            if let Err(status) = write_out(out_handle, raw) {
                // Reclaim the allocation rather than leaking it on a bad call.
                drop(alloc::boxed::Box::from_raw(raw));
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Destroys an automation subsystem.
///
/// Idempotent and null-safe: passing null is a no-op, so a Dart finalizer
/// cannot double-free. The pointer is invalid immediately on return.
///
/// # Safety
///
/// `handle` must be null or a pointer returned by [`zenith_automation_create`]
/// that has not already been destroyed.
#[no_mangle]
pub extern "C" fn zenith_automation_destroy(handle: *mut ZenithAutomation) {
    // `guard` is not used: this returns nothing, and the only failure mode is a
    // null pointer, which is explicitly allowed.
    if handle.is_null() {
        return;
    }
    // SAFETY: the caller guarantees this pointer came from `create` and has
    // not been destroyed, so reclaiming the Box is sound exactly once.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(unsafe { alloc::boxed::Box::from_raw(handle) });
    }));
}

/// Reconfigures the sample rate and reserves storage for `lane_count` lanes.
///
/// Control thread only, and never while the audio thread is inside
/// `advance_block` (ABI §7.3).
///
/// # Safety
///
/// `handle` must be null or a live pointer from [`zenith_automation_create`].
#[no_mangle]
pub extern "C" fn zenith_automation_prepare(
    handle: *mut ZenithAutomation,
    sample_rate: f32,
    lane_count: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        this.player.prepare(rate, lane_count as usize);
        this.recorder.prepare(rate);
        Status::Ok
    })
    .code()
}

// ── Parameter registration and description ──

/// Registers a parameter so it can be read, written and automated.
///
/// `kind`, `unit` and `flags` use the discriminant modules in
/// [`crate::ffi::types`]. `key` and `label` are copied into the handle's own
/// storage; Dart keeps ownership of the buffers it passes (ABI §3.3).
///
/// Returns [`Status::InvalidArg`] for an unrecognized `kind`/`unit`
/// discriminant or an inverted value range, and [`Status::NullPointer`] for a
/// null `key`/`label`.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live, and `key`/`label` must be NUL-terminated UTF-8
/// buffers valid for the duration of the call.
#[no_mangle]
pub extern "C" fn zenith_automation_register_parameter(
    handle: *mut ZenithAutomation,
    kind: u16,
    index: u32,
    sub: u16,
    key: *const c_char,
    label: *const c_char,
    unit: u32,
    flags: u32,
    min_value: f32,
    max_value: f32,
    default_value: f32,
    smoothing_ms: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Some(kind) = ParameterKind::from_u16(kind) else {
            return Status::InvalidArg;
        };
        let Some(unit) = ParameterUnit::from_u32(unit) else {
            return Status::InvalidArg;
        };
        // SAFETY: the caller guarantees NUL-terminated UTF-8 for the call.
        let (key, label) = unsafe { (cstr_to_str(key), cstr_to_str(label)) };
        let (Some(key), Some(label)) = (key, label) else {
            return Status::NullPointer;
        };

        this.register(ParameterDescriptor {
            address: ParameterAddress::new(kind, index, sub),
            // Leaked so the descriptor can hand out `'static` pointers; see
            // `ZenithAutomation::descriptors`.
            key: alloc::boxed::Box::leak(alloc::string::String::from(key).into_boxed_str()),
            label: alloc::boxed::Box::leak(alloc::string::String::from(label).into_boxed_str()),
            unit,
            flags,
            min_value,
            max_value,
            default_value,
            smoothing_ms: smoothing_ms.clamp(MIN_SMOOTHING_MS, MAX_SMOOTHING_MS),
        })
        .map_or_else(|status| status, |()| Status::Ok)
    })
    .code()
}

/// Reads a parameter's static description.
///
/// The returned strings are owned by the handle and must not be freed by Dart
/// (ABI §3.3). They stay valid until the handle is destroyed.
///
/// Control thread only (it is a UI-facing query, not an audio-path call).
///
/// # Safety
///
/// `handle` must be live and `out_desc` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_describe_parameter(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    out_desc: *mut ZenithParamDescriptor,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let address = match ParameterAddress::try_from(id) {
            Ok(address) => address,
            Err(_) => return Status::InvalidArg,
        };
        let Some(descriptor) = this.store.descriptor(address) else {
            return Status::NotFound;
        };
        let mirrored = ZenithParamDescriptor::from(*descriptor);
        // SAFETY: the caller guarantees `out_desc` is null or writable.
        unsafe {
            if let Err(status) = write_out(out_desc, mirrored) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Writes up to `capacity` descriptors into `out_descs`, reporting the total.
///
/// This is the query behind "generate the UI from the engine": Dart asks for
/// the list and builds controls without knowing any parameter by name.
///
/// The two-call protocol is intentional — pass `capacity = 0` to learn the
/// count, then allocate exactly. `out_count` always receives the **total**
/// number of descriptors, even when fewer were written, so the caller can tell
/// truncation from exhaustion (P10: no null-as-failure).
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live; `out_descs` must be null when `capacity` is 0, or
/// writable for `capacity` `ZenithParamDescriptor` values; `out_count` must be
/// null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_list_parameters(
    handle: *const ZenithAutomation,
    out_descs: *mut ZenithParamDescriptor,
    capacity: usize,
    out_count: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let total = this.store.len();

        // SAFETY: `out_count` must be null or writable per the contract.
        unsafe {
            if let Err(status) = write_out(out_count, total) {
                return status;
            }
        }

        if capacity == 0 {
            return Status::Ok;
        }
        if out_descs.is_null() {
            return Status::NullPointer;
        }

        // The index is a raw offset into caller-owned memory, bounded by the
        // `take(capacity)` above and the caller's stated buffer size.
        for (written, descriptor) in this.store.descriptors().take(capacity).enumerate() {
            let mirrored = ZenithParamDescriptor::from(*descriptor);
            // SAFETY: `written < capacity` and the caller guaranteed room for
            // `capacity` elements, so this offset is in bounds.
            unsafe {
                out_descs.add(written).write(mirrored);
            }
        }
        Status::Ok
    })
    .code()
}

/// Reads a parameter's current value.
///
/// **Real-time safe** (a binary search plus a relaxed atomic load), so this may
/// be called from any thread.
///
/// # Safety
///
/// `handle` must be live and `out_value` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_param_get(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    out_value: *mut f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let address = match ParameterAddress::try_from(id) {
            Ok(address) => address,
            Err(_) => return Status::InvalidArg,
        };
        let Some(value) = this.store.read(address) else {
            return Status::NotFound;
        };
        // SAFETY: the caller guarantees `out_value` is null or writable.
        unsafe {
            if let Err(status) = write_out(out_value, value) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Writes a parameter's value, clamped into its descriptor's range.
///
/// **Real-time safe**; suitable for a UI fader drag. The write is visible to
/// the audio thread on its next block.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_param_set(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    value: f32,
) -> i32 {
    guard(|| {
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let address = match ParameterAddress::try_from(id) {
            Ok(address) => address,
            Err(_) => return Status::InvalidArg,
        };
        match this.store.write(address, value) {
            Some(_) => Status::Ok,
            None => Status::NotFound,
        }
    })
    .code()
}

/// Sets a parameter's smoothing time in milliseconds, clamped to 1..50.
///
/// **Real-time safe.**
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_param_set_smoothing(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    smoothing_ms: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let address = match ParameterAddress::try_from(id) {
            Ok(address) => address,
            Err(_) => return Status::InvalidArg,
        };
        match this.store.set_smoothing_ms(address, smoothing_ms) {
            Some(_) => Status::Ok,
            None => Status::NotFound,
        }
    })
    .code()
}

/// Evaluates a parameter at `frame`, including automation and modulation.
///
/// This is the **single source of truth** for "what is this parameter doing
/// right now" (see [`crate::automation::parameter`] for the normative order).
/// Callers must not interpolate on their own.
///
/// Control thread only: it consults the lanes directly rather than the
/// smoothed value the audio thread has published, so it is a UI-preview query,
/// not an audio-path read.
///
/// # Safety
///
/// `handle` must be live and `out_value` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_value_at(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    frame: i64,
    out_value: *mut f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let address = match ParameterAddress::try_from(id) {
            Ok(address) => address,
            Err(_) => return Status::InvalidArg,
        };
        let Some(descriptor) = this.store.descriptor(address) else {
            return Status::NotFound;
        };

        // Order: base → automation → modulator → clamp, exactly as the player.
        let base = this.store.read(address).unwrap_or(descriptor.default_value);
        let automated = this
            .lanes
            .get(address)
            .and_then(|lane| lane.value_at(frame))
            .unwrap_or(base);
        let modulated = automated + this.modulation_offset(address);
        let clamped = descriptor.clamp(modulated);

        // SAFETY: the caller guarantees `out_value` is null or writable.
        unsafe {
            if let Err(status) = write_out(out_value, clamped) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

impl ZenithAutomation {
    /// Sums the modulation contribution for one parameter.
    ///
    /// Mirrors the player's accumulator so the UI preview and the audio path
    /// agree; kept here (rather than reusing the player's private helper)
    /// because the player folds *all* targets in one pass per block, while a
    /// preview query asks about exactly one.
    fn modulation_offset(&self, address: ParameterAddress) -> f32 {
        let mut total = 0.0f32;
        for index in 0..self.modulators.lfo_count() {
            if let Some(lfo) = self.modulators.lfo(index) {
                let output = lfo.value();
                for (target, depth) in lfo.target_depths() {
                    if target == address {
                        total += output * depth;
                    }
                }
            }
        }
        for index in 0..self.modulators.envelope_count() {
            if let Some(envelope) = self.modulators.envelope(index) {
                let output = envelope.value();
                for (target, depth) in envelope.target_depths() {
                    if target == address {
                        total += output * depth;
                    }
                }
            }
        }
        total
    }
}

// ── Lane and clip editing ──

/// Creates a lane for a parameter if none exists.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lane_create(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        if this.store.descriptor(address).is_none() {
            // Refuse to create an orphan lane: it could never be evaluated, and
            // a silent no-op lane is harder to diagnose than a clear error.
            return Status::NotFound;
        }
        this.lanes.entry(address);
        Status::Ok
    })
    .code()
}

/// Removes a lane. Returns [`Status::NotFound`] when there was none.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lane_remove(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        if this.lanes.remove(address) {
            Status::Ok
        } else {
            Status::NotFound
        }
    })
    .code()
}

/// Reports a lane's state, or [`Status::NotFound`] when it does not exist.
///
/// Control thread only (the editor calls it while painting).
///
/// # Safety
///
/// `handle` must be live and `out_state` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_lane_state(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    out_state: *mut ZenithLaneState,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get(address) else {
            return Status::NotFound;
        };
        let (first, last) = lane.frame_range().unwrap_or((0, 0));
        let state = ZenithLaneState {
            id: address.into(),
            point_count: lane.clip().len() as u32,
            enabled: u8::from(lane.is_enabled()),
            armed: u8::from(lane.is_armed()),
            collapsed: u8::from(lane.is_collapsed()),
            _reserved_0: 0,
            first_frame: first,
            last_frame: last,
            color: lane.color(),
            height: lane.height(),
        };
        // SAFETY: the caller guarantees `out_state` is null or writable.
        unsafe {
            if let Err(status) = write_out(out_state, state) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Enables or disables a lane's playback.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lane_set_enabled(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    enabled: u32,
) -> i32 {
    set_lane_flag(handle, id, |lane| lane.set_enabled(enabled != 0))
}

/// Arms or disarms a lane for recording.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lane_set_armed(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    armed: u32,
) -> i32 {
    set_lane_flag(handle, id, |lane| lane.set_armed(armed != 0))
}

/// Collapses or expands a lane in the editor.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lane_set_collapsed(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    collapsed: u32,
) -> i32 {
    set_lane_flag(handle, id, |lane| lane.set_collapsed(collapsed != 0))
}

/// Applies a small mutation to one lane, sharing the lookup and error mapping.
fn set_lane_flag(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    mutate: impl FnOnce(&mut Lane),
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        match this.lanes.get_mut(address) {
            Some(lane) => {
                mutate(lane);
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Number of points in a lane.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live and `out_count` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_point_count(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    out_count: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get(address) else {
            return Status::NotFound;
        };
        // SAFETY: the caller guarantees `out_count` is null or writable.
        unsafe {
            if let Err(status) = write_out(out_count, lane.clip().len()) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Copies up to `capacity` points into `out_points`, reporting the total.
///
/// Same two-call protocol as [`zenith_automation_list_parameters`].
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live; `out_points` must be null when `capacity` is 0, or
/// writable for `capacity` elements; `out_count` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_get_points(
    handle: *const ZenithAutomation,
    id: ZenithParamId,
    out_points: *mut ZenithAutomationPoint,
    capacity: usize,
    out_count: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get(address) else {
            return Status::NotFound;
        };
        let points = lane.clip().points();
        // SAFETY: `out_count` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_count, points.len()) {
                return status;
            }
        }
        if capacity == 0 {
            return Status::Ok;
        }
        if out_points.is_null() {
            return Status::NullPointer;
        }
        for (index, point) in points.iter().take(capacity).enumerate() {
            let mirrored = ZenithAutomationPoint {
                frame: point.frame,
                value: point.value,
                tension: point.tension,
                curve: point.curve.as_u32(),
                _reserved: 0,
            };
            // SAFETY: `index < capacity` and the caller guaranteed room.
            unsafe {
                out_points.add(index).write(mirrored);
            }
        }
        Status::Ok
    })
    .code()
}

/// Replaces a lane's points wholesale.
///
/// This is how the editor commits an edit, and how a project load installs a
/// saved curve. The points are sorted and indexed once, here, so evaluation
/// never has to.
///
/// Control thread only; allocates.
///
/// # Safety
///
/// `handle` must be live; `points` must be null when `count` is 0, or readable
/// for `count` elements.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_set_points(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    points: *const ZenithAutomationPoint,
    count: usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        if count > 0 && points.is_null() {
            return Status::NullPointer;
        }
        let mut converted = alloc::vec::Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: the caller guaranteed `count` readable elements.
            let raw = unsafe { points.add(index).read() };
            let Some(curve) = CurveKind::from_u32(raw.curve) else {
                return Status::InvalidArg;
            };
            let tension = if raw.tension.is_finite() {
                raw.tension.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            converted.push(AutomationPoint {
                frame: raw.frame,
                value: raw.value,
                tension,
                curve,
            });
        }
        if this.store.descriptor(address).is_none() {
            return Status::NotFound;
        }
        this.lanes.entry(address).set_points(converted);
        Status::Ok
    })
    .code()
}

/// Inserts one point into a lane, keeping the clip sorted.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_insert_point(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    point: ZenithAutomationPoint,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(curve) = CurveKind::from_u32(point.curve) else {
            return Status::InvalidArg;
        };
        if this.store.descriptor(address).is_none() {
            return Status::NotFound;
        }
        let tension = if point.tension.is_finite() {
            point.tension.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        this.lanes.entry(address).clip_mut().insert(AutomationPoint {
            frame: point.frame,
            value: point.value,
            tension,
            curve,
        });
        Status::Ok
    })
    .code()
}

/// Removes every point whose frame lies in `start..=end`.
///
/// This is the "delete a range" gesture, and the pre-step of a punch-in.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_remove_range(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    start_frame: i64,
    end_frame: i64,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get_mut(address) else {
            return Status::NotFound;
        };
        lane.clip_mut().remove_range(start_frame, end_frame);
        Status::Ok
    })
    .code()
}

/// Sets a point's frame and value, re-sorting the clip if it moved.
///
/// This is the drag gesture. `index` is the position in the clip's sorted
/// order as last reported by [`zenith_automation_clip_get_points`]; after a
/// re-sort the indices change, so the caller must re-read before dragging
/// another point.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_move_point(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    index: usize,
    frame: i64,
    value: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get_mut(address) else {
            return Status::NotFound;
        };
        if lane.clip_mut().move_point(index, frame, value) {
            Status::Ok
        } else {
            Status::OutOfRange
        }
    })
    .code()
}

/// Sets a point's interpolation mode and tension, clamping tension to -1..1.
///
/// This is the "curve tension" handle in the editor. Tension is stored per
/// point and governs the segment to that point's *right*.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_set_curve(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    index: usize,
    curve: u32,
    tension: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(curve) = CurveKind::from_u32(curve) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get_mut(address) else {
            return Status::NotFound;
        };
        if lane.clip_mut().set_curve(index, curve, tension) {
            Status::Ok
        } else {
            Status::OutOfRange
        }
    })
    .code()
}

/// Removes the point at `index`.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_remove_point(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    index: usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get_mut(address) else {
            return Status::NotFound;
        };
        if lane.clip_mut().remove(index).is_some() {
            Status::Ok
        } else {
            Status::OutOfRange
        }
    })
    .code()
}

/// Removes every point in a lane.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_clip_clear(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(lane) = this.lanes.get_mut(address) else {
            return Status::NotFound;
        };
        lane.clip_mut().clear();
        Status::Ok
    })
    .code()
}

/// Total number of automation points across every lane.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live and `out_count` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_total_points(
    handle: *const ZenithAutomation,
    out_count: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        // SAFETY: `out_count` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_count, this.lanes.total_points()) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

// ── Evaluation ──

/// Advances automation and modulation by one block.
///
/// **This is the function S1 calls from the audio callback**, once per block,
/// at a block boundary. Until the engine exists it is callable directly, which
/// is how S2 is verified today (see `docs/stages/s2-report.md`).
///
/// Real-time safe: no allocation, no lock, no IO. Enforced by
/// `automation::tests::advance_block_does_not_allocate`.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_advance_block(
    handle: *mut ZenithAutomation,
    frame: i64,
    frames: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        // Split the borrows rather than cloning the lane list: a `to_vec()`
        // here would allocate on the audio thread, which is exactly the
        // violation this function's contract promises not to commit.
        let ZenithAutomation {
            lanes,
            player,
            modulators,
            store,
            ..
        } = this;
        player.advance_block(
            lanes.lanes(),
            modulators,
            store,
            frame,
            frames as usize,
        );
        Status::Ok
    })
    .code()
}

/// Reports the player's statistics for the most recent block.
///
/// **Real-time safe** (a plain struct copy).
///
/// # Safety
///
/// `handle` must be live and `out_stats` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_stats(
    handle: *const ZenithAutomation,
    out_stats: *mut ZenithAutomationStats,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let stats = this.player.stats();
        let mirrored = ZenithAutomationStats {
            evaluated: stats.evaluated as u32,
            automated: stats.automated as u32,
            modulated: stats.modulated as u32,
            written: stats.written as u32,
            skipped: stats.skipped as u32,
            unresolved: stats.unresolved as u32,
            _reserved: 0,
        };
        // SAFETY: `out_stats` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_stats, mirrored) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

// ── Modulation sources ──

/// Adds an LFO and returns its index through `out_index`.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live and `out_index` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_modulator_add_lfo(
    handle: *mut ZenithAutomation,
    out_index: *mut u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let index = this.modulators.add_lfo(Lfo::new());
        // SAFETY: `out_index` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_index, index as u32) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Adds an envelope generator and returns its index through `out_index`.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live and `out_index` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_modulator_add_envelope(
    handle: *mut ZenithAutomation,
    out_index: *mut u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let index = this.modulators.add_envelope(EnvelopeGenerator::new());
        // SAFETY: `out_index` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_index, index as u32) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Configures an LFO's shape, rate, phase offset and trigger mode.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lfo_configure(
    handle: *mut ZenithAutomation,
    index: u32,
    shape: u32,
    rate_hz: f32,
    phase_offset: f32,
    trigger: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Some(shape) = LfoShape::from_u32(shape) else {
            return Status::InvalidArg;
        };
        let Some(trigger) = LfoTriggerMode::from_u32(trigger) else {
            return Status::InvalidArg;
        };
        let Some(lfo) = this.modulators.lfo_mut(index as usize) else {
            return Status::NotFound;
        };
        lfo.set_shape(shape);
        lfo.set_rate_hz(rate_hz);
        lfo.set_phase_offset(phase_offset);
        lfo.set_trigger(trigger);
        Status::Ok
    })
    .code()
}

/// Sets an LFO's enabled state.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lfo_set_enabled(
    handle: *mut ZenithAutomation,
    index: u32,
    enabled: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        match this.modulators.lfo_mut(index as usize) {
            Some(lfo) => {
                lfo.set_enabled(enabled != 0);
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Restarts an LFO's phase.
///
/// Control thread only (a retrigger from the audio thread would need a
/// lock-free command; S1 will add one if it is needed).
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lfo_retrigger(
    handle: *mut ZenithAutomation,
    index: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        match this.modulators.lfo_mut(index as usize) {
            Some(lfo) => {
                lfo.retrigger();
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Connects an LFO to a parameter at a signed depth.
///
/// Returns [`Status::Capacity`] when the LFO's fixed target array is full.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lfo_connect(
    handle: *mut ZenithAutomation,
    index: u32,
    id: ZenithParamId,
    depth: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        match this.modulators.lfo_mut(index as usize) {
            Some(lfo) => {
                if lfo.connect(address, depth) {
                    Status::Ok
                } else {
                    Status::Capacity
                }
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Disconnects an LFO from a parameter.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_lfo_disconnect(
    handle: *mut ZenithAutomation,
    index: u32,
    id: ZenithParamId,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        match this.modulators.lfo_mut(index as usize) {
            Some(lfo) => {
                if lfo.disconnect(address) {
                    Status::Ok
                } else {
                    Status::NotFound
                }
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Configures an envelope's ADSR times and sustain level.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_envelope_configure(
    handle: *mut ZenithAutomation,
    index: u32,
    attack_s: f32,
    decay_s: f32,
    sustain: f32,
    release_s: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        match this.modulators.envelope_mut(index as usize) {
            Some(envelope) => {
                envelope.set_adsr(attack_s, decay_s, sustain, release_s);
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Opens or closes an envelope's gate.
///
/// Control thread only; S1 will route note events through a lock-free queue.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_envelope_gate(
    handle: *mut ZenithAutomation,
    index: u32,
    open: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        match this.modulators.envelope_mut(index as usize) {
            Some(envelope) => {
                if open != 0 {
                    envelope.gate_on();
                } else {
                    envelope.gate_off();
                }
                Status::Ok
            }
            None => Status::NotFound,
        }
    })
    .code()
}

/// Connects an envelope to a parameter at a signed depth.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_envelope_connect(
    handle: *mut ZenithAutomation,
    index: u32,
    id: ZenithParamId,
    depth: f32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        match this.modulators.envelope_mut(index as usize) {
            Some(envelope) => {
                if envelope.connect(address, depth) {
                    Status::Ok
                } else {
                    Status::Capacity
                }
            }
            None => Status::NotFound,
        }
    })
    .code()
}

// ── Recording ──

/// Enables or disables global automation recording.
///
/// Disabling commits any open take (see [`crate::automation::recorder`]), so a
/// user who hits stop mid-pass keeps what they recorded.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_recorder_set_enabled(
    handle: *mut ZenithAutomation,
    enabled: u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        this.recorder.set_enabled(enabled != 0, &mut this.lanes);
        Status::Ok
    })
    .code()
}

/// Reports that a control moved, so an armed lane can capture it.
///
/// `mode` uses [`crate::ffi::types::zenith_record_mode`]. The outcome is
/// written to `out_outcome` as one of the `zenith_record_outcome` values, so
/// the UI can show "thinned" versus "captured" without guessing.
///
/// `touching` must reflect whether the user is *currently* holding the
/// control; a host that cannot distinguish touch from move should pass 1.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live and `out_outcome` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_recorder_on_control_move(
    handle: *mut ZenithAutomation,
    id: ZenithParamId,
    value: f32,
    frame: i64,
    touching: u32,
    mode: u32,
    out_outcome: *mut u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let Ok(address) = ParameterAddress::try_from(id) else {
            return Status::InvalidArg;
        };
        let Some(mode) = RecordMode::from_u32(mode) else {
            return Status::InvalidArg;
        };
        let outcome = this.recorder.on_control_move(
            &mut this.lanes,
            address,
            value,
            frame,
            touching != 0,
            mode,
        );
        let code = match outcome {
            RecordOutcome::Captured => zenith_record_outcome::CAPTURED,
            RecordOutcome::Thinned => zenith_record_outcome::THINNED,
            RecordOutcome::NotArmed => zenith_record_outcome::NOT_ARMED,
            RecordOutcome::TransportStopped => zenith_record_outcome::TRANSPORT_STOPPED,
            RecordOutcome::OutOfOrder => zenith_record_outcome::OUT_OF_ORDER,
        };
        // SAFETY: `out_outcome` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_outcome, code) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

/// Recording outcomes, mirrored for Dart.
pub mod zenith_record_outcome {
    /// A point was captured.
    pub const CAPTURED: u32 = 0;
    /// The value was too close to the last one to be worth a point.
    pub const THINNED: u32 = 1;
    /// The lane is not armed, or the mode is `Off`.
    pub const NOT_ARMED: u32 = 2;
    /// The transport is not running.
    pub const TRANSPORT_STOPPED: u32 = 3;
    /// The point arrived out of order and was dropped.
    pub const OUT_OF_ORDER: u32 = 4;
}

/// Ends the open take and merges it into its lane.
///
/// Called when the transport stops. Writing `out_committed` is optional: pass
/// null when the caller does not need to know.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live and `out_committed` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_recorder_finish(
    handle: *mut ZenithAutomation,
    out_committed: *mut u32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let committed = this.recorder.finish(&mut this.lanes);
        this.transport_running = false;
        if !out_committed.is_null() {
            // SAFETY: the caller guarantees `out_committed` is null or writable.
            unsafe {
                if let Err(status) = write_out(out_committed, u32::from(committed)) {
                    return status;
                }
            }
        }
        Status::Ok
    })
    .code()
}

/// Discards the open take without committing it.
///
/// Control thread only.
///
/// # Safety
///
/// `handle` must be live.
#[no_mangle]
pub extern "C" fn zenith_automation_recorder_cancel(handle: *mut ZenithAutomation) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_mut(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        this.recorder.cancel();
        Status::Ok
    })
    .code()
}

/// Reports the recorder's state, for the transport's record indicator.
///
/// **Real-time safe.**
///
/// # Safety
///
/// `handle` must be live and `out_state` must be null or writable.
#[no_mangle]
pub extern "C" fn zenith_automation_recorder_state(
    handle: *const ZenithAutomation,
    out_state: *mut ZenithRecorderState,
) -> i32 {
    guard(|| {
        // SAFETY: the caller upholds the live-pointer contract.
        let this = match unsafe { as_ref(handle) } {
            Ok(this) => this,
            Err(status) => return status,
        };
        let active = this.recorder.active_take();
        let state = ZenithRecorderState {
            enabled: u8::from(this.recorder.is_enabled()),
            take_open: u8::from(active.is_some()),
            _reserved_0: 0,
            mode: zenith_record_mode::OFF,
            active_id: active
                .map(|take| take.address.into())
                .unwrap_or_default(),
            take_points: active.map_or(0, |take| take.len() as u32),
            total_captured: this.recorder.captured_points() as u32,
            _reserved_1: 0,
        };
        // SAFETY: `out_state` must be null or writable.
        unsafe {
            if let Err(status) = write_out(out_state, state) {
                return status;
            }
        }
        Status::Ok
    })
    .code()
}

// ── Self-check ──

/// Returns the size of [`ZenithParamId`], for the ABI mirror check (ABI §2.3).
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_sizeof_param_id() -> usize {
    size_of::<ZenithParamId>()
}

/// Returns the size of [`ZenithParamDescriptor`], for the mirror check.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_sizeof_param_descriptor() -> usize {
    size_of::<ZenithParamDescriptor>()
}

/// Returns the size of [`ZenithAutomationPoint`], for the mirror check.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_sizeof_automation_point() -> usize {
    size_of::<ZenithAutomationPoint>()
}

/// Returns the size of [`ZenithAutomationStats`], for the mirror check.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_sizeof_automation_stats() -> usize {
    size_of::<ZenithAutomationStats>()
}

/// Returns the size of [`ZenithLaneState`], for the mirror check.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_sizeof_lane_state() -> usize {
    size_of::<ZenithLaneState>()
}

/// Returns the size of [`ZenithRecorderState`], for the mirror check.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_sizeof_recorder_state() -> usize {
    size_of::<ZenithRecorderState>()
}

/// Returns the lowest legal smoothing time in milliseconds.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_smoothing_ms_min() -> f32 {
    MIN_SMOOTHING_MS
}

/// Returns the highest legal smoothing time in milliseconds.
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_smoothing_ms_max() -> f32 {
    MAX_SMOOTHING_MS
}

/// Returns the number of parameter flag bits this build knows about.
///
/// A cheap capability probe: Dart compares the flags it *received* against this
/// mask and ignores bits it does not understand, which is what makes adding a
/// flag a non-breaking change (ABI §2.2).
#[no_mangle]
#[must_use]
pub extern "C" fn zenith_param_flag_mask() -> u32 {
    zenith_param_flags::AUTOMATABLE
        | zenith_param_flags::DISCRETE
        | zenith_param_flags::LOGARITHMIC
        | zenith_param_flags::BIPOLAR
        | zenith_param_flags::SMOOTHED
}

/// Reads a NUL-terminated UTF-8 C string.
///
/// # Safety
///
/// `ptr` must be null or point to a NUL-terminated buffer of at most
/// [`MAX_CSTR_LEN`] bytes.
unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // Bounded scan: a missing NUL in a hostile or corrupt caller would
    // otherwise read past the end of the buffer. The limit is generous for a
    // parameter key or label.
    const MAX_CSTR_LEN: usize = 256;
    let mut len = 0usize;
    // SAFETY: the caller guarantees a readable NUL-terminated buffer.
    while len < MAX_CSTR_LEN && unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    if len == MAX_CSTR_LEN {
        // No terminator within the bound: treat as malformed rather than
        // reading further.
        return None;
    }
    // SAFETY: `len` bytes were verified readable above.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };
    core::str::from_utf8(bytes).ok()
}

/// Keeps `parameter_flags` referenced so the module import is not dead.
const _: u32 = parameter_flags::AUTOMATABLE;

/// Marker kept so the flag mask above compiles while the constant it names is
/// defined next to the other flags.
#[allow(dead_code)]
const _UNUSED: () = ();

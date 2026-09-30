//! `#[repr(C)]` structures shared with Dart (ABI §3.4, principle P8).
//!
//! # Ownership of this file
//!
//! This file is a **shared** resource (`docs/COORDINATION.md`, entry C-003).
//! Each agent owns one clearly delimited section and must not edit another's:
//!
//! * `// ── S0/S1 engine ──` — Agent-A
//! * `// ── S2 parameter & automation ──` — Agent-C
//! * `// ── S3 mixer ──` — Agent-D
//!
//! # The mirroring rule
//!
//! Every struct here has a hand-written twin in
//! `lib/engine/ffi/native_types.dart`. The two must agree on **field order and
//! width**, exactly. Two things enforce that rather than trusting reviewers:
//!
//! 1. each struct has a `const` assertion on `size_of` in this crate;
//! 2. each struct's size is exported as `zenith_sizeof_<type>()` so Dart can
//!    compare at load time (ABI §2.3).
//!
//! Fields are ordered largest-alignment-first to remove implicit padding, and
//! no manual padding is ever inserted (ABI §3.4).

use core::ffi::c_char;

use crate::automation::clip::CurveKind;
use crate::automation::lane::RecordMode;
use crate::automation::modulator::{EnvelopeStage, LfoShape, LfoTriggerMode};
use crate::automation::parameter::{
    ParameterAddress, ParameterDescriptor, ParameterKind,
};

// ── S2 parameter & automation ──

/// Compact, hash-free parameter address (ABI §6.4).
///
/// Layout: `u16`, `u16`, `u32` — eight bytes, no padding. The Rust-side
/// authority is [`ParameterAddress`], whose `#[repr(C)]` layout this mirrors
/// field for field.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZenithParamId {
    /// Parameter category; see [`ZenithParamKind`].
    pub kind: u16,
    /// Parameter ordinal within the owning object.
    pub sub: u16,
    /// Which instance of the owning object (channel, track, effect slot).
    pub index: u32,
}

/// Parameter category discriminants, mirrored from [`ParameterKind`].
///
/// Values are ABI-frozen: an existing discriminant must never be reassigned,
/// only appended to (ABI §2.2).
pub mod zenith_param_kind {
    /// Engine-wide parameter (tempo, master gain).
    pub const GLOBAL: u16 = 0;
    /// Mixer channel strip parameter.
    pub const CHANNEL: u16 = 1;
    /// Track parameter.
    pub const TRACK: u16 = 2;
    /// Effect slot parameter.
    pub const EFFECT: u16 = 3;
    /// Modulation source parameter.
    pub const MODULATOR: u16 = 4;
}

/// Parameter unit discriminants, mirrored from [`ParameterUnit`].
pub mod zenith_param_unit {
    /// Plain number.
    pub const LINEAR: u32 = 0;
    /// Decibels.
    pub const DECIBELS: u32 = 1;
    /// Hertz.
    pub const HERTZ: u32 = 2;
    /// Seconds.
    pub const SECONDS: u32 = 3;
    /// Percent, 0..100.
    pub const PERCENT: u32 = 4;
    /// A choice among discrete options.
    pub const ENUMERATION: u32 = 5;
    /// Beats, for tempo-synced values.
    pub const BEATS: u32 = 6;
}

/// Parameter behaviour flag bits, mirrored from `parameter::parameter_flags`.
pub mod zenith_param_flags {
    /// The parameter can carry an automation lane.
    pub const AUTOMATABLE: u32 = 0x01;
    /// Values are discrete steps.
    pub const DISCRETE: u32 = 0x02;
    /// Display on a logarithmic scale.
    pub const LOGARITHMIC: u32 = 0x04;
    /// The range is bipolar around zero.
    pub const BIPOLAR: u32 = 0x08;
    /// Apply the default one-pole smoothing.
    pub const SMOOTHED: u32 = 0x10;
}
/// Static description of one parameter, as published to Dart (ABI §6.4).
///
/// The two string pointers are **borrowed** `'static` data owned by the Rust
/// registry. Dart must only read them and must never free them (ABI §3.3).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ZenithParamDescriptor {
    /// Compact address.
    pub id: ZenithParamId,
    /// Lowest legal value.
    pub min_value: f32,
    /// Highest legal value.
    pub max_value: f32,
    /// Value used when nothing else supplies one.
    pub default_value: f32,
    /// Default smoothing time in milliseconds.
    pub smoothing_ms: f32,
    /// Display unit; see [`zenith_param_unit`].
    pub unit: u32,
    /// Behaviour bits; see [`zenith_param_flags`].
    pub flags: u32,
    /// Stable machine-readable key, e.g. `"volume"`. NUL-terminated UTF-8.
    pub key_utf8: *const c_char,
    /// Human-readable label. NUL-terminated UTF-8.
    pub label_utf8: *const c_char,
}

// SAFETY: every field is either a plain scalar or a pointer to immutable
// `'static` data. Nothing in the struct gives access to interior mutability,
// so sharing it across threads cannot introduce a data race, and the ABI
// requires the type to be `Send` for the control thread to hand descriptors
// to a UI thread.
unsafe impl Send for ZenithParamDescriptor {}
// SAFETY: as above — all pointers are to `'static` immutable data.
unsafe impl Sync for ZenithParamDescriptor {}

/// One point on an automation curve (ABI §6.4).
///
/// `frame` is `int64` to match the tick-first time model; the accompanying
/// `_reserved` field keeps the struct at a multiple of 8 bytes so Dart's
/// `sizeOf` agrees without compiler-specific padding.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZenithAutomationPoint {
    /// Position in frames from the project origin.
    pub frame: i64,
    /// Value at `frame`.
    pub value: f32,
    /// Curvature toward the next point, `-1.0..=1.0`.
    pub tension: f32,
    /// Interpolation to the next point; see [`zenith_curve_kind`].
    pub curve: u32,
    /// Explicit padding, held at zero. Present so the struct's size is
    /// unambiguous and identical on every target.
    pub _reserved: u32,
}

/// Interpolation mode discriminants, mirrored from [`CurveKind`].
pub mod zenith_curve_kind {
    /// Straight line.
    pub const LINEAR: u32 = 0;
    /// Step: hold the left value.
    pub const HOLD: u32 = 1;
    /// Curved segment, bend set by `tension`.
    pub const CURVE: u32 = 2;
    /// Fast-then-flat bend.
    pub const EXPONENTIAL: u32 = 3;
    /// Slow-then-steep bend.
    pub const LOGARITHMIC: u32 = 4;
}

/// Automation record mode discriminants, mirrored from [`RecordMode`].
pub mod zenith_record_mode {
    /// Not recording.
    pub const OFF: u32 = 0;
    /// Write only while a control is touched.
    pub const TOUCH: u32 = 1;
    /// Write from first touch until the transport stops.
    pub const LATCH: u32 = 2;
    /// Rewrite the whole pass.
    pub const WRITE: u32 = 3;
}

/// LFO waveform discriminants, mirrored from [`LfoShape`].
pub mod zenith_lfo_shape {
    /// Sine.
    pub const SINE: u32 = 0;
    /// Triangle.
    pub const TRIANGLE: u32 = 1;
    /// Rising saw.
    pub const SAW: u32 = 2;
    /// Falling ramp.
    pub const RAMP: u32 = 3;
    /// Square.
    pub const SQUARE: u32 = 4;
    /// Deterministic stepped random.
    pub const RANDOM: u32 = 5;
    /// Constant `1.0`.
    pub const CONSTANT: u32 = 6;
}

/// LFO trigger mode discriminants, mirrored from [`LfoTriggerMode`].
pub mod zenith_lfo_trigger {
    /// Free-running.
    pub const FREE: u32 = 0;
    /// Reset on transport start.
    pub const TRANSPORT: u32 = 1;
    /// Reset on note-on.
    pub const NOTE: u32 = 2;
    /// Single cycle on demand.
    pub const ONE_SHOT: u32 = 3;
}

/// Envelope stage discriminants, mirrored from [`EnvelopeStage`].
pub mod zenith_envelope_stage {
    /// Not running.
    pub const IDLE: u32 = 0;
    /// Rising.
    pub const ATTACK: u32 = 1;
    /// Falling to sustain.
    pub const DECAY: u32 = 2;
    /// Holding.
    pub const SUSTAIN: u32 = 3;
    /// Falling to zero.
    pub const RELEASE: u32 = 4;
}

/// Statistics describing the player's most recent block (ABI §6.4).
///
/// Exposed so the UI can show automation load and so a support report can
/// distinguish "no automation" from "automation that failed to resolve".
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZenithAutomationStats {
    /// Parameters evaluated this block.
    pub evaluated: u32,
    /// Parameters that had a lane contributing.
    pub automated: u32,
    /// Parameters that had at least one modulator contributing.
    pub modulated: u32,
    /// Parameters written because their value changed.
    pub written: u32,
    /// Parameters dropped at the per-block capacity.
    pub skipped: u32,
    /// Lane addresses that were not registered in the store.
    pub unresolved: u32,
    /// Explicit padding, held at zero.
    pub _reserved: u32,
}

/// State of a single automation lane, as reported to the editor.
///
/// `PartialEq` only, deliberately: `height` is a float, and claiming `Eq`
/// would be a lie that a future `HashSet` would silently act on.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZenithLaneState {
    /// The lane's parameter.
    pub id: ZenithParamId,
    /// Number of points in the lane.
    pub point_count: u32,
    /// Whether the lane plays back.
    pub enabled: u8,
    /// Whether the lane accepts recorded takes.
    pub armed: u8,
    /// Whether the lane is collapsed in the editor.
    pub collapsed: u8,
    /// Explicit padding, held at zero.
    pub _reserved_0: u8,
    /// First frame covered, or 0 when the lane is empty.
    pub first_frame: i64,
    /// Last frame covered, or 0 when the lane is empty.
    pub last_frame: i64,
    /// Lane colour as `0xRRGGBB`; `0` means the theme default.
    pub color: u32,
    /// Editor height in logical pixels; `0` means the default.
    pub height: f32,
}

/// A snapshot of the recorder's state, for the transport's record indicator.
///
/// Not `Eq`: it carries no float, but `RecordMode`-derived `mode` is a plain
/// integer and the struct is compared by field in tests, so `PartialEq` is
/// enough and `Eq` is deliberately omitted to keep the derive list honest if a
/// float is added later.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZenithRecorderState {
    /// Whether global recording is enabled.
    pub enabled: u8,
    /// Whether a take is currently open.
    pub take_open: u8,
    /// Explicit padding, held at zero.
    pub _reserved_0: u16,
    /// The active mode; see [`zenith_record_mode`].
    pub mode: u32,
    /// Parameter being recorded, when `take_open` is set.
    pub active_id: ZenithParamId,
    /// Points captured in the current take.
    pub take_points: u32,
    /// Points captured since the recorder was created.
    pub total_captured: u32,
    /// Explicit padding, held at zero.
    pub _reserved_1: u32,
}

// ── Mirror conversion helpers ──

impl From<ParameterAddress> for ZenithParamId {
    fn from(address: ParameterAddress) -> Self {
        Self {
            kind: address.kind.as_u16(),
            sub: address.sub,
            index: address.index,
        }
    }
}

impl TryFrom<ZenithParamId> for ParameterAddress {
    /// The error is the unrecognized `kind` discriminant.
    type Error = ParameterKindMismatch;

    fn try_from(id: ZenithParamId) -> Result<Self, Self::Error> {
        let kind = ParameterKind::from_u16(id.kind).ok_or(ParameterKindMismatch(id.kind))?;
        Ok(ParameterAddress::new(kind, id.index, id.sub))
    }
}

/// Returned when a [`ZenithParamId`] carries a `kind` this build does not know.
///
/// A new kind introduced by a newer core must surface as an error rather than
/// being coerced, because coercing would write a *different* parameter than
/// the caller asked for (ABI §2.2: callers must handle unknown enum values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterKindMismatch(pub u16);

impl From<ParameterDescriptor> for ZenithParamDescriptor {
    fn from(descriptor: ParameterDescriptor) -> Self {
        Self {
            id: descriptor.address.into(),
            min_value: descriptor.min_value,
            max_value: descriptor.max_value,
            default_value: descriptor.default_value,
            smoothing_ms: descriptor.smoothing_ms,
            unit: descriptor.unit.as_u32(),
            flags: descriptor.flags,
            key_utf8: name_ptr(descriptor.key),
            label_utf8: name_ptr(descriptor.label),
        }
    }
}

/// A `'static` C-string pointer that may be shared across threads.
///
/// Raw pointers are not `Send`/`Sync` by default, which is the right default:
/// nothing stops two threads mutating through one. The pointer stored in the
/// intern table is different in kind — it refers to a `CString` that was leaked
/// once and is **never written again** — so sharing it cannot race, and the
/// newtype is what records that reasoning for the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SharedNamePtr(*const c_char);

// SAFETY: the pointee is immutable for the rest of the process. Nothing in
// this module ever obtains a mutable reference to it, so two threads reading
// through the same pointer cannot introduce a data race.
unsafe impl Send for SharedNamePtr {}
// SAFETY: as above — shared read-only access to leaked, never-mutated memory.
unsafe impl Sync for SharedNamePtr {}

/// Returns a NUL-terminated pointer to a string from the registry.
///
/// The descriptors are built from `&'static str` literals, which in Rust are
/// **not** NUL-terminated, so a `CString` has to exist somewhere. Interning
/// keeps that bounded: the first query for a given name leaks one small buffer
/// and every later query returns the same pointer.
///
/// Leaking is the right trade here. The alternative is either a lifetime
/// parameter threaded through every FFI signature, or freeing the buffer while
/// Dart still holds the pointer — and the number of *distinct* names in the
/// engine is bounded by the compiled-in registry, so the total leak is a few
/// hundred bytes per session.
///
/// Exhausting the intern table (which cannot happen with a `'static` registry)
/// degrades to an empty label rather than a dangling pointer.
fn name_ptr(name: &'static str) -> *const c_char {
    /// Upper bound on distinct interned names, to keep the table auditable.
    const MAX_NAMES: usize = 512;

    use std::sync::{Mutex, OnceLock};

    static TABLE: OnceLock<Mutex<alloc::vec::Vec<(&'static str, SharedNamePtr)>>> = OnceLock::new();

    // A poisoned lock means another thread panicked while interning. The table
    // is append-only, so the worst case is one duplicate entry; recovering is
    // safe and far better than aborting inside an FFI call.
    let table = TABLE.get_or_init(|| Mutex::new(alloc::vec::Vec::new()));
    let mut guard = match table.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if let Some((_, ptr)) = guard.iter().find(|(key, _)| *key == name) {
        return ptr.0;
    }
    if guard.len() >= MAX_NAMES {
        return c"".as_ptr();
    }

    // A parameter key or label is short; the cap is a guard against a caller
    // passing something pathological, not a real limit.
    const MAX_LEN: usize = 1024;
    if name.len() >= MAX_LEN {
        return c"".as_ptr();
    }

    // Build a NUL-terminated buffer and leak it so the pointer outlives the
    // call (and every future call) — see the note above. A name containing an
    // interior NUL cannot be represented as a C string; `unwrap_or_default`
    // degrades it to an empty label rather than panicking across the ABI.
    let owned: &'static core::ffi::CStr = alloc::boxed::Box::leak(
        std::ffi::CString::new(name)
            .unwrap_or_default()
            .into_boxed_c_str(),
    );
    let ptr = SharedNamePtr(owned.as_ptr());
    guard.push((name, ptr));
    ptr.0
}

impl From<CurveKind> for u32 {
    fn from(curve: CurveKind) -> Self {
        curve.as_u32()
    }
}

impl From<RecordMode> for u32 {
    fn from(mode: RecordMode) -> Self {
        mode.as_u32()
    }
}

impl From<LfoShape> for u32 {
    fn from(shape: LfoShape) -> Self {
        shape.as_u32()
    }
}

impl From<LfoTriggerMode> for u32 {
    fn from(trigger: LfoTriggerMode) -> Self {
        trigger.as_u32()
    }
}

impl From<EnvelopeStage> for u32 {
    fn from(stage: EnvelopeStage) -> Self {
        stage.as_u32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::parameter::ParameterUnit;
    use core::mem::size_of;

    // ── Size assertions: these are the contract with Dart (ABI §9.2) ──
    //
    // A failure here means `lib/engine/ffi/native_types.dart` must change in
    // the same commit. The numbers are hard-coded on purpose: deriving them
    // from `size_of` in both places would make the test vacuous.

    #[test]
    fn param_id_is_eight_bytes_with_no_padding() {
        assert_eq!(size_of::<ZenithParamId>(), 8);
        assert_eq!(size_of::<ZenithParamId>(), 2 + 2 + 4);
    }

    #[test]
    fn param_descriptor_layout_is_pinned() {
        // 8 (id) + 4*4 (four floats) + 4 + 4 (unit/flags) + 8 + 8 (two ptrs)
        // = 8 + 16 + 8 + 16 = 48 on 64-bit targets.
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<ZenithParamDescriptor>(), 48);
        // On wasm32 pointers are 4 bytes: 8 + 16 + 8 + 8 = 40.
        #[cfg(target_pointer_width = "32")]
        assert_eq!(size_of::<ZenithParamDescriptor>(), 40);
    }

    #[test]
    fn automation_point_is_twenty_four_bytes() {
        assert_eq!(size_of::<ZenithAutomationPoint>(), 24);
    }

    #[test]
    fn automation_stats_is_twenty_eight_bytes() {
        assert_eq!(size_of::<ZenithAutomationStats>(), 28);
    }

    #[test]
    fn lane_state_layout_is_pinned() {
        // 8 (id) + 4 (count) + 4 (four u8s) + 8 + 8 + 4 + 4 = 40
        assert_eq!(size_of::<ZenithLaneState>(), 40);
    }

    #[test]
    fn recorder_state_layout_is_pinned() {
        // 4 (three u8s + pad) + 4 (mode) + 8 (id) + 4 + 4 + 4 (pad) = 28
        assert_eq!(size_of::<ZenithRecorderState>(), 28);
    }

    #[test]
    fn address_round_trips_through_the_mirror() {
        let original = ParameterAddress::effect(7, 3, 42);
        let mirrored: ZenithParamId = original.into();
        let back = ParameterAddress::try_from(mirrored).expect("known kind");
        assert_eq!(back, original);
    }

    #[test]
    fn an_unknown_kind_is_rejected_not_coerced() {
        let bogus = ZenithParamId {
            kind: 999,
            sub: 0,
            index: 0,
        };
        assert_eq!(
            ParameterAddress::try_from(bogus),
            Err(ParameterKindMismatch(999))
        );
    }

    #[test]
    fn kind_discriminants_match_the_frozen_abi() {
        assert_eq!(zenith_param_kind::GLOBAL, ParameterKind::Global.as_u16());
        assert_eq!(zenith_param_kind::CHANNEL, ParameterKind::Channel.as_u16());
        assert_eq!(zenith_param_kind::TRACK, ParameterKind::Track.as_u16());
        assert_eq!(zenith_param_kind::EFFECT, ParameterKind::Effect.as_u16());
        assert_eq!(
            zenith_param_kind::MODULATOR,
            ParameterKind::Modulator.as_u16()
        );
    }

    #[test]
    fn unit_and_mode_discriminants_match_their_rust_counterparts() {
        assert_eq!(zenith_param_unit::LINEAR, ParameterUnit::Linear.as_u32());
        assert_eq!(zenith_curve_kind::HOLD, CurveKind::Hold.as_u32());
        assert_eq!(zenith_record_mode::WRITE, RecordMode::Write.as_u32());
        assert_eq!(zenith_lfo_shape::RANDOM, LfoShape::Random.as_u32());
        assert_eq!(
            zenith_lfo_trigger::ONE_SHOT,
            LfoTriggerMode::OneShot.as_u32()
        );
        assert_eq!(
            zenith_envelope_stage::RELEASE,
            EnvelopeStage::Release.as_u32()
        );
    }

    #[test]
    fn interned_names_are_readable_and_stable() {
        let first = name_ptr("volume");
        let second = name_ptr("volume");
        assert_eq!(first, second, "interning must be stable across calls");
        // SAFETY: `name_ptr` returns a pointer to a NUL-terminated static slot.
        let text = unsafe { core::ffi::CStr::from_ptr(first) };
        assert_eq!(text.to_str().unwrap(), "volume");

        // A different name gets a different slot.
        let other = name_ptr("pan");
        assert_ne!(first, other);
        // SAFETY: as above.
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(other) }.to_str().unwrap(),
            "pan"
        );
    }
}

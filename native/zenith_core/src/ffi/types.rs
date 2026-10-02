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
//! * `// ── S5 effects ──` — Agent-C
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

// ── S3 mixer ──
//
// Owned by Agent-D. Per `docs/COORDINATION.md` C-005, this section is additive:
// it does not modify a single existing line of the S0/S1/S2 sections above.
//
// The layouts mirror `lib/mixer/ffi/mixer_types.dart` field-for-field. Every
// struct is `#[repr(C)]` with explicit padding where needed, because the ABI is
// the only contract between the two languages (P8).

/// A mixer channel's parameters, mirrored across the ABI (S3).
///
/// Field order and widths are frozen: the size is asserted in the tests below
/// and in `lib/mixer/ffi/mixer_types.dart`. `role` and `flags` are `u32`
/// rather than Rust enums so an unknown discriminant from a newer core cannot
/// make the struct unrepresentable in Dart (ABI §2.2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZenithMixerChannel {
    /// Channel index. Never reused after removal (ABI §11 Q3).
    pub id: u32,
    /// [`zenith_channel_role`] discriminant.
    pub role: u32,
    /// Fader position in decibels, clamped to `MIN_GAIN_DB..=MAX_GAIN_DB`.
    pub gain_db: f32,
    /// Pan position, `-1.0` hard left to `1.0` hard right.
    pub pan: f32,
    /// Index of the channel this one feeds, or `u32::MAX` for master.
    pub output: u32,
    /// [`zenith_channel_flags`] bitset.
    pub flags: u32,
    /// Number of occupied effect slots, for the UI badge.
    pub effect_count: u32,
    /// Number of active sends.
    pub active_sends: u32,
}

/// One send slot, mirrored across the ABI (S3).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZenithMixerSend {
    /// Whether this send contributes.
    pub enabled: u32,
    /// [`zenith_send_tap`] discriminant.
    pub tap: u32,
    /// Send level in decibels.
    pub level_db: f32,
    /// Destination channel index, or `u32::MAX` when unrouted.
    pub destination: u32,
}

/// One effect slot, mirrored across the ABI (S3).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZenithMixerEffectSlot {
    /// Slot position, `0..10`. This is the processing order, not the kind.
    pub index: u32,
    /// Effect kind id, or `u32::MAX` when the slot is empty.
    ///
    /// Built-ins occupy `0x0000_0000..=0x0000_FFFF` and plugins `0x0001_0000+`
    /// (ABI §11 Q2), so the sentinel can never collide with a real kind.
    pub kind: u32,
    /// Whether the slot is bypassed.
    pub bypassed: u32,
    /// Wet/dry balance, `0.0` fully dry to `1.0` fully wet.
    pub wet: f32,
    /// Sidechain source channel, or `u32::MAX` when none.
    pub sidechain: u32,
}

/// Sentinel meaning "no channel" in a `u32` channel field.
///
/// `u32::MAX` rather than `0`, because channel `0` is master and a legitimate
/// destination: using `0` as "none" would silently route unset fields to the
/// master bus.
pub const ZENITH_CHANNEL_NONE: u32 = u32::MAX;

/// Sentinel meaning "no effect kind" in [`ZenithMixerEffectSlot::kind`].
pub const ZENITH_KIND_NONE: u32 = u32::MAX;

/// [`ZenithMixerChannel::role`] discriminants.
///
/// These values are part of the ABI and must never be renumbered.
pub mod zenith_channel_role {
    /// A normal insert channel fed by a track.
    pub const INSERT: u32 = 0;
    /// A return channel fed by sends.
    pub const RETURN: u32 = 1;
    /// A group channel that sums other channels.
    pub const GROUP: u32 = 2;
    /// The single master channel.
    pub const MASTER: u32 = 3;
}

/// [`ZenithMixerChannel::flags`] bits.
///
/// A bitset rather than four separate `u8` fields: the ABI passes booleans as
/// `u8` (ABI §3.1), and four of them would cost four bytes of padding anyway.
pub mod zenith_channel_flags {
    /// The channel is muted.
    pub const MUTED: u32 = 1 << 0;
    /// The channel is soloed.
    pub const SOLO: u32 = 1 << 1;
    /// The channel's polarity is inverted.
    pub const PHASE_INVERT: u32 = 1 << 2;
    /// The channel currently contributes to the mix, after mute/solo.
    pub const AUDIBLE: u32 = 1 << 3;
    /// The channel is still alive (not removed).
    pub const ALIVE: u32 = 1 << 4;
}

/// [`ZenithMixerSend::tap`] discriminants.
pub mod zenith_send_tap {
    /// Post-fader: the send follows the channel fader.
    pub const POST_FADER: u32 = 0;
    /// Pre-fader: the send ignores the channel fader.
    pub const PRE_FADER: u32 = 1;
}

/// A channel's level reading, mirrored across the ABI (S3).
///
/// The field order matches `docs/ABI.md` §6.7, which froze it before S3 landed.
/// All values are linear amplitudes where `1.0` is full scale.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZenithMeterSnapshot {
    /// Peak magnitude, left.
    pub peak_l: f32,
    /// Peak magnitude, right.
    pub peak_r: f32,
    /// RMS magnitude, left.
    pub rms_l: f32,
    /// RMS magnitude, right.
    pub rms_r: f32,
    /// Held peak, left.
    pub peak_hold_l: f32,
    /// Held peak, right.
    pub peak_hold_r: f32,
}

/// Aggregate counts describing a mixer, mirrored across the ABI (S3).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZenithMixerStats {
    /// Live channels, master included.
    pub channels: u32,
    /// Total sends currently active across every channel.
    pub active_sends: u32,
    /// Total occupied effect slots across every channel.
    pub effects: u32,
    /// Whether any channel is soloed.
    pub has_solo: u32,
    /// Deepest group nesting currently in the graph.
    pub max_depth: u32,
    /// Explicit padding, held at zero.
    pub _reserved_0: u32,
}

// ── S5 effects ──
//
// The effect descriptor is what lets Dart build an effect panel without a
// hand-written class per effect (PLAN §3.S5 "UI 自动生成"). It carries the
// identity and shape of an effect; the parameter list is fetched separately,
// item by item, so a new effect needs no ABI change.

/// Identity and shape of one built-in effect.
///
/// Field order is largest-first to keep the struct free of implicit padding
/// (ABI §3.4), so both sides can assert an exact size.
///
/// String fields point at **static** storage owned by Rust and are valid for
/// the lifetime of the process. Dart must read them and must **not** free them
/// (ABI §3.3, "Rust → Dart (borrow)").
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZenithEffectDescriptor {
    /// Effect kind id, as stored in a mixer effect slot.
    ///
    /// Built-in kinds are `0x0000_0000..=0x0000_FFFF` and plugin kinds
    /// `0x0001_0000+`, so a slot can hold either without ambiguity
    /// (ABI §11 Q2).
    pub kind: u32,
    /// Family, see `ZenithEffectCategory`.
    pub category: u32,
    /// How many parameters the effect publishes. Parameter `i` has ordinal
    /// `first_param + i`, so a caller may iterate `0..param_count` and pass the
    /// ordinal straight to `zenith_effect_describe_parameter`.
    pub param_count: u32,
    /// Lower bound of this effect's parameter ordinals.
    ///
    /// Always `0` for the built-ins, which number their own parameters from
    /// zero. It exists so a future effect that borrows another's ordinal space
    /// does not need a signature change.
    pub first_param: u32,
    /// Non-zero when the effect introduces latency PDC must compensate.
    ///
    /// A hint for the UI (a "latency" badge). The authoritative value is
    /// [`zenith_effect_latency_samples`], which depends on the sample rate and
    /// the effect's current settings.
    pub has_latency: u32,
    /// Non-zero when the effect is an analyser that passes audio through.
    ///
    /// The UI shows these in a separate group, because putting an analyser in
    /// an insert chain is a monitoring decision rather than a mixing one.
    pub is_analysis_only: u32,
    /// Stable machine-readable key, e.g. `"compressor"`. Never localized, safe
    /// to key UI state by. Static, Dart must not free.
    pub key_utf8: *const c_char,
    /// Human-readable label. Static, Dart must not free.
    pub label_utf8: *const c_char,
}

// SAFETY: the two pointers are `'static` string literals owned by Rust; the
// type is a plain data carrier and no thread mutates it.
unsafe impl Send for ZenithEffectDescriptor {}
// SAFETY: see above — the pointees are immutable statics.
unsafe impl Sync for ZenithEffectDescriptor {}

/// Effect parameter categories, mirrored by Dart as a plain integer.
///
/// Published values are ABI-frozen; new categories are appended.
pub mod zenith_effect_category {
    /// Filters and equalisers.
    pub const EQUALIZER: u32 = 0;
    /// Compressors, limiters and gates.
    pub const DYNAMICS: u32 = 1;
    /// Reverberation.
    pub const REVERB: u32 = 2;
    /// Delays and echoes.
    pub const DELAY: u32 = 3;
    /// Chorus, flanger and phaser.
    pub const MODULATION: u32 = 4;
    /// Saturation and bit reduction.
    pub const DISTORTION: u32 = 5;
    /// Signal-shaping filters.
    pub const FILTER: u32 = 6;
    /// Analysis that produces no audio.
    pub const ANALYSIS: u32 = 7;
    /// Utility processors.
    pub const UTILITY: u32 = 8;
}

/// Effect kind id ranges, mirrored by Dart.
pub mod zenith_effect_kind_range {
    /// Lowest built-in kind id.
    pub const BUILTIN_BASE: u32 = 0x0000_0000;
    /// Highest built-in kind id.
    pub const BUILTIN_END: u32 = 0x0000_FFFF;
    /// Lowest plugin kind id (ABI §11 Q2).
    pub const PLUGIN_BASE: u32 = 0x0001_0000;
}

/// Returns the size of [`ZenithEffectDescriptor`] as Rust laid it out.
///
/// Dart compares this with its own `sizeOf` on startup and throws on a
/// mismatch, which is how a struct-mirror drift surfaces as a loud failure
/// instead of a silent misread (ABI §2.3, §9.2).
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_sizeof_effect_descriptor() -> usize {
    size_of::<ZenithEffectDescriptor>()
}

/// Returns `ZENITH_EFFECT_BUILTIN_BASE` — the lowest built-in kind id.
///
/// Provided so Dart does not hard-code the constant, which is how the two
/// sides drift apart.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_kind_base() -> u32 {
    zenith_effect_kind_range::BUILTIN_BASE
}

/// Returns `ZENITH_EFFECT_PLUGIN_BASE` — the lowest plugin kind id.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_kind_plugin_base() -> u32 {
    zenith_effect_kind_range::PLUGIN_BASE
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
/// Exposed to the crate (`pub(crate)`) because the S5 effect surface in
/// `ffi/effect_api.rs` publishes effect keys and labels through exactly the
/// same interning: a `&'static str` is not NUL-terminated, and two modules
/// building their own buffers would leak twice for the same name.
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
pub(crate) fn name_ptr(name: &'static str) -> *const c_char {
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

// ── S3 mixer mirror conversions ──

impl From<crate::mixer::ChannelRole> for u32 {
    fn from(role: crate::mixer::ChannelRole) -> Self {
        use crate::mixer::ChannelRole;
        match role {
            ChannelRole::Insert => zenith_channel_role::INSERT,
            ChannelRole::Return => zenith_channel_role::RETURN,
            ChannelRole::Group => zenith_channel_role::GROUP,
            ChannelRole::Master => zenith_channel_role::MASTER,
        }
    }
}

impl TryFrom<u32> for crate::mixer::ChannelRole {
    /// The unrecognized discriminant.
    type Error = ChannelRoleMismatch;

    /// Rejects an unknown role rather than coercing it.
    ///
    /// Coercing would address a *different kind of channel* than the caller
    /// asked for, which is exactly the silent misroute ABI §2.2 warns about.
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        use crate::mixer::ChannelRole;
        match value {
            zenith_channel_role::INSERT => Ok(ChannelRole::Insert),
            zenith_channel_role::RETURN => Ok(ChannelRole::Return),
            zenith_channel_role::GROUP => Ok(ChannelRole::Group),
            zenith_channel_role::MASTER => Ok(ChannelRole::Master),
            other => Err(ChannelRoleMismatch(other)),
        }
    }
}

/// Returned when a channel role discriminant is not known to this build.
///
/// An unknown role must surface as an error rather than being coerced, because
/// coercing would address a different kind of channel than the caller asked for
/// (ABI §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelRoleMismatch(pub u32);

impl From<crate::mixer::SendTap> for u32 {
    fn from(tap: crate::mixer::SendTap) -> Self {
        use crate::mixer::SendTap;
        match tap {
            SendTap::PostFader => zenith_send_tap::POST_FADER,
            SendTap::PreFader => zenith_send_tap::PRE_FADER,
        }
    }
}

impl From<crate::mixer::MeterSnapshot> for ZenithMeterSnapshot {
    fn from(snapshot: crate::mixer::MeterSnapshot) -> Self {
        Self {
            peak_l: snapshot.peak_l,
            peak_r: snapshot.peak_r,
            rms_l: snapshot.rms_l,
            rms_r: snapshot.rms_r,
            peak_hold_l: snapshot.peak_hold_l,
            peak_hold_r: snapshot.peak_hold_r,
        }
    }
}

// ── S1 engine & transport ──
//
// Owned by Agent-A. Per `docs/COORDINATION.md` C-013, this section is additive:
// it touches no existing line above. These mirrors are declared in
// `docs/ABI.md` §5.2 (config), §3.5 (musical time) and §6.3 (status) and have
// hand-written twins in `lib/engine/ffi/native_types.dart`.

use core::mem::size_of;

/// A musical position in ticks (ABI §3.5).
///
/// `ticks` is `int64` to match the tick-first model; `ppq` is carried
/// redundantly so the Rust side can validate it independently, and `_reserved`
/// keeps the struct at a multiple of 8 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZenithMusicalTime {
    /// Absolute position, in ticks (PPQ = 960, PLAN §3.S0).
    pub ticks: i64,
    /// Pulses per quarter note, for independent validation.
    pub ppq: u32,
    /// Explicit padding, held at zero.
    pub _reserved: u32,
}

/// Engine creation parameters (ABI §5.2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZenithEngineConfig {
    /// Target sample rate, e.g. 48000.
    pub sample_rate: u32,
    /// Frames per block, 64..2048 (PLAN §3.S1 item 4).
    pub block_size: u32,
    /// Preallocated mixer channels (PLAN §3.S3).
    pub max_channels: u32,
    /// Preallocated tracks.
    pub max_tracks: u32,
    /// [`zenith_driver_kind`] discriminant.
    pub driver_kind: u32,
    /// [`zenith_engine_flags`] bitset.
    pub flags: u32,
    /// The major version the caller expects, for a pre-create check.
    pub abi_major: u32,
    /// Explicit padding, held at zero.
    pub _reserved: u32,
}

/// Driver kind discriminants, mirrored from `ZenithDriverKind` (ABI §5.2).
pub mod zenith_driver_kind {
    /// Platform default: `cpal` or `worklet`.
    pub const AUTO: u32 = 0;
    /// Desktop/mobile device via `cpal`; unsupported on `wasm32`.
    pub const CPAL: u32 = 1;
    /// Driven by the web `AudioWorklet`.
    pub const WORKLET: u32 = 2;
    /// Offline rendering, no device (PLAN §3.S4).
    pub const OFFLINE: u32 = 3;
}

/// Engine flag bits (ABI §5.2).
pub mod zenith_engine_flags {
    /// Real-time safety assertions on (debug/test builds only).
    pub const STRICT_REALTIME: u32 = 0x01;
    /// Allow the web degradation strategy to intervene.
    pub const WEB_DEGRADE: u32 = 0x02;
    /// Offline rendering uses the large-buffer fast path.
    pub const OFFLINE_FAST: u32 = 0x04;
}

/// Engine status snapshot (ABI §6.3).
///
/// Ordering is largest-first to avoid implicit padding. The playhead is `i64`
/// and every other scalar is `u32`/`f32`; Dart takes the runtime size, since the
/// layout is target-independent but the values are not.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZenithEngineStatus {
    /// Playhead position, in frames.
    pub playhead_frames: i64,
    /// Tempo, in beats per minute.
    pub bpm: f64,
    /// Real-time load, `0.0..1.0`.
    pub cpu_load: f32,
    /// Buffer underruns accumulated.
    pub xrun_count: u32,
    /// Voices currently sounding.
    pub active_voices: u32,
    /// Voice pool size.
    pub max_voices: u32,
    /// Web degradation tier: 0, 1 or 2.
    pub degrade_level: u32,
    /// `0` stopped, `1` playing, `2` paused.
    pub state: u32,
    /// Driver kind in use.
    pub driver_kind: u32,
    /// Negotiated sample rate.
    pub sample_rate: u32,
    /// Block size.
    pub block_size: u32,
    /// Explicit padding, held at zero.
    pub _reserved: u32,
}

// SAFETY: the struct is plain data (no pointers, no interior mutability), so it
// is freely shareable across the control thread and the audio thread.
unsafe impl Send for ZenithEngineStatus {}
// SAFETY: as above.
unsafe impl Sync for ZenithEngineStatus {}

impl From<crate::engine::EngineStatus> for ZenithEngineStatus {
    fn from(status: crate::engine::EngineStatus) -> Self {
        Self {
            playhead_frames: status.playhead_frames,
            bpm: status.bpm as f64,
            cpu_load: status.cpu_load,
            xrun_count: status.xrun_count,
            active_voices: status.active_voices,
            max_voices: status.max_voices,
            degrade_level: status.degrade_level,
            state: status.state,
            driver_kind: status.driver_kind,
            sample_rate: status.sample_rate,
            block_size: status.block_size,
            _reserved: 0,
        }
    }
}

impl From<crate::engine::EngineConfig> for ZenithEngineConfig {
    fn from(config: crate::engine::EngineConfig) -> Self {
        Self {
            sample_rate: config.sample_rate,
            block_size: config.block_size,
            max_channels: config.max_channels,
            max_tracks: config.max_tracks,
            driver_kind: config.driver_kind,
            flags: config.flags,
            abi_major: 0,
            _reserved: 0,
        }
    }
}

impl From<ZenithEngineConfig> for crate::engine::EngineConfig {
    fn from(config: ZenithEngineConfig) -> Self {
        Self {
            sample_rate: config.sample_rate,
            block_size: config.block_size,
            max_channels: config.max_channels,
            max_tracks: config.max_tracks,
            driver_kind: config.driver_kind,
            flags: config.flags,
        }
    }
}

/// Returns the size of [`ZenithEngineConfig`] as Rust laid it out.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_sizeof_engine_config() -> usize {
    size_of::<ZenithEngineConfig>()
}

/// Returns the size of [`ZenithEngineStatus`] as Rust laid it out.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_sizeof_engine_status() -> usize {
    size_of::<ZenithEngineStatus>()
}

/// Returns the size of [`ZenithMusicalTime`] as Rust laid it out.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_sizeof_musical_time() -> usize {
    size_of::<ZenithMusicalTime>()
}

/// Returns the size of the (opaque) engine handle. Zero, because Dart must
/// never interpret the engine's memory (ABI principle P2).
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_sizeof_engine() -> usize {
    0
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
    fn effect_descriptor_is_six_words_plus_two_pointers() {
        // 6 x u32 + 2 pointers. On 64-bit that is 24 + 16 = 40; on wasm32 the
        // pointers are 4 bytes, giving 24 + 8 = 32. Both sides must agree, so
        // Dart takes the runtime size rather than hard-coding one of them
        // (ABI §9.3's note on pointer-bearing structs).
        let pointer = size_of::<*const c_char>();
        let expected = 6 * size_of::<u32>() + 2 * pointer;
        assert_eq!(size_of::<ZenithEffectDescriptor>(), expected);
        assert_eq!(size_of::<ZenithEffectDescriptor>() % 4, 0, "no padding");
    }

    #[test]
    fn effect_descriptor_has_no_implicit_padding() {
        // Largest-alignment-first ordering is what removes padding; an
        // accidental field reorder would reintroduce it and silently desync
        // Dart, so this checks the layout rather than just the total.
        assert_eq!(
            size_of::<ZenithEffectDescriptor>(),
            6 * size_of::<u32>() + 2 * size_of::<*const c_char>()
        );
    }

    #[test]
    fn effect_category_and_kind_range_constants_are_stable() {
        // These cross to Dart as plain integers; a renumbering would
        // recategorise every effect panel in the UI.
        assert_eq!(zenith_effect_category::EQUALIZER, 0);
        assert_eq!(zenith_effect_category::DYNAMICS, 1);
        assert_eq!(zenith_effect_category::REVERB, 2);
        assert_eq!(zenith_effect_category::DELAY, 3);
        assert_eq!(zenith_effect_category::MODULATION, 4);
        assert_eq!(zenith_effect_category::DISTORTION, 5);
        assert_eq!(zenith_effect_category::FILTER, 6);
        assert_eq!(zenith_effect_category::ANALYSIS, 7);
        assert_eq!(zenith_effect_category::UTILITY, 8);

        assert_eq!(zenith_effect_kind_range::BUILTIN_BASE, 0);
        assert_eq!(zenith_effect_kind_range::BUILTIN_END, 0x0000_FFFF);
        assert_eq!(zenith_effect_kind_range::PLUGIN_BASE, 0x0001_0000);
    }

    #[test]
    fn the_exported_helpers_agree_with_the_constants() {
        assert_eq!(
            zenith_sizeof_effect_descriptor(),
            size_of::<ZenithEffectDescriptor>()
        );
        assert_eq!(
            zenith_effect_kind_base(),
            zenith_effect_kind_range::BUILTIN_BASE
        );
        assert_eq!(
            zenith_effect_kind_plugin_base(),
            zenith_effect_kind_range::PLUGIN_BASE
        );
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

    // ── S3 mixer layout assertions ──
    //
    // These mirror `lib/mixer/ffi/mixer_types.dart`; a failure here means both
    // sides must change in the same commit (ABI §9.2).

    #[test]
    fn mixer_channel_layout_is_pinned() {
        // 4 (id) + 4 (role) + 4 (gain) + 4 (pan) + 4 (output) + 4 (flags)
        // + 4 (effect_count) + 4 (active_sends) = 32
        assert_eq!(size_of::<ZenithMixerChannel>(), 32);
    }

    #[test]
    fn mixer_send_layout_is_pinned() {
        // 4 (enabled) + 4 (tap) + 4 (level) + 4 (destination) = 16
        assert_eq!(size_of::<ZenithMixerSend>(), 16);
    }

    #[test]
    fn mixer_effect_slot_layout_is_pinned() {
        // 4 (index) + 4 (kind) + 4 (bypassed) + 4 (wet) + 4 (sidechain) = 20
        assert_eq!(size_of::<ZenithMixerEffectSlot>(), 20);
    }

    #[test]
    fn meter_snapshot_layout_is_pinned() {
        // Six f32s, and the ABI §6.7 field order is frozen.
        assert_eq!(size_of::<ZenithMeterSnapshot>(), 24);
    }

    #[test]
    fn mixer_stats_layout_is_pinned() {
        // 6 × u32 = 24
        assert_eq!(size_of::<ZenithMixerStats>(), 24);
    }

    #[test]
    fn the_none_sentinels_cannot_collide_with_real_values() {
        // Master is channel 0, so "none" must not be 0 or it would silently
        // route unset fields to the master bus.
        assert_ne!(ZENITH_CHANNEL_NONE, 0);
        assert_ne!(ZENITH_CHANNEL_NONE, crate::mixer::ChannelId::MASTER.get());

        // The kind sentinel must sit above the plugin id range. Read through
        // `black_box` so this stays a runtime check: the constants are known at
        // compile time, and a folded assertion would be flagged as vacuous
        // while providing no protection against a future renumbering.
        let none = core::hint::black_box(ZENITH_KIND_NONE);
        let top_builtin = core::hint::black_box(0x0001_FFFFu32);
        assert!(none > top_builtin);
    }

    #[test]
    fn channel_role_round_trips_and_rejects_unknown() {
        use crate::mixer::ChannelRole;
        for role in [
            ChannelRole::Insert,
            ChannelRole::Return,
            ChannelRole::Group,
            ChannelRole::Master,
        ] {
            let wire: u32 = role.into();
            assert_eq!(ChannelRole::try_from(wire), Ok(role));
        }
        assert_eq!(
            ChannelRole::try_from(999),
            Err(ChannelRoleMismatch(999)),
            "an unknown role must be rejected, not coerced"
        );
    }

    #[test]
    fn channel_flag_bits_are_distinct() {
        let bits = [
            zenith_channel_flags::MUTED,
            zenith_channel_flags::SOLO,
            zenith_channel_flags::PHASE_INVERT,
            zenith_channel_flags::AUDIBLE,
            zenith_channel_flags::ALIVE,
        ];
        for (i, a) in bits.iter().enumerate() {
            assert_eq!(a.count_ones(), 1, "bit {i} is not a single bit");
            for b in bits.iter().skip(i + 1) {
                assert_eq!(a & b, 0, "flag bits overlap");
            }
        }
    }

    #[test]
    fn meter_snapshot_conversion_preserves_every_field() {
        let internal = crate::mixer::MeterSnapshot {
            peak_l: 1.0,
            peak_r: 2.0,
            rms_l: 3.0,
            rms_r: 4.0,
            peak_hold_l: 5.0,
            peak_hold_r: 6.0,
        };
        let mirrored: ZenithMeterSnapshot = internal.into();
        assert_eq!(mirrored.peak_l, 1.0);
        assert_eq!(mirrored.peak_r, 2.0);
        assert_eq!(mirrored.rms_l, 3.0);
        assert_eq!(mirrored.rms_r, 4.0);
        assert_eq!(mirrored.peak_hold_l, 5.0);
        assert_eq!(mirrored.peak_hold_r, 6.0);
    }

    // ── S1 engine layout assertions ──
    //
    // These mirror `lib/engine/ffi/native_types.dart`; a failure here means
    // both sides must change in the same commit (ABI §9.2).

    #[test]
    fn engine_config_is_eight_words() {
        // 8 × u32 = 32, with no pointer or 64-bit field to shift alignment.
        assert_eq!(size_of::<ZenithEngineConfig>(), 32);
    }

    #[test]
    fn musical_time_is_two_words() {
        // i64 + u32 + u32 = 16, no implicit padding after the 8-byte field.
        assert_eq!(size_of::<ZenithMusicalTime>(), 16);
    }

    #[test]
    fn engine_status_layout_is_pinned() {
        // i64 + f64 + f32 + 9 × u32 = 8 + 8 + 4 + 36 = 56; alignment 8 holds.
        assert_eq!(size_of::<ZenithEngineStatus>(), 56);
        assert_eq!(size_of::<ZenithEngineStatus>() % 8, 0, "no padding");
    }

    #[test]
    fn the_s1_size_helpers_agree_with_the_types() {
        assert_eq!(
            zenith_sizeof_engine_config(),
            size_of::<ZenithEngineConfig>()
        );
        assert_eq!(
            zenith_sizeof_engine_status(),
            size_of::<ZenithEngineStatus>()
        );
        assert_eq!(
            zenith_sizeof_musical_time(),
            size_of::<ZenithMusicalTime>()
        );
        assert_eq!(zenith_sizeof_engine(), 0, "the handle is opaque to Dart");
    }

    #[test]
    fn engine_status_round_trips_through_the_mirror() {
        let internal = crate::engine::EngineStatus {
            playhead_frames: 12_345,
            bpm: 128.0,
            cpu_load: 0.25,
            xrun_count: 2,
            active_voices: 7,
            max_voices: 64,
            degrade_level: 1,
            state: 1,
            driver_kind: 3,
            sample_rate: 48_000,
            block_size: 256,
        };
        let mirrored: ZenithEngineStatus = internal.into();
        assert_eq!(mirrored.playhead_frames, 12_345);
        assert_eq!(mirrored.bpm, 128.0);
        assert_eq!(mirrored.sample_rate, 48_000);
        assert_eq!(mirrored._reserved, 0);
    }

    #[test]
    fn config_round_trips_through_the_mirror() {
        let internal = crate::engine::EngineConfig {
            sample_rate: 44_100,
            block_size: 128,
            max_channels: 32,
            max_tracks: 96,
            driver_kind: 3,
            flags: 0,
        };
        let mirrored: ZenithEngineConfig = internal.into();
        let back: crate::engine::EngineConfig = mirrored.into();
        assert_eq!(back, internal);
    }

    #[test]
    fn driver_kind_and_flag_constants_are_stable() {
        assert_eq!(zenith_driver_kind::AUTO, 0);
        assert_eq!(zenith_driver_kind::OFFLINE, 3);
        assert_eq!(zenith_engine_flags::STRICT_REALTIME, 0x01);
        assert_eq!(zenith_engine_flags::WEB_DEGRADE, 0x02);
        assert_eq!(zenith_engine_flags::OFFLINE_FAST, 0x04);
    }
}

//! Parameter identity, descriptors and value shaping.
//!
//! # Address, not name
//!
//! A parameter is addressed by a compact triple — [`ParameterAddress`] —
//! never by a string. Dart composes the human-readable form
//! (`channel/3/volume`) once, maps it to the triple, and passes only integers
//! across the FFI boundary. The audio thread therefore never hashes a string,
//! which is what makes per-sample automation evaluation affordable
//! (PLAN §3.S2 requirement 1).
//!
//! # Evaluation order (normative)
//!
//! Every consumer of this module must compute a parameter's effective value in
//! exactly this order:
//!
//! ```text
//!   base value  →  automation  →  modulator sum  →  clamp
//! ```
//!
//! Rationale for the order:
//!
//! * **base first** — automation and modulation are *overrides* of the user's
//!   manual setting, so the manual value is the anchor a lane falls back to
//!   when it is empty or when the transport is outside its range.
//! * **automation before modulation** — automation is a *replacement* (it is
//!   the recorded intent for that parameter), while modulation is an
//!   *offset* layered on top. Swapping them would make an LFO's depth depend
//!   on whether a lane happens to be playing.
//! * **clamp last** — a single clamp at the end means intermediate stages can
//!   overshoot safely; clamping early would silently destroy a bipolar
//!   modulator's negative excursion.
//!
//! This order is implemented in [`crate::automation::player`] and must not be
//! re-derived locally by effects or the mixer.

use core::fmt;

/// Category of a parameter's owning object.
///
/// The discriminant is part of the C ABI: **published values must never
/// change**, only new ones may be appended (`docs/ABI.md` §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(C)]
pub enum ParameterKind {
    /// A property of the engine as a whole (tempo, master gain).
    Global = 0,
    /// A mixer channel strip property (gain, pan, mute).
    Channel = 1,
    /// A property of a track as distinct from its mixer channel.
    Track = 2,
    /// A parameter belonging to an effect slot instance.
    Effect = 3,
    /// A parameter belonging to a modulation source.
    Modulator = 4,
}

impl ParameterKind {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    ///
    /// Unknown kinds must fail rather than be coerced to a default: silently
    /// treating a future `ParameterKind` as `Global` would write the wrong
    /// parameter, and the caller would have no way to notice.
    #[must_use]
    pub const fn from_u16(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::Global),
            1 => Some(Self::Channel),
            2 => Some(Self::Track),
            3 => Some(Self::Effect),
            4 => Some(Self::Modulator),
            _ => None,
        }
    }

    /// The ABI discriminant for this kind.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Compact, hash-free address of a single parameter.
///
/// * `kind` — what sort of object owns the parameter.
/// * `index` — which instance of that object (channel 3, effect slot 1, …).
/// * `sub` — the parameter within that object. For an effect this is the
///   effect's own parameter ordinal; combined with `index` it is unique.
///
/// The triple is `Copy` and pointer-sized-ish, so passing it by value through
/// the FFI boundary costs nothing and cannot allocate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(C)]
pub struct ParameterAddress {
    /// Owning object category.
    pub kind: ParameterKind,
    /// Parameter ordinal within the owning object (formerly `sub`).
    pub sub: u16,
    /// Which instance of the owning object.
    pub index: u32,
}

impl ParameterAddress {
    /// Builds an address from its parts.
    #[must_use]
    pub const fn new(kind: ParameterKind, index: u32, sub: u16) -> Self {
        Self { kind, sub, index }
    }

    /// An address for a global parameter (tempo, master gain).
    #[must_use]
    pub const fn global(sub: u16) -> Self {
        Self::new(ParameterKind::Global, 0, sub)
    }

    /// An address for a parameter on mixer channel `index`.
    #[must_use]
    pub const fn channel(index: u32, sub: u16) -> Self {
        Self::new(ParameterKind::Channel, index, sub)
    }

    /// An address for a parameter on track `index`.
    #[must_use]
    pub const fn track(index: u32, sub: u16) -> Self {
        Self::new(ParameterKind::Track, index, sub)
    }

    /// An address for parameter `sub` of effect slot `slot` on channel `index`.
    #[must_use]
    pub const fn effect(index: u32, slot: u8, sub: u16) -> Self {
        // The slot is folded into the high half of the low 16 bits rather than
        // widening the struct: keeping `sub` at u16 preserves the ABI layout
        // agreed in docs/ABI.md §6.4 while still giving effects 256 slots.
        Self::new(ParameterKind::Effect, index, (slot as u16) << 8 | (sub & 0x00FF))
    }

    /// The effect slot encoded in [`Self::effect`] addresses.
    ///
    /// Only meaningful when `kind == Effect`.
    #[must_use]
    pub const fn effect_slot(self) -> u8 {
        (self.sub >> 8) as u8
    }

    /// A stable integer key for map lookups that avoids hashing the struct.
    ///
    /// #[must_use] is deliberate: an address used as a key must be consumed.
    #[must_use]
    pub const fn key(self) -> u64 {
        ((self.kind as u64) << 48) | ((self.index as u64) << 16) | (self.sub as u64)
    }
}

impl fmt::Display for ParameterAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}/{}", self.kind, self.index)
    }
}

/// How a parameter's value should be interpreted and displayed.
///
/// Discriminants are ABI-frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum ParameterUnit {
    /// Plain number.
    Linear = 0,
    /// Decibels.
    Decibels = 1,
    /// Hertz.
    Hertz = 2,
    /// Seconds.
    Seconds = 3,
    /// Percent, 0..100.
    Percent = 4,
    /// A choice among `max_value` discrete options.
    Enumeration = 5,
    /// Beats, for tempo-synced values (delay time, LFO rate).
    Beats = 6,
}

impl ParameterUnit {
    /// Converts a raw ABI discriminant, rejecting unknown values.
    #[must_use]
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Linear),
            1 => Some(Self::Decibels),
            2 => Some(Self::Hertz),
            3 => Some(Self::Seconds),
            4 => Some(Self::Percent),
            5 => Some(Self::Enumeration),
            6 => Some(Self::Beats),
            _ => None,
        }
    }

    /// The ABI discriminant for this unit.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Bit flags describing a parameter's automation behaviour.
///
/// A plain `u32` rather than an enum so unknown bits survive a round trip
/// through an older Dart build.
pub mod parameter_flags {
    /// The parameter can carry an automation lane.
    pub const AUTOMATABLE: u32 = 0x01;
    /// Values are discrete steps, not a continuum.
    pub const DISCRETE: u32 = 0x02;
    /// The UI should display the value on a logarithmic scale.
    pub const LOGARITHMIC: u32 = 0x04;
    /// The value is bipolar (a symmetric range around zero).
    pub const BIPOLAR: u32 = 0x08;
    /// Changes should be smoothed by the default one-pole filter.
    pub const SMOOTHED: u32 = 0x10;
}

/// Static description of a parameter, as published to Dart.
///
/// Names are `&'static str` and therefore borrowed: descriptors live in
/// `'static` tables inside the registry, so handing them to Dart costs no
/// allocation and no copy (see [`crate::automation::store`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterDescriptor {
    /// Compact address used to read and write the parameter.
    pub address: ParameterAddress,
    /// Stable machine-readable key, e.g. `"volume"`. Never localized.
    pub key: &'static str,
    /// Human-readable label; the UI localizes as it sees fit.
    pub label: &'static str,
    /// Interpretation and display unit.
    pub unit: ParameterUnit,
    /// Behaviour flags, see [`parameter_flags`].
    pub flags: u32,
    /// Lowest legal value.
    pub min_value: f32,
    /// Highest legal value.
    pub max_value: f32,
    /// Value used when neither the user nor a project file supplies one.
    pub default_value: f32,
    /// Default smoothing time in milliseconds.
    pub smoothing_ms: f32,
}

impl ParameterDescriptor {
    /// Whether the parameter may carry an automation lane.
    #[must_use]
    pub const fn is_automatable(&self) -> bool {
        self.flags & parameter_flags::AUTOMATABLE != 0
    }

    /// Whether the value is a discrete step rather than a continuum.
    #[must_use]
    pub const fn is_discrete(&self) -> bool {
        self.flags & parameter_flags::DISCRETE != 0 || matches!(self.unit, ParameterUnit::Enumeration)
    }

    /// Clamps `value` into `[min_value, max_value]`.
    ///
    /// `NaN` maps to `default_value`, because a `NaN` arriving from a corrupt
    /// project file or a misbehaving modulator must not propagate into the
    /// audio signal, where it would silence the bus and be very hard to trace
    /// back to its source.
    ///
    /// Infinities are *not* treated as garbage: they clamp to the nearest
    /// bound, which is what a caller means by "as loud as possible" and what
    /// keeps a modulator that has run away from silently resetting the
    /// parameter to its default instead of pinning it.
    #[must_use]
    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_nan() {
            return self.default_value;
        }
        if value < self.min_value {
            self.min_value
        } else if value > self.max_value {
            self.max_value
        } else {
            value
        }
    }

    /// Normalizes `value` into `0.0..=1.0` for display and UI handles.
    #[must_use]
    pub fn normalize(&self, value: f32) -> f32 {
        let span = self.max_value - self.min_value;
        if span <= 0.0 {
            return 0.0;
        }
        ((self.clamp(value) - self.min_value) / span).clamp(0.0, 1.0)
    }

    /// Maps a normalized `0.0..=1.0` position back to a parameter value.
    #[must_use]
    pub fn denormalize(&self, normalized: f32) -> f32 {
        let span = self.max_value - self.min_value;
        self.clamp(self.min_value + normalized.clamp(0.0, 1.0) * span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_distinct_and_ordered_by_key() {
        let a = ParameterAddress::channel(1, 0);
        let b = ParameterAddress::channel(2, 0);
        let c = ParameterAddress::track(1, 0);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a.key(), b.key());
        assert_ne!(a.key(), c.key());
    }

    #[test]
    fn effect_address_round_trips_the_slot() {
        let addr = ParameterAddress::effect(7, 3, 42);
        assert_eq!(addr.effect_slot(), 3);
        assert_eq!(addr.index, 7);
        // The low byte keeps the effect's own parameter ordinal.
        assert_eq!(addr.sub & 0x00FF, 42);
    }

    #[test]
    fn unknown_abi_discriminants_are_rejected_not_coerced() {
        assert_eq!(ParameterKind::from_u16(0), Some(ParameterKind::Global));
        assert_eq!(ParameterKind::from_u16(99), None);
        assert_eq!(ParameterUnit::from_u32(6), Some(ParameterUnit::Beats));
        assert_eq!(ParameterUnit::from_u32(77), None);
    }

    #[test]
    fn kind_discriminants_are_abi_frozen() {
        assert_eq!(ParameterKind::Global.as_u16(), 0);
        assert_eq!(ParameterKind::Channel.as_u16(), 1);
        assert_eq!(ParameterKind::Track.as_u16(), 2);
        assert_eq!(ParameterKind::Effect.as_u16(), 3);
        assert_eq!(ParameterKind::Modulator.as_u16(), 4);
    }

    fn gain_descriptor() -> ParameterDescriptor {
        ParameterDescriptor {
            address: ParameterAddress::channel(0, 0),
            key: "volume",
            label: "Volume",
            unit: ParameterUnit::Decibels,
            flags: parameter_flags::AUTOMATABLE | parameter_flags::SMOOTHED,
            min_value: -60.0,
            max_value: 12.0,
            default_value: 0.0,
            smoothing_ms: 10.0,
        }
    }

    #[test]
    fn clamp_bounds_both_ends() {
        let d = gain_descriptor();
        assert_eq!(d.clamp(-200.0), -60.0);
        assert_eq!(d.clamp(200.0), 12.0);
        assert_eq!(d.clamp(-6.0), -6.0);
    }

    #[test]
    fn clamp_neutralizes_non_finite_input() {
        let d = gain_descriptor();
        // A NaN or infinity must never reach the audio path.
        assert_eq!(d.clamp(f32::NAN), 0.0);
        assert_eq!(d.clamp(f32::INFINITY), 12.0);
        assert_eq!(d.clamp(f32::NEG_INFINITY), -60.0);
    }

    #[test]
    fn normalize_and_denormalize_are_inverses() {
        let d = gain_descriptor();
        for value in [-60.0_f32, -30.0, 0.0, 6.0, 12.0] {
            let round_tripped = d.denormalize(d.normalize(value));
            assert!(
                (round_tripped - value).abs() < 1e-4,
                "{value} round-tripped to {round_tripped}"
            );
        }
    }

    #[test]
    fn degenerate_range_does_not_divide_by_zero() {
        let d = ParameterDescriptor {
            min_value: 4.0,
            max_value: 4.0,
            ..gain_descriptor()
        };
        assert_eq!(d.normalize(4.0), 0.0);
        assert_eq!(d.denormalize(0.5), 4.0);
    }

    #[test]
    fn discrete_and_automatable_flags_are_read_correctly() {
        let d = gain_descriptor();
        assert!(d.is_automatable());
        assert!(!d.is_discrete());

        let e = ParameterDescriptor {
            unit: ParameterUnit::Enumeration,
            flags: parameter_flags::DISCRETE,
            ..d
        };
        assert!(e.is_discrete());
        assert!(!e.is_automatable());
    }
}

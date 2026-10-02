//! The built-in effect registry: kind ids, factories and descriptors.
//!
//! # Kind id space
//!
//! `docs/ABI.md` §11 Q2 fixes the split: **built-in kinds occupy
//! `0x0000_0000..=0x0000_FFFF`** and plugin kinds `0x0001_0000+`. The mixer
//! stores a kind as a bare `u32` in an effect slot (`mixer::effect_chain`), so
//! the two never collide and an unknown kind is simply bypassed.
//!
//! # Why a table and not a `HashMap`
//!
//! The registry is consulted from the control thread when a slot is populated,
//! and from Dart through `zenith_effect_describe`. A sorted static array keeps
//! the whole registry a compile-time constant: no lazy initialisation, no
//! allocation, and the exported name list is auditable by reading this file.
//!
//! # Factories, not instances
//!
//! `create` builds a processor for a given slot address. Descriptors are
//! per-instance only because their parameter addresses embed the slot; the
//! rest of the description is genuinely static.

use super::buffer::AudioBuffer;
use super::{
    delay, dynamics, distortion, eq, filter, modulation, reverb, util, EffectDescriptor,
    EffectProcessor,
};
use crate::automation::parameter::ParameterAddress;

// ── Built-in kind ids ──
//
// These values are ABI-frozen once published: a project file stores the kind
// id of every effect in every slot, so renumbering one would silently load a
// different effect into the user's project. New effects take the next free
// number in the category block and are appended — never inserted.

/// Parametric equaliser.
pub const KIND_EQ_PARAMETRIC: u32 = 0x0000_0100;
/// Spectrum analyser (pass-through, publishes analysis data).
pub const KIND_EQ_SPECTRUM: u32 = 0x0000_0101;

/// Compressor.
pub const KIND_COMPRESSOR: u32 = 0x0000_0200;
/// Brick-wall limiter.
pub const KIND_LIMITER: u32 = 0x0000_0201;
/// Noise gate.
pub const KIND_GATE: u32 = 0x0000_0202;

/// Algorithmic reverb.
pub const KIND_REVERB_ALGORITHMIC: u32 = 0x0000_0300;
/// Convolution reverb.
pub const KIND_REVERB_CONVOLUTION: u32 = 0x0000_0301;

/// Tempo-synced delay.
pub const KIND_DELAY_SYNC: u32 = 0x0000_0400;

/// Chorus.
pub const KIND_CHORUS: u32 = 0x0000_0500;
/// Flanger.
pub const KIND_FLANGER: u32 = 0x0000_0501;
/// Phaser.
pub const KIND_PHASER: u32 = 0x0000_0502;

/// Saturation.
pub const KIND_SATURATION: u32 = 0x0000_0600;
/// Bit crusher.
pub const KIND_BITCRUSH: u32 = 0x0000_0601;

/// Multimode filter.
pub const KIND_FILTER_MULTIMODE: u32 = 0x0000_0700;

/// The lowest built-in kind id.
pub const KIND_BUILTIN_BASE: u32 = 0x0000_0000;
/// One past the highest built-in kind id.
pub const KIND_BUILTIN_END: u32 = 0x0000_FFFF;
/// The lowest plugin kind id (ABI §11 Q2).
pub const KIND_PLUGIN_BASE: u32 = 0x0001_0000;

/// How to build one effect.
#[derive(Clone, Copy)]
pub struct EffectEntry {
    /// Static description.
    pub descriptor: &'static EffectDescriptor,
    /// Factory, given the slot's automation address.
    pub factory: fn(ParameterAddress) -> alloc::boxed::Box<dyn EffectProcessor>,
}

/// Whether `kind` belongs to the built-in range.
///
/// `KIND_BUILTIN_BASE` is zero, so `kind >= KIND_BUILTIN_BASE` is trivially
/// true and only the upper bound can reject anything. It is written as a single
/// comparison rather than a redundant `>=` the compiler rejects as absurd.
#[must_use]
pub const fn is_builtin_kind(kind: u32) -> bool {
    kind <= KIND_BUILTIN_END
}

/// The complete built-in registry, in kind-id order.
///
/// Order matters for two things: `zenith_effect_kind_at` indexes this table,
/// and the binary search below requires it to be sorted. A test enforces both.
pub static ENTRIES: &[EffectEntry] = &[
    EffectEntry {
        descriptor: &eq::parametric::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(eq::parametric::ParametricEq::new(address)),
    },
    EffectEntry {
        descriptor: &eq::spectrum::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(eq::spectrum::SpectrumAnalyser::new(address)),
    },
    EffectEntry {
        descriptor: &dynamics::compressor::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(dynamics::compressor::Compressor::new(address)),
    },
    EffectEntry {
        descriptor: &dynamics::limiter::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(dynamics::limiter::Limiter::new(address)),
    },
    EffectEntry {
        descriptor: &dynamics::gate::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(dynamics::gate::Gate::new(address)),
    },
    EffectEntry {
        descriptor: &reverb::algorithmic::DESCRIPTOR,
        factory: |address| {
            alloc::boxed::Box::new(reverb::algorithmic::AlgorithmicReverb::new(address))
        },
    },
    EffectEntry {
        descriptor: &reverb::convolution::DESCRIPTOR,
        factory: |address| {
            alloc::boxed::Box::new(reverb::convolution::ConvolutionReverb::new(address))
        },
    },
    EffectEntry {
        descriptor: &delay::sync_delay::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(delay::sync_delay::SyncDelay::new(address)),
    },
    EffectEntry {
        descriptor: &modulation::chorus::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(modulation::chorus::Chorus::new(address)),
    },
    EffectEntry {
        descriptor: &modulation::flanger::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(modulation::flanger::Flanger::new(address)),
    },
    EffectEntry {
        descriptor: &modulation::phaser::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(modulation::phaser::Phaser::new(address)),
    },
    EffectEntry {
        descriptor: &distortion::saturation::DESCRIPTOR,
        factory: |address| {
            alloc::boxed::Box::new(distortion::saturation::Saturation::new(address))
        },
    },
    EffectEntry {
        descriptor: &distortion::bitcrush::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(distortion::bitcrush::BitCrusher::new(address)),
    },
    EffectEntry {
        descriptor: &filter::multimode::DESCRIPTOR,
        factory: |address| alloc::boxed::Box::new(filter::multimode::MultimodeFilter::new(address)),
    },
];

/// How many built-in effects there are.
#[must_use]
pub fn count() -> usize {
    ENTRIES.len()
}

/// The entry for `kind`, if built in.
#[must_use]
pub fn find(kind: u32) -> Option<&'static EffectEntry> {
    ENTRIES
        .binary_search_by_key(&kind, |entry| entry.descriptor.kind)
        .ok()
        .and_then(|index| ENTRIES.get(index))
}

/// The entry at table index `index`.
#[must_use]
pub fn at(index: usize) -> Option<&'static EffectEntry> {
    ENTRIES.get(index)
}

/// The descriptor for `kind`, if built in.
#[must_use]
pub fn describe(kind: u32) -> Option<&'static EffectDescriptor> {
    find(kind).map(|entry| entry.descriptor)
}

/// Creates a processor for `kind` at `address`.
///
/// Returns `None` for an unknown kind rather than substituting a default
/// effect: silently loading a different effect into a slot the project asked
/// for would corrupt the user's session on save.
#[must_use]
pub fn create(
    kind: u32,
    address: ParameterAddress,
) -> Option<alloc::boxed::Box<dyn EffectProcessor>> {
    find(kind).map(|entry| (entry.factory)(address))
}

/// Whether `kind` is a built-in effect this build knows about.
#[must_use]
pub fn is_known(kind: u32) -> bool {
    find(kind).is_some()
}

/// Processes one block through a created processor.
///
/// A free function rather than a method so the engine can hold processors as
/// trait objects without naming the concrete type.
pub fn process_block(
    processor: &mut dyn EffectProcessor,
    buffer: &mut AudioBuffer<'_>,
    ctx: &super::buffer::RenderContext,
) {
    processor.process(buffer, ctx);
}

/// The oversampling factor a kind uses by default, for diagnostics.
///
/// Reported through `zenith_effect_describe` so the UI can show a quality
/// badge. Kept here rather than on each effect because it is a property of the
/// suite's design budget, not of any one processor.
#[must_use]
pub const fn default_oversampling(kind: u32) -> u32 {
    match kind {
        KIND_SATURATION | KIND_BITCRUSH => util::OversamplingFactor::X4 as u32,
        KIND_FILTER_MULTIMODE | KIND_COMPRESSOR => util::OversamplingFactor::X2 as u32,
        _ => util::OversamplingFactor::None as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_sorted_by_kind_id() {
        // Binary search silently returns wrong answers on an unsorted table,
        // which would make `zenith_effect_describe` report the wrong effect.
        for window in ENTRIES.windows(2) {
            assert!(
                window[0].descriptor.kind < window[1].descriptor.kind,
                "registry is not sorted: {:#x} then {:#x}",
                window[0].descriptor.kind,
                window[1].descriptor.kind
            );
        }
    }

    #[test]
    fn no_two_effects_share_a_kind_id() {
        for (i, a) in ENTRIES.iter().enumerate() {
            for b in &ENTRIES[i + 1..] {
                assert_ne!(
                    a.descriptor.kind, b.descriptor.kind,
                    "duplicate kind id {:#x}",
                    a.descriptor.kind
                );
            }
        }
    }

    #[test]
    fn no_two_effects_share_a_key() {
        // The key travels to Dart and is used as a stable identifier there.
        for (i, a) in ENTRIES.iter().enumerate() {
            for b in &ENTRIES[i + 1..] {
                assert_ne!(
                    a.descriptor.key, b.descriptor.key,
                    "duplicate effect key {}",
                    a.descriptor.key
                );
            }
        }
    }

    #[test]
    fn every_kind_is_inside_the_builtin_range() {
        for entry in ENTRIES {
            assert!(
                is_builtin_kind(entry.descriptor.kind),
                "{:#x} is outside the built-in range",
                entry.descriptor.kind
            );
        }
    }

    #[test]
    fn the_plugin_range_never_overlaps_the_builtin_range() {
        // ABI §11 Q2; a slot holding a plugin must not resolve to a built-in.
        // Compile-time constants, so the relation is checked at compile time
        // rather than re-proved in a test the compiler can fold away.
        const { assert!(KIND_BUILTIN_END < KIND_PLUGIN_BASE) };
        assert!(is_builtin_kind(KIND_BUILTIN_END));
        assert!(!is_builtin_kind(KIND_PLUGIN_BASE));
        assert!(!is_builtin_kind(KIND_PLUGIN_BASE + 0xFFFF));
    }

    #[test]
    fn every_kind_is_reachable_by_find_and_at() {
        for (index, entry) in ENTRIES.iter().enumerate() {
            let kind = entry.descriptor.kind;
            assert!(is_known(kind));
            assert_eq!(find(kind).map(|e| e.descriptor.kind), Some(kind));
            assert_eq!(
                at(index).map(|e| e.descriptor.kind),
                Some(kind),
                "index {index} does not round trip"
            );
            assert_eq!(describe(kind).map(|d| d.key), Some(entry.descriptor.key));
        }
        assert!(at(ENTRIES.len()).is_none());
    }

    #[test]
    fn unknown_kinds_are_reported_as_unknown() {
        assert!(!is_known(0xDEAD_BEEF));
        assert!(find(0xDEAD_BEEF).is_none());
        assert!(describe(0xDEAD_BEEF).is_none());
        assert!(create(0xDEAD_BEEF, ParameterAddress::effect(0, 0, 0)).is_none());
    }

    #[test]
    fn every_registered_effect_can_be_created_and_prepared() {
        // Catches a factory that points at the wrong type and an effect whose
        // `prepare` panics on a realistic block size.
        for entry in ENTRIES {
            let address = ParameterAddress::effect(0, 0, 0);
            let kind = entry.descriptor.kind;
            let mut processor =
                create(kind, address).unwrap_or_else(|| panic!("{kind:#x} did not build"));

            assert_eq!(
                processor.descriptor().kind,
                kind,
                "{kind:#x} built a processor describing a different kind"
            );
            processor.prepare(48_000.0, 256, 2);

            let parameters = processor.parameters();
            assert_eq!(
                parameters.len(),
                entry.descriptor.param_count as usize,
                "{kind:#x} publishes {} parameters but its descriptor claims {}",
                parameters.len(),
                entry.descriptor.param_count
            );
        }
    }

    #[test]
    fn every_registered_effect_processes_a_block_without_panicking() {
        for entry in ENTRIES {
            let kind = entry.descriptor.kind;
            let address = ParameterAddress::effect(0, 0, 0);
            let mut processor = create(kind, address).expect("registered");
            processor.prepare(48_000.0, 256, 2);

            let mut left = alloc::vec![0.3_f32; 256];
            let mut right = alloc::vec![-0.3_f32; 256];
            {
                let mut views = [&mut left[..], &mut right[..]];
                let mut buffer = AudioBuffer::new(&mut views);
                let ctx = crate::effects::buffer::RenderContext::new(48_000.0, 256, 0, 120.0, 960);
                processor.process(&mut buffer, &ctx);
            }
            for (i, sample) in left.iter().enumerate() {
                assert!(
                    sample.is_finite(),
                    "{kind:#x} produced a non-finite sample at {i}: {sample}"
                );
            }
        }
    }

    #[test]
    fn the_descriptor_counts_agree_with_the_parameter_tables() {
        // The descriptor's `param_count` is what Dart iterates; if it were
        // larger than the table, the UI would read past the end of it.
        for entry in ENTRIES {
            let address = ParameterAddress::effect(3, 1, 0);
            let processor = create(entry.descriptor.kind, address).expect("registered");
            assert_eq!(
                processor.parameters().len(),
                entry.descriptor.param_count as usize,
                "{} disagrees on its parameter count",
                entry.descriptor.key
            );
        }
    }

    #[test]
    fn parameter_ordinals_are_contiguous_within_an_effect() {
        // `first_param..first_param + param_count` is what the ABI publishes;
        // the table must actually occupy that range.
        for entry in ENTRIES {
            let address = ParameterAddress::effect(0, 0, 0);
            let processor = create(entry.descriptor.kind, address).expect("registered");
            let range = entry.descriptor.param_range();
            for (offset, spec) in processor.parameters().iter().enumerate() {
                let ordinal = range.start + offset as u16;
                assert!(
                    range.contains(&ordinal),
                    "{} publishes an ordinal outside {range:?}",
                    entry.descriptor.key
                );
                assert_eq!(
                    spec.address.sub & 0x00FF,
                    ordinal,
                    "{} parameter {offset} has a mismatched address",
                    entry.descriptor.key
                );
            }
        }
    }

    #[test]
    fn parameter_keys_are_unique_within_each_effect() {
        // Dart keys its UI widgets by parameter key; a duplicate would make
        // two knobs share one field.
        for entry in ENTRIES {
            let processor = create(entry.descriptor.kind, ParameterAddress::effect(0, 0, 0))
                .expect("registered");
            let table = processor.parameters();
            for (i, a) in table.iter().enumerate() {
                for b in &table[i + 1..] {
                    assert_ne!(
                        a.key, b.key,
                        "{} has two parameters keyed {}",
                        entry.descriptor.key, a.key
                    );
                }
            }
        }
    }

    #[test]
    fn every_effect_declares_a_sane_category_and_key() {
        for entry in ENTRIES {
            let d = entry.descriptor;
            assert!(!d.key.is_empty(), "empty key for {:#x}", d.kind);
            assert!(!d.label.is_empty(), "empty label for {:#x}", d.kind);
            assert!(
                d.key.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "key {} is not a stable machine key",
                d.key
            );
            assert!(d.param_count > 0, "{} publishes no parameters", d.key);
        }
    }

    #[test]
    fn default_oversampling_is_a_valid_factor() {
        for entry in ENTRIES {
            let factor = default_oversampling(entry.descriptor.kind);
            assert!(
                util::OversamplingFactor::from_u32(factor).is_some(),
                "{:#x} declares invalid oversampling {factor}",
                entry.descriptor.kind
            );
        }
    }
}

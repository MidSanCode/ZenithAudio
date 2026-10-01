//! Trimmed registry for the isolated dynamics verification harness.

use super::buffer::AudioBuffer;
use super::dynamics;
use super::util;
use super::{EffectDescriptor, EffectProcessor};
use crate::automation::parameter::ParameterAddress;

/// Compressor.
pub const KIND_COMPRESSOR: u32 = 0x0000_0200;
/// Brick-wall limiter.
pub const KIND_LIMITER: u32 = 0x0000_0201;
/// Noise gate.
pub const KIND_GATE: u32 = 0x0000_0202;

/// The lowest built-in kind id.
pub const KIND_BUILTIN_BASE: u32 = 0x0000_0000;
/// One past the highest built-in kind id.
pub const KIND_BUILTIN_END: u32 = 0x0000_FFFF;
/// The lowest plugin kind id.
pub const KIND_PLUGIN_BASE: u32 = 0x0001_0000;

/// How to build one effect.
#[derive(Clone, Copy)]
pub struct EffectEntry {
    /// Static description.
    pub descriptor: &'static EffectDescriptor,
    /// Factory.
    pub factory: fn(ParameterAddress) -> alloc::boxed::Box<dyn EffectProcessor>,
}

/// Whether `kind` belongs to the built-in range.
#[must_use]
pub const fn is_builtin_kind(kind: u32) -> bool {
    kind >= KIND_BUILTIN_BASE && kind <= KIND_BUILTIN_END
}

/// The registry.
pub static ENTRIES: &[EffectEntry] = &[
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
pub fn process_block(
    processor: &mut dyn EffectProcessor,
    buffer: &mut AudioBuffer<'_>,
    ctx: &super::buffer::RenderContext,
) {
    processor.process(buffer, ctx);
}

/// The oversampling factor a kind uses by default.
#[must_use]
pub const fn default_oversampling(kind: u32) -> u32 {
    match kind {
        KIND_COMPRESSOR => util::OversamplingFactor::X2 as u32,
        _ => util::OversamplingFactor::None as u32,
    }
}

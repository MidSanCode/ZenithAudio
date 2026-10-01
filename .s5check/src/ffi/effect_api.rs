//! C ABI surface for the built-in effect suite (ABI §6.5, S5).
//!
//! # What this module is for
//!
//! `docs/ABI.md` §6.4 records an explicit debt: `zenith_effect_describe_params`
//! was **not** delivered by S2 because it needs an effect-slot model. S5 pays
//! that debt here.
//!
//! The payoff is PLAN §3.S5's "UI 自动生成": Dart builds an effect panel by
//! asking this module what effects exist, what each one's parameters are, and
//! what ranges and units they use. **No hand-written Dart class per effect** —
//! adding a new effect to `effects/registry.rs` makes it appear in the UI with
//! no Dart change at all.
//!
//! # Why there is no handle
//!
//! Unlike S2's `zenith_automation_*` and S3's `zenith_mixer_*`, these functions
//! take **no handle**. The built-in effect registry is a compile-time constant:
//! it does not depend on the engine, a sample rate, or any instance. Making the
//! caller create a handle first would imply state that does not exist.
//!
//! The one function that *is* instance-dependent,
//! [`zenith_effect_latency_samples`], therefore takes the sample rate and block
//! size explicitly rather than reading them from an engine.
//!
//! # Static strings
//!
//! Descriptions contain `*const c_char` pointing at `'static` Rust literals.
//! They stay valid for the lifetime of the process and Dart **must not** free
//! them (ABI §3.3). This is the "Rust → Dart (borrow)" row of that table.
//!
//! # Real-time safety
//!
//! Every function here is **control thread only** and all of them are
//! allocation-free reads of a static table. None of them touch audio state, so
//! none of them are dangerous to call while the transport runs — but none of
//! them belong in an audio callback either.

// See `param_api.rs` for why this lint is allowed: the exported functions keep
// C signatures, validate every pointer, and document safety per function.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_char;
use core::mem::size_of;

use crate::effects::registry;
use crate::effects::EffectCategory;
use crate::ffi::types::{
    name_ptr, zenith_effect_category, zenith_effect_kind_range, ZenithEffectDescriptor,
    ZenithParamDescriptor,
};
use crate::guard;
use crate::Status;

/// Borrows a `*const T` as `&T`, or returns [`Status::NullPointer`].
///
/// # Safety
///
/// `ptr` must be null or point to a valid, aligned `T` that outlives the call.
unsafe fn as_ref<'a, T>(ptr: *const T) -> Result<&'a T, Status> {
    // SAFETY: the caller upholds the validity invariant.
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

/// Builds the ABI description of a registry entry.
fn describe_entry(entry: &'static registry::EffectEntry) -> ZenithEffectDescriptor {
    let d = entry.descriptor;
    ZenithEffectDescriptor {
        kind: d.kind,
        category: d.category.as_u32(),
        param_count: u32::from(d.param_count),
        first_param: u32::from(d.first_param),
        has_latency: u32::from(d.has_latency),
        is_analysis_only: u32::from(d.is_analysis_only),
        // Interning is shared with the parameter surface, so a name published
        // by both paths has one buffer and one pointer.
        key_utf8: name_ptr(d.key),
        label_utf8: name_ptr(d.label),
    }
}

// ── Enumeration ──

/// Returns how many built-in effects this build knows about.
///
/// Together with [`zenith_effect_kind_at`] this is the "two-call protocol":
/// ask for the count, allocate that many slots in Dart, then fetch each kind.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_count() -> u32 {
    u32::try_from(registry::count()).unwrap_or(u32::MAX)
}

/// Writes the kind id at table index `index` to `out_kind`.
///
/// Returns [`Status::OutOfRange`] when `index` is past the end, so a caller
/// that raced a library reload cannot read unrelated memory, and
/// [`Status::NullPointer`] when `out_kind` is null. On failure `out_kind` is
/// left untouched (ABI §4.1).
///
/// # Safety
///
/// `out_kind` must be null or point to a writable `u32`.
#[no_mangle]
pub extern "C" fn zenith_effect_kind_at(index: u32, out_kind: *mut u32) -> Status {
    guard(|| {
        let entry = registry::at(index as usize).ok_or(Status::OutOfRange)?;
        unsafe { write_out(out_kind, entry.descriptor.kind) }?;
        Ok(Status::Ok)
    })
}

/// Whether `kind` is a built-in effect this build can instantiate.
///
/// Returns `1` for yes and `0` for no. A caller must check this before storing
/// a kind into a slot: the mixer stores a bare `u32` and silently bypasses an
/// unknown kind, so a project referencing an effect from a newer build would
/// otherwise load with a quietly empty slot.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_is_known(kind: u32) -> u32 {
    u32::from(registry::is_known(kind))
}

/// Writes the description of `kind` to `out_descriptor`.
///
/// Returns [`Status::NotFound`] for an unknown kind rather than substituting a
/// default, because a default would describe a *different* effect and the
/// caller would have no way to notice.
///
/// The pointer fields in the written descriptor are `'static` and must not be
/// freed by the caller (ABI §3.3).
///
/// # Safety
///
/// `out_descriptor` must be null or point to a writable [`ZenithEffectDescriptor`].
#[no_mangle]
pub extern "C" fn zenith_effect_describe(
    kind: u32,
    out_descriptor: *mut ZenithEffectDescriptor,
) -> Status {
    guard(|| {
        let entry = registry::find(kind).ok_or(Status::NotFound)?;
        unsafe { write_out(out_descriptor, describe_entry(entry)) }?;
        Ok(Status::Ok)
    })
}

/// Writes the description of the effect at table `index` to `out_descriptor`.
///
/// The index-based twin of [`zenith_effect_describe`], so a caller walking the
/// registry does not have to fetch the kind first.
///
/// # Safety
///
/// `out_descriptor` must be null or point to a writable [`ZenithEffectDescriptor`].
#[no_mangle]
pub extern "C" fn zenith_effect_describe_at(
    index: u32,
    out_descriptor: *mut ZenithEffectDescriptor,
) -> Status {
    guard(|| {
        let entry = registry::at(index as usize).ok_or(Status::OutOfRange)?;
        unsafe { write_out(out_descriptor, describe_entry(entry)) }?;
        Ok(Status::Ok)
    })
}

/// Returns the name of the effect `kind` as a static C string, or null when
/// `kind` is unknown.
///
/// The pointer is `'static`; the caller must read it and must **not** free it
/// (ABI §3.3). `null` here means "no such effect" — it is the one place in this
/// module where a null return is the documented contract, which is why the
/// string-returning functions are separated from the status-returning ones.
///
/// # Safety
///
/// The returned pointer must only be read, never written or freed.
#[no_mangle]
pub extern "C" fn zenith_effect_name(kind: u32) -> *const c_char {
    match registry::find(kind) {
        Some(entry) => name_ptr(entry.descriptor.label),
        None => core::ptr::null(),
    }
}

/// Returns the stable machine key of the effect `kind`, or null when unknown.
///
/// Unlike [`zenith_effect_name`] this is never localized, so it is safe to use
/// as a persistence key and as the identity of a Dart widget.
///
/// # Safety
///
/// The returned pointer must only be read, never written or freed.
#[no_mangle]
pub extern "C" fn zenith_effect_key(kind: u32) -> *const c_char {
    match registry::find(kind) {
        Some(entry) => name_ptr(entry.descriptor.key),
        None => core::ptr::null(),
    }
}

/// Returns the category discriminant of the effect `kind`, or
/// `ZENITH_EFFECT_CATEGORY_UTILITY` when `kind` is unknown.
///
/// Unknown kinds report the utility category rather than a sentinel so the UI
/// can still render a generic panel for an effect from a newer build instead of
/// dropping it from the list.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_category(kind: u32) -> u32 {
    registry::find(kind).map_or(zenith_effect_category::UTILITY, |entry| {
        entry.descriptor.category.as_u32()
    })
}

// ── Parameters ──

/// Returns how many parameters the effect `kind` publishes.
///
/// Returns `0` for an unknown kind. A caller iterates `0..count` and passes
/// each ordinal to [`zenith_effect_describe_parameter`] on a live instance.
///
/// Note the distinction this API makes deliberately: the *count* comes from the
/// static registry, while the *descriptors* come from an instance, because a
/// descriptor carries an automation address that depends on which slot the
/// effect occupies (see `EffectProcessor::parameters`).
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_parameter_count(kind: u32) -> u32 {
    registry::describe(kind).map_or(0, |d| u32::from(d.param_count))
}

/// Returns how many parameters an effect instance publishes.
///
/// `processor` is a `*mut dyn EffectProcessor` created by the engine. Passing a
/// `dyn` trait object across the boundary keeps the concrete type out of the
/// ABI, which is what lets Dart drive an effect it has never heard of.
///
/// Returns `0` when `processor` is null.
///
/// # Safety
///
/// `processor` must be null or a live `*mut dyn EffectProcessor`.
#[no_mangle]
pub extern "C" fn zenith_effect_instance_parameter_count(
    processor: *const dyn crate::effects::EffectProcessor,
) -> u32 {
    // SAFETY: the caller guarantees the pointer is live, or null.
    let Some(processor) = (unsafe { processor.as_ref() }) else {
        return 0;
    };
    u32::try_from(processor.parameters().len()).unwrap_or(u32::MAX)
}

/// Describes parameter `ordinal` of a live effect instance.
///
/// Writes into `out_descriptor` a [`crate::ffi::types::ZenithParamDescriptor`]
/// — the **same** mirror S2 already publishes — so Dart has exactly one
/// parameter-descriptor type to decode, whether the parameter belongs to a
/// channel or to an effect.
///
/// Returns [`Status::OutOfRange`] past the end of the table and
/// [`Status::NullPointer`] for a null argument.
///
/// # Safety
///
/// `processor` must be null or a live `*const dyn EffectProcessor`;
/// `out_descriptor` must be null or point to a writable
/// [`crate::ffi::types::ZenithParamDescriptor`].
#[no_mangle]
pub extern "C" fn zenith_effect_instance_describe_parameter(
    processor: *const dyn crate::effects::EffectProcessor,
    ordinal: u32,
    out_descriptor: *mut crate::ffi::types::ZenithParamDescriptor,
) -> Status {
    guard(|| {
        // SAFETY: the caller guarantees the pointer is live, or null.
        let processor = unsafe { processor.as_ref() }.ok_or(Status::NullPointer)?;
        let table = processor.parameters();
        let spec = table.get(ordinal as usize).ok_or(Status::OutOfRange)?;
        let mirror = ZenithParamDescriptor::from(*spec);
        unsafe { write_out(out_descriptor, mirror) }?;
        Ok(Status::Ok)
    })
}

// ── Latency ──

/// Instantiates `kind` at `sample_rate` and reports its latency in samples.
///
/// This is the authoritative PDC query (PLAN §3.S4 requirement 1). It exists
/// because latency is not a constant: a saturation stage that is oversampling
/// carries a filter delay while the same stage at 0 dB drive does not, and a
/// limiter's look-ahead is a parameter.
///
/// The effect is created, prepared and then dropped; no state is retained. That
/// makes the call safe from Dart without an engine handle, at the cost of one
/// allocation per call — which is why it is a **control-thread** query, called
/// when a slot changes rather than per block.
///
/// Returns [`Status::NotFound`] for an unknown kind.
///
/// # Safety
///
/// `out_samples` must be null or point to a writable `u32`.
#[no_mangle]
pub extern "C" fn zenith_effect_latency_samples(
    kind: u32,
    sample_rate: u32,
    max_block: u32,
    channels: u32,
    out_samples: *mut u32,
) -> Status {
    guard(|| {
        let entry = registry::find(kind).ok_or(Status::NotFound)?;
        let address = crate::automation::parameter::ParameterAddress::effect(0, 0, 0);
        let mut processor = (entry.factory)(address);

        let rate = if sample_rate == 0 {
            48_000.0
        } else {
            sample_rate as f32
        };
        let block = if max_block == 0 {
            256
        } else {
            max_block as usize
        };
        let channel_count = if channels == 0 {
            2
        } else {
            channels as usize
        };
        processor.prepare(rate, block, channel_count);

        let latency = u32::try_from(processor.latency_samples()).unwrap_or(u32::MAX);
        unsafe { write_out(out_samples, latency) }?;
        Ok(Status::Ok)
    })
}

/// The oversampling factor `kind` uses internally, for a UI quality badge.
///
/// Returns `1` for an effect that does not oversample. Kept separate from the
/// latency query because it does not require instantiating anything.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_oversampling(kind: u32) -> u32 {
    registry::default_oversampling(kind)
}

// ── Kind-range self-check ──

/// Reports whether `kind` falls in the built-in range, regardless of whether
/// this build actually implements it.
///
/// Distinct from [`zenith_effect_is_known`]: this answers "whose namespace is
/// this?" and is what a loader uses to decide whether to look for a plugin.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_is_builtin_kind(kind: u32) -> u32 {
    u32::from(registry::is_builtin_kind(kind))
}

/// Returns the size of [`ZenithEffectDescriptor`] as Rust laid it out.
///
/// Dart compares this with its own `sizeOf` at load time and throws on a
/// mismatch (ABI §2.3).
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_sizeof_effect_descriptor_checked() -> usize {
    size_of::<ZenithEffectDescriptor>()
}

/// Returns `ZENITH_EFFECT_PLUGIN_BASE`, so Dart never hard-codes it.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_plugin_kind_base() -> u32 {
    zenith_effect_kind_range::PLUGIN_BASE
}

/// Returns `ZENITH_EFFECT_CATEGORY_ANALYSIS`.
///
/// # Safety
///
/// No preconditions; the function reads no memory.
#[no_mangle]
pub extern "C" fn zenith_effect_category_analysis() -> u32 {
    zenith_effect_category::ANALYSIS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::types::ZenithParamDescriptor;
    use crate::Status;
    use core::ffi::CStr;

    /// Reads a static C string the way Dart would.
    ///
    /// # Safety
    ///
    /// `ptr` must be null or a valid NUL-terminated string.
    unsafe fn read_str(ptr: *const c_char) -> Option<alloc::string::String> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees the pointer is valid and terminated.
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .expect("published strings are ASCII")
                .to_string(),
        )
    }

    #[test]
    fn the_registry_is_reachable_through_the_abi() {
        let count = zenith_effect_count();
        assert!(count > 0, "no effects are exported");
        assert_eq!(count as usize, registry::count());

        for index in 0..count {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);
            assert_eq!(zenith_effect_is_known(kind), 1);

            let mut descriptor = impossible_descriptor();
            assert_eq!(zenith_effect_describe(kind, &mut descriptor), Status::Ok);
            assert_eq!(descriptor.kind, kind);

            // The index-based walk must agree with the kind-based one.
            let mut by_index = impossible_descriptor();
            assert_eq!(
                zenith_effect_describe_at(index, &mut by_index),
                Status::Ok
            );
            assert_eq!(by_index.kind, descriptor.kind);
            assert_eq!(by_index.param_count, descriptor.param_count);
        }
    }

    /// A descriptor value that no real effect could produce, so a failed write
    /// is detectable by comparing against it.
    fn impossible_descriptor() -> ZenithEffectDescriptor {
        ZenithEffectDescriptor {
            kind: 0xFFFF_FFFF,
            category: 0xFFFF_FFFF,
            param_count: 0xFFFF_FFFF,
            first_param: 0xFFFF_FFFF,
            has_latency: 0xFFFF_FFFF,
            is_analysis_only: 0xFFFF_FFFF,
            key_utf8: core::ptr::null(),
            label_utf8: core::ptr::null(),
        }
    }

    #[test]
    fn a_failed_call_leaves_the_out_parameter_untouched() {
        // ABI §4.1: on failure the out parameter is not modified, so Dart can
        // safely ignore a partial failure without checking the value.
        let mut kind = 0xAAAA_AAAA_u32;
        assert_eq!(
            zenith_effect_kind_at(u32::MAX, &mut kind),
            Status::OutOfRange
        );
        assert_eq!(kind, 0xAAAA_AAAA, "out parameter was modified on failure");

        let mut descriptor = impossible_descriptor();
        let before = descriptor;
        assert_eq!(
            zenith_effect_describe(0xDEAD_BEEF, &mut descriptor),
            Status::NotFound
        );
        assert_eq!(descriptor, before, "descriptor was modified on failure");

        let mut latency = 0xAAAA_AAAA_u32;
        assert_eq!(
            zenith_effect_latency_samples(0xDEAD_BEEF, 48_000, 256, 2, &mut latency),
            Status::NotFound
        );
        assert_eq!(latency, 0xAAAA_AAAA);
    }

    #[test]
    fn null_out_pointers_are_reported_not_dereferenced() {
        assert_eq!(
            zenith_effect_kind_at(0, core::ptr::null_mut()),
            Status::NullPointer
        );
        assert_eq!(
            zenith_effect_describe(registry::KIND_FILTER_MULTIMODE, core::ptr::null_mut()),
            Status::NullPointer
        );
        assert_eq!(
            zenith_effect_latency_samples(
                registry::KIND_FILTER_MULTIMODE,
                48_000,
                256,
                2,
                core::ptr::null_mut()
            ),
            Status::NullPointer
        );
    }

    #[test]
    fn names_and_keys_round_trip_and_are_nul_terminated() {
        // A missing terminator here reads past the end of a literal, so this is
        // checked explicitly rather than assumed.
        let count = zenith_effect_count();
        for index in 0..count {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);

            // SAFETY: the functions return 'static NUL-terminated literals.
            let name = unsafe { read_str(zenith_effect_name(kind)) };
            // SAFETY: as above.
            let key = unsafe { read_str(zenith_effect_key(kind)) };

            let name = name.unwrap_or_else(|| panic!("{kind:#x} has no name"));
            let key = key.unwrap_or_else(|| panic!("{kind:#x} has no key"));
            assert!(!name.is_empty());
            assert!(!key.is_empty());
            assert!(
                key.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "key {key} is not a stable machine key"
            );
        }
    }

    #[test]
    fn unknown_kinds_report_null_names_not_empty_strings() {
        // Null and "" must not be conflated: one means "no such effect", the
        // other would be a malformed entry.
        assert!(zenith_effect_name(0xDEAD_BEEF).is_null());
        assert!(zenith_effect_key(0xDEAD_BEEF).is_null());
    }

    #[test]
    fn categories_match_the_mirrored_constants() {
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);
            let category = zenith_effect_category(kind);
            assert!(
                category <= zenith_effect_category::UTILITY,
                "{kind:#x} reports unknown category {category}"
            );
        }
        // An unknown kind still yields a renderable category rather than a
        // sentinel the UI would have to special-case.
        assert_eq!(
            zenith_effect_category(0xDEAD_BEEF),
            zenith_effect_category::UTILITY
        );
    }

    #[test]
    fn parameter_counts_agree_between_the_static_and_instance_queries() {
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);

            let static_count = zenith_effect_parameter_count(kind);
            assert!(static_count > 0, "{kind:#x} publishes no parameters");

            // A live instance must report the same number, or Dart's loop
            // bound would not match the table it reads.
            let address = crate::automation::parameter::ParameterAddress::effect(0, 0, 0);
            let mut processor = registry::create(kind, address).expect("registered");
            processor.prepare(48_000.0, 256, 2);

            let instance_count = zenith_effect_instance_parameter_count(
                alloc::boxed::Box::as_ref(&processor) as *const dyn crate::effects::EffectProcessor,
            );
            assert_eq!(
                static_count, instance_count,
                "{kind:#x} disagrees between the registry and a live instance"
            );
        }
    }

    #[test]
    fn every_parameter_of_every_effect_describes_through_the_abi() {
        // This is the whole point of the module: Dart must be able to build a
        // panel for any effect without knowing what it is.
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);

            let address = crate::automation::parameter::ParameterAddress::effect(0, 0, 0);
            let mut processor = registry::create(kind, address).expect("registered");
            processor.prepare(48_000.0, 256, 2);
            let reference: *const dyn crate::effects::EffectProcessor =
                alloc::boxed::Box::as_ref(&processor);

            let count = zenith_effect_parameter_count(kind);
            for ordinal in 0..count {
                let mut mirror = impossible_param_descriptor();
                assert_eq!(
                    zenith_effect_instance_describe_parameter(reference, ordinal, &mut mirror),
                    Status::Ok,
                    "{kind:#x} parameter {ordinal} did not describe"
                );
                // The written descriptor must be structurally usable: a
                // well-ordered range, a default inside it, and a key.
                assert!(
                    mirror.min_value <= mirror.max_value,
                    "{kind:#x} parameter {ordinal} has an inverted range"
                );
                assert!(
                    mirror.min_value <= mirror.default_value
                        && mirror.default_value <= mirror.max_value,
                    "{kind:#x} parameter {ordinal} default is outside its range"
                );
                assert!(
                    !mirror.key_utf8.is_null() && !mirror.label_utf8.is_null(),
                    "{kind:#x} parameter {ordinal} is missing a key or label"
                );
                // SAFETY: the descriptor borrows 'static interned literals.
                let key = unsafe { CStr::from_ptr(mirror.key_utf8) }
                    .to_str()
                    .expect("ASCII key");
                assert!(!key.is_empty(), "{kind:#x} parameter {ordinal} has no key");
                // The unit and flags must be values this build understands.
                assert!(
                    crate::automation::parameter::ParameterUnit::from_u32(mirror.unit).is_some(),
                    "{kind:#x} parameter {ordinal} has unknown unit {}",
                    mirror.unit
                );

                // The address must encode this ordinal, since that is how the
                // automation system will look the parameter up.
                assert_eq!(
                    mirror.id.sub & 0x00FF,
                    ordinal as u16,
                    "{kind:#x} parameter {ordinal} has a mismatched address"
                );
            }

            // One past the end must be refused, not clamped to the last entry.
            let mut mirror = impossible_param_descriptor();
            assert_eq!(
                zenith_effect_instance_describe_parameter(reference, count, &mut mirror),
                Status::OutOfRange,
                "{kind:#x} accepted an out-of-range ordinal"
            );
        }
    }

    #[test]
    fn a_null_processor_is_reported_not_dereferenced() {
        let null: *const dyn crate::effects::EffectProcessor = core::ptr::null();
        assert_eq!(zenith_effect_instance_parameter_count(null), 0);
        let mut mirror = impossible_param_descriptor();
        assert_eq!(
            zenith_effect_instance_describe_parameter(null, 0, &mut mirror),
            Status::NullPointer
        );
    }

    /// A parameter descriptor no real parameter could produce, so a failed
    /// write is detectable.
    fn impossible_param_descriptor() -> ZenithParamDescriptor {
        ZenithParamDescriptor {
            id: crate::ffi::types::ZenithParamId {
                kind: 0xFFFF,
                sub: 0xFFFF,
                index: 0xFFFF_FFFF,
            },
            min_value: f32::NAN,
            max_value: f32::NAN,
            default_value: f32::NAN,
            smoothing_ms: f32::NAN,
            unit: 0xFFFF_FFFF,
            flags: 0xFFFF_FFFF,
            key_utf8: core::ptr::null(),
            label_utf8: core::ptr::null(),
        }
    }

    #[test]
    fn latency_queries_return_a_value_for_every_effect() {
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);

            let mut latency = u32::MAX;
            assert_eq!(
                zenith_effect_latency_samples(kind, 48_000, 256, 2, &mut latency),
                Status::Ok,
                "{kind:#x} did not report latency"
            );
            // A latency beyond a few seconds at 48 kHz means a units error.
            assert!(
                latency < 48_000 * 5,
                "{kind:#x} reports an implausible latency of {latency} samples"
            );
        }
    }

    #[test]
    fn the_latency_query_tolerates_degenerate_parameters() {
        // A host that has not opened a device yet passes zeroes; the query must
        // substitute sane defaults rather than dividing by zero.
        let mut latency = u32::MAX;
        assert_eq!(
            zenith_effect_latency_samples(registry::KIND_REVERB_CONVOLUTION, 0, 0, 0, &mut latency),
            Status::Ok
        );
        assert!(latency < 48_000 * 5, "degenerate inputs gave {latency}");
    }

    #[test]
    fn oversampling_reports_a_factor_the_oversampler_accepts() {
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);
            let factor = zenith_effect_oversampling(kind);
            assert!(
                crate::effects::OversamplingFactor::from_u32(factor).is_some(),
                "{kind:#x} reports invalid oversampling factor {factor}"
            );
        }
    }

    #[test]
    fn the_builtin_and_plugin_ranges_are_distinguishable() {
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);
            assert_eq!(zenith_effect_is_builtin_kind(kind), 1);
        }
        let plugin = zenith_effect_plugin_kind_base();
        assert_eq!(zenith_effect_is_builtin_kind(plugin), 0);
        assert_eq!(zenith_effect_is_known(plugin), 0, "a plugin is not built in");
    }

    #[test]
    fn the_analysis_effects_are_flagged() {
        // The UI groups analysers separately, so at least one must be flagged
        // and none may be flagged by accident.
        let mut flagged = 0;
        for index in 0..zenith_effect_count() {
            let mut kind = 0_u32;
            assert_eq!(zenith_effect_kind_at(index, &mut kind), Status::Ok);
            let mut descriptor = impossible_descriptor();
            assert_eq!(zenith_effect_describe(kind, &mut descriptor), Status::Ok);
            if descriptor.is_analysis_only != 0 {
                flagged += 1;
                assert_eq!(
                    descriptor.category,
                    zenith_effect_category::ANALYSIS,
                    "{kind:#x} is analysis-only but not categorised as analysis"
                );
            }
        }
        assert!(flagged > 0, "no analysis effect is flagged");
    }

    #[test]
    fn size_and_base_helpers_agree_with_the_mirror() {
        assert_eq!(
            zenith_sizeof_effect_descriptor_checked(),
            size_of::<ZenithEffectDescriptor>()
        );
        assert_eq!(zenith_effect_category_analysis(), zenith_effect_category::ANALYSIS);
    }
}

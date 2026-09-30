//! The parameter registry: pre-allocated, atomically readable, hash-free.
//!
//! # Why there is no `HashMap` on the hot path
//!
//! Reading a parameter from a [`crate::automation::player`] happens once per
//! automated parameter per block. A hash lookup — even a fast one — would make
//! the cost depend on the number of registered parameters and would touch a
//! hash table that a control-thread insertion could be resizing concurrently.
//!
//! Instead the store is a **sorted, contiguous `Vec<Slot>` found by binary
//! search over the integer [`ParameterAddress::key`]**. There is no hashing, no
//! allocation, and no lock: insertion happens only on the control thread and
//! re-sorts, while readers see a consistent slice.
//!
//! Values live in `AtomicU32` cells holding the bit pattern of an `f32`, so
//! the control thread can write and the audio thread can read with **Relaxed**
//! ordering and no torn value. Relaxed is sufficient because a parameter value
//! carries no cross-thread invariant: a one-block-late value is inaudible,
//! while the alternative (`SeqCst`) would emit fence instructions inside the
//! audio loop for no benefit.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::parameter::{
    parameter_flags, ParameterAddress, ParameterDescriptor, ParameterKind, ParameterUnit,
};

/// How many parameters the store reserves up front.
///
/// PLAN §3.S3 targets 64 channels with 10 effect slots; a few parameters per
/// channel plus global/master values lands comfortably inside this. Exceeding
/// it is not an error — the store grows — but growth is a control-thread
/// operation, so a realistic project never pays for it while audio runs.
pub const DEFAULT_PARAMETER_CAPACITY: usize = 4096;

/// One parameter's registration record.
struct Slot {
    /// Integer form of the address; the sort key for binary search.
    key: u64,
    /// Static description published to Dart.
    ///
    /// `&'static` because descriptors are installed from `'static` tables;
    /// this is what lets `describe` return borrowed strings with no copy.
    descriptor: &'static ParameterDescriptor,
    /// Current value as raw `f32` bits.
    ///
    /// Bit-punned rather than stored as a float because there is no
    /// `AtomicF32` in core, and a `Mutex<f32>` would violate P5.
    value: AtomicU32,
    /// Smoothing time in milliseconds; see [`crate::automation::player`].
    smoothing_ms: AtomicU32,
}

/// Registry of parameters, their descriptors and their current values.
///
/// Cheap to grow on the control thread; safe to read from the audio thread.
pub struct ParameterStore {
    /// Slots sorted by `Slot::key`; the binary-search index.
    slots: Vec<Slot>,
    /// Number of values ever written, for diagnostics and tests.
    writes: core::sync::atomic::AtomicU64,
}

impl Default for ParameterStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_PARAMETER_CAPACITY)
    }
}

impl ParameterStore {
    /// Creates an empty store with room for `capacity` parameters.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            writes: core::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Number of registered parameters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether nothing is registered yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// How many values have been written since construction.
    #[must_use]
    pub fn write_count(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    /// Registers a parameter from a `'static` descriptor.
    ///
    /// The initial value is the descriptor's default. Re-registering the same
    /// address **updates the descriptor but preserves the current value**, so
    /// rebuilding an engine (or re-activating an effect) does not silently
    /// discard a mix the user already dialled in.
    ///
    /// Returns the index the slot occupies after sorting, or `None` when the
    /// address is not automatable-legal (defensive: a zero-width range would
    /// make every clamp meaningless).
    ///
    /// Control thread only — allocates and re-sorts.
    pub fn register(&mut self, descriptor: &'static ParameterDescriptor) -> Option<usize> {
        if !(descriptor.max_value >= descriptor.min_value) {
            return None;
        }
        let key = descriptor.address.key();
        let default = descriptor.default_value.to_bits();
        let smoothing = descriptor.smoothing_ms.to_bits();

        if let Some(pos) = self.position_of(key) {
            // Keep the live value; only refresh the static description.
            let slot = &self.slots[pos];
            let current = slot.value.load(Ordering::Relaxed);
            let new_slot = Slot {
                key,
                descriptor,
                value: AtomicU32::new(current),
                smoothing_ms: AtomicU32::new(smoothing),
            };
            self.slots[pos] = new_slot;
            return Some(pos);
        }

        let slot = Slot {
            key,
            descriptor,
            value: AtomicU32::new(default),
            smoothing_ms: AtomicU32::new(smoothing),
        };
        // Insert in sorted position so the invariant never lapses.
        let at = self.slots.partition_point(|s| s.key < key);
        self.slots.insert(at, slot);
        Some(at)
    }

    /// Registers a parameter with an explicit initial value.
    ///
    /// Used when loading a project, where the stored value overrides the
    /// descriptor's default.
    pub fn register_with_value(
        &mut self,
        descriptor: &'static ParameterDescriptor,
        value: f32,
    ) -> Option<usize> {
        let index = self.register(descriptor)?;
        // `index` is the post-sort slot position, so this is a direct store.
        let clamped = descriptor.clamp(value);
        self.slots[index].value.store(clamped.to_bits(), Ordering::Relaxed);
        Some(index)
    }

    /// Binary search for a slot by integer key.
    fn position_of(&self, key: u64) -> Option<usize> {
        self.slots
            .binary_search_by_key(&key, |s| s.key)
            .ok()
    }

    /// The descriptor registered at `address`, if any.
    ///
    /// Real-time safe.
    #[must_use]
    pub fn descriptor(&self, address: ParameterAddress) -> Option<&'static ParameterDescriptor> {
        self.position_of(address.key())
            .map(|i| self.slots[i].descriptor)
    }

    /// All registered descriptors, in address order.
    ///
    /// Borrowed from the store — no allocation, no copy.
    #[must_use]
    pub fn descriptors(&self) -> impl Iterator<Item = &'static ParameterDescriptor> + '_ {
        self.slots.iter().map(|s| s.descriptor)
    }

    /// Reads a parameter's current value, or `None` when unregistered.
    ///
    /// Real-time safe: one binary search and one relaxed atomic load.
    #[must_use]
    pub fn read(&self, address: ParameterAddress) -> Option<f32> {
        let index = self.position_of(address.key())?;
        Some(f32::from_bits(self.slots[index].value.load(Ordering::Relaxed)))
    }

    /// Reads a parameter's value with its descriptor, ready for evaluation.
    ///
    /// Combining the two lookups into one call is what keeps the player's
    /// hot loop to a single binary search per parameter per block.
    ///
    /// Real-time safe.
    #[must_use]
    pub fn read_with_descriptor(
        &self,
        address: ParameterAddress,
    ) -> Option<(&'static ParameterDescriptor, f32)> {
        let index = self.position_of(address.key())?;
        let slot = &self.slots[index];
        Some((
            slot.descriptor,
            f32::from_bits(slot.value.load(Ordering::Relaxed)),
        ))
    }

    /// Writes a parameter, clamping into its descriptor's range.
    ///
    /// Returns the value actually stored, or `None` when the address is
    /// unknown. Clamping rather than rejecting keeps a UI slider that
    /// overshoots by a fraction of a pixel working, while still guaranteeing
    /// the audio path never sees an out-of-range value.
    ///
    /// Real-time safe (no allocation; `Relaxed` store).
    pub fn write(&self, address: ParameterAddress, value: f32) -> Option<f32> {
        let index = self.position_of(address.key())?;
        let slot = &self.slots[index];
        let clamped = slot.descriptor.clamp(value);
        slot.value.store(clamped.to_bits(), Ordering::Relaxed);
        self.writes
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Some(clamped)
    }

    /// Writes without clamping, for the automation player's final stage.
    ///
    /// Only the player may call this: it performs its own clamp through the
    /// descriptor, and routing the write through [`Self::write`] as well would
    /// mean two binary searches per parameter per block.
    ///
    /// Real-time safe.
    pub(crate) fn write_clamped_unchecked(
        &self,
        index: usize,
        clamped: f32,
    ) -> bool {
        match self.slots.get(index) {
            Some(slot) => {
                slot.value.store(clamped.to_bits(), Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Slot index for an address, for callers that resolve once and reuse.
    ///
    /// Real-time safe.
    #[must_use]
    pub fn index_of(&self, address: ParameterAddress) -> Option<usize> {
        self.position_of(address.key())
    }

    /// The descriptor at a slot index previously returned by [`Self::index_of`].
    ///
    /// Real-time safe.
    #[must_use]
    pub fn descriptor_at(&self, index: usize) -> Option<&'static ParameterDescriptor> {
        self.slots.get(index).map(|s| s.descriptor)
    }

    /// The smoothing time at a slot index, in milliseconds.
    ///
    /// Real-time safe.
    #[must_use]
    pub fn smoothing_ms_at(&self, index: usize) -> Option<f32> {
        self.slots
            .get(index)
            .map(|s| f32::from_bits(s.smoothing_ms.load(Ordering::Relaxed)))
    }

    /// Sets the smoothing time for a parameter, clamped to the legal 1..50 ms.
    ///
    /// PLAN §3.S2 requirement 3 fixes this range: below 1 ms the one-pole
    /// filter stops removing zipper noise, and above 50 ms a fader starts to
    /// feel disconnected from the mouse.
    ///
    /// Real-time safe.
    pub fn set_smoothing_ms(&self, address: ParameterAddress, ms: f32) -> Option<f32> {
        let index = self.position_of(address.key())?;
        let clamped = if ms.is_finite() { ms.clamp(MIN_SMOOTHING_MS, MAX_SMOOTHING_MS) } else {
            MIN_SMOOTHING_MS
        };
        self.slots[index]
            .smoothing_ms
            .store(clamped.to_bits(), Ordering::Relaxed);
        Some(clamped)
    }

    /// Resets every parameter to its descriptor default.
    ///
    /// Control thread; used when tearing a project down so the next one does
    /// not inherit a stale mix.
    pub fn reset_all_to_default(&self) {
        for slot in &self.slots {
            slot.value
                .store(slot.descriptor.default_value.to_bits(), Ordering::Relaxed);
        }
    }

    /// Descriptors of every parameter on one owner object.
    ///
    /// The `kind`/`index` pair is the owning object; this is how a channel
    /// strip UI asks for exactly its own controls.
    #[must_use]
    pub fn descriptors_for_owner(
        &self,
        kind: ParameterKind,
        index: u32,
    ) -> Vec<&'static ParameterDescriptor> {
        self.slots
            .iter()
            .filter(|s| {
                let address = s.descriptor.address;
                address.kind == kind && address.index == index
            })
            .map(|s| s.descriptor)
            .collect()
    }
}

/// Lowest legal smoothing time, in milliseconds.
pub const MIN_SMOOTHING_MS: f32 = 1.0;
/// Highest legal smoothing time, in milliseconds.
pub const MAX_SMOOTHING_MS: f32 = 50.0;

/// Builds a `'static` descriptor, for use in static registration tables.
///
/// A macro (rather than a const fn) because it needs to produce a value with
/// `'static` lifetime that can be placed directly in a `static` array.
#[macro_export]
macro_rules! param_descriptor {
    (
        kind: $kind:expr, index: $index:expr, sub: $sub:expr,
        key: $key:literal, label: $label:literal,
        unit: $unit:expr, flags: $flags:expr,
        min: $min:expr, max: $max:expr, default: $default:expr,
        smoothing_ms: $smoothing:expr
    ) => {
        $crate::automation::parameter::ParameterDescriptor {
            address: $crate::automation::parameter::ParameterAddress::new($kind, $index, $sub),
            key: $key,
            label: $label,
            unit: $unit,
            flags: $flags,
            min_value: $min,
            max_value: $max,
            default_value: $default,
            smoothing_ms: $smoothing,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::parameter::ParameterAddress;

    const fn desc(
        kind: ParameterKind,
        index: u32,
        sub: u16,
        key: &'static str,
        min: f32,
        max: f32,
        default: f32,
    ) -> ParameterDescriptor {
        ParameterDescriptor {
            address: ParameterAddress::new(kind, index, sub),
            key,
            label: key,
            unit: ParameterUnit::Linear,
            flags: parameter_flags::AUTOMATABLE,
            min_value: min,
            max_value: max,
            default_value: default,
            smoothing_ms: 10.0,
        }
    }

    static VOLUME_0: ParameterDescriptor = desc(ParameterKind::Channel, 0, 0, "volume", -60.0, 12.0, 0.0);
    static PAN_0: ParameterDescriptor = desc(ParameterKind::Channel, 0, 1, "pan", -1.0, 1.0, 0.0);
    static VOLUME_1: ParameterDescriptor = desc(ParameterKind::Channel, 1, 0, "volume", -60.0, 12.0, 0.0);
    static SEND_0: ParameterDescriptor = desc(ParameterKind::Channel, 0, 2, "send.a", -60.0, 12.0, -60.0);

    fn store() -> ParameterStore {
        let mut s = ParameterStore::with_capacity(8);
        s.register(&VOLUME_0).unwrap();
        s.register(&PAN_0).unwrap();
        s.register(&VOLUME_1).unwrap();
        s.register(&SEND_0).unwrap();
        s
    }

    #[test]
    fn registration_populates_defaults() {
        let s = store();
        assert_eq!(s.len(), 4);
        assert_eq!(s.read(VOLUME_0.address), Some(0.0));
        assert_eq!(s.read(SEND_0.address), Some(-60.0));
    }

    #[test]
    fn slots_stay_sorted_by_key_regardless_of_registration_order() {
        // Register in a deliberately shuffled order and verify lookup works.
        let mut s = ParameterStore::with_capacity(4);
        s.register(&VOLUME_1).unwrap();
        s.register(&VOLUME_0).unwrap();
        s.register(&SEND_0).unwrap();
        s.register(&PAN_0).unwrap();
        assert_eq!(s.read(VOLUME_0.address), Some(0.0));
        assert_eq!(s.read(PAN_0.address), Some(0.0));
        assert_eq!(s.read(VOLUME_1.address), Some(0.0));
        assert_eq!(s.read(SEND_0.address), Some(-60.0));
        let keys: Vec<u64> = s.slots.iter().map(|slot| slot.key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "the binary-search invariant must hold");
    }

    #[test]
    fn unknown_address_reads_none_and_write_is_rejected() {
        let s = store();
        let unknown = ParameterAddress::channel(99, 0);
        assert_eq!(s.read(unknown), None);
        assert_eq!(s.write(unknown, 1.0), None);
        assert_eq!(s.descriptor(unknown), None);
        assert_eq!(s.index_of(unknown), None);
    }

    #[test]
    fn write_clamps_into_the_descriptor_range() {
        let s = store();
        assert_eq!(s.write(VOLUME_0.address, 999.0), Some(12.0));
        assert_eq!(s.read(VOLUME_0.address), Some(12.0));
        assert_eq!(s.write(VOLUME_0.address, -999.0), Some(-60.0));
        assert_eq!(s.read(PAN_0.address), Some(0.0));
        assert_eq!(s.write(PAN_0.address, 5.0), Some(1.0));
    }

    #[test]
    fn non_finite_writes_fall_back_to_the_default() {
        let s = store();
        s.write(VOLUME_0.address, 6.0);
        assert_eq!(s.write(VOLUME_0.address, f32::NAN), Some(0.0));
        assert_eq!(s.read(VOLUME_0.address), Some(0.0));
    }

    #[test]
    fn re_registration_preserves_the_live_value() {
        // Rebuilding an engine must not silently reset a mix the user dialled.
        let s = store();
        s.write(VOLUME_0.address, -6.0);
        let mut s = s;
        s.register(&VOLUME_0).unwrap();
        assert_eq!(s.read(VOLUME_0.address), Some(-6.0));
        assert_eq!(s.len(), 4, "re-registering must not add a duplicate slot");
    }

    #[test]
    fn register_with_value_overrides_the_default() {
        let mut s = ParameterStore::with_capacity(2);
        s.register_with_value(&VOLUME_0, -12.0).unwrap();
        assert_eq!(s.read(VOLUME_0.address), Some(-12.0));
    }

    #[test]
    fn register_with_value_clamps() {
        let mut s = ParameterStore::with_capacity(2);
        s.register_with_value(&VOLUME_0, 100.0).unwrap();
        assert_eq!(s.read(VOLUME_0.address), Some(12.0));
    }

    #[test]
    fn inverted_range_descriptor_is_refused() {
        static BAD: ParameterDescriptor = desc(ParameterKind::Global, 0, 0, "bad", 1.0, -1.0, 0.0);
        let mut s = ParameterStore::with_capacity(1);
        assert!(s.register(&BAD).is_none());
        assert!(s.is_empty());
    }

    #[test]
    fn read_with_descriptor_returns_both_in_one_lookup() {
        let s = store();
        let (descriptor, value) = s.read_with_descriptor(PAN_0.address).unwrap();
        assert_eq!(descriptor.key, "pan");
        assert_eq!(value, 0.0);
    }

    #[test]
    fn descriptors_iterate_in_address_order() {
        let s = store();
        let keys: Vec<&str> = s.descriptors().map(|d| d.key).collect();
        assert_eq!(keys.len(), 4);
        // Channel 0 sorts before channel 1, and within channel 0, sub order holds.
        assert_eq!(keys[0], "volume");
        assert_eq!(keys[1], "pan");
        assert_eq!(keys[2], "send.a");
        assert_eq!(keys[3], "volume");
    }

    #[test]
    fn descriptors_for_owner_selects_one_strip() {
        let s = store();
        let channel_0 = s.descriptors_for_owner(ParameterKind::Channel, 0);
        let keys: Vec<&str> = channel_0.iter().map(|d| d.key).collect();
        assert_eq!(keys, vec!["volume", "pan", "send.a"]);
        assert!(s.descriptors_for_owner(ParameterKind::Effect, 0).is_empty());
    }

    #[test]
    fn smoothing_defaults_to_the_descriptor_and_clamps_to_range() {
        let s = store();
        let index = s.index_of(VOLUME_0.address).unwrap();
        assert_eq!(s.smoothing_ms_at(index), Some(10.0));

        assert_eq!(s.set_smoothing_ms(VOLUME_0.address, 0.0), Some(1.0));
        assert_eq!(s.smoothing_ms_at(index), Some(1.0));
        assert_eq!(s.set_smoothing_ms(VOLUME_0.address, 500.0), Some(50.0));
        assert_eq!(s.set_smoothing_ms(VOLUME_0.address, f32::NAN), Some(1.0));
        assert_eq!(s.set_smoothing_ms(ParameterAddress::channel(9, 0), 5.0), None);
    }

    #[test]
    fn reset_all_restores_defaults() {
        let s = store();
        s.write(VOLUME_0.address, -3.0);
        s.write(PAN_0.address, 0.5);
        s.reset_all_to_default();
        assert_eq!(s.read(VOLUME_0.address), Some(0.0));
        assert_eq!(s.read(PAN_0.address), Some(0.0));
    }

    #[test]
    fn write_count_tracks_successful_writes_only() {
        let s = store();
        let before = s.write_count();
        s.write(VOLUME_0.address, 0.0);
        assert_eq!(s.write_count(), before + 1);
        s.write(ParameterAddress::channel(9, 9), 0.0);
        assert_eq!(s.write_count(), before + 1, "rejected writes must not count");
    }

    #[test]
    fn store_grows_past_its_initial_capacity() {
        // Capacity is a hint, not a limit: a bigger project must still load.
        let mut s = ParameterStore::with_capacity(1);
        for channel in 0..200u32 {
            let d: &'static ParameterDescriptor = alloc::boxed::Box::leak(alloc::boxed::Box::new(
                desc(ParameterKind::Channel, channel, 0, "volume", -60.0, 12.0, 0.0),
            ));
            s.register(d).unwrap();
        }
        assert_eq!(s.len(), 200);
        assert_eq!(s.read(ParameterAddress::channel(199, 0)), Some(0.0));
    }
}

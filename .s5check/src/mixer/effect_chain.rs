//! Effect slots: the ten insert positions on every channel strip.
//!
//! The chain holds *descriptors*, not processors. The DSP implementations live
//! in `effects/` (S5) and are addressed here by kind id, so the mixer never
//! depends on any particular effect being compiled in — a slot referencing an
//! unknown kind is simply bypassed rather than breaking the chain.
//!
//! ## Slot identity
//!
//! A slot's *position* is its processing order; its stored kind says what it
//! is. Reordering therefore permutes the kinds while the positions stay `0..10`.
//! This is what lets the UI drag an effect up and down without the engine
//! reallocating anything.

/// Insert slots per channel (PLAN §1.2 / §3.S3).
pub const MAX_EFFECT_SLOTS: usize = 10;

/// A single insert position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectSlot {
    /// Effect kind id, or `None` when the slot is empty.
    ///
    /// Built-in kinds occupy `0x0000_0000..=0x0000_FFFF` and plugin kinds
    /// `0x0001_0000+` (ABI §11 Q2), so the two never collide.
    pub kind: Option<u32>,
    /// Whether the slot is bypassed.
    ///
    /// A bypassed slot keeps its kind and its parameters, so bypass is a
    /// non-destructive A/B rather than a removal.
    pub bypassed: bool,
    /// Wet/dry balance, `0.0` = fully dry, `1.0` = fully wet.
    pub wet: f32,
    /// Sidechain source channel, when this effect listens to another channel.
    pub sidechain_source: Option<u32>,
}

impl EffectSlot {
    /// An empty, unbypassed slot.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            kind: None,
            bypassed: false,
            wet: 1.0,
            sidechain_source: None,
        }
    }

    /// A slot holding `kind`, fully wet.
    #[must_use]
    pub const fn with_kind(kind: u32) -> Self {
        Self {
            kind: Some(kind),
            bypassed: false,
            wet: 1.0,
            sidechain_source: None,
        }
    }

    /// Whether this slot should process audio.
    ///
    /// True only when a kind is loaded *and* the slot is not bypassed *and* it
    /// is not fully dry. The last case matters: a fully dry slot must be
    /// skipped, otherwise it still pays the effect's latency and the plugin
    /// delay compensation would shift the channel for no audible reason.
    #[must_use]
    pub fn is_processing(&self) -> bool {
        self.kind.is_some() && !self.bypassed && self.wet > 0.0
    }

    /// Whether this slot is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.kind.is_none()
    }

    /// Sets the wet/dry balance, clamped to `0.0..=1.0` and NaN-safe.
    pub fn set_wet(&mut self, wet: f32) {
        self.wet = if wet.is_nan() { 1.0 } else { wet.clamp(0.0, 1.0) };
    }

    /// Clears this slot back to empty, discarding its routing.
    pub fn clear(&mut self) {
        *self = Self::empty();
    }
}

/// The ten insert slots of one channel.
#[derive(Debug, Clone, Copy)]
pub struct EffectChain {
    slots: [EffectSlot; MAX_EFFECT_SLOTS],
}

impl Default for EffectChain {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectChain {
    /// Creates ten empty slots.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [EffectSlot::empty(); MAX_EFFECT_SLOTS],
        }
    }

    /// Returns the slot at `index`, or `None` when out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&EffectSlot> {
        self.slots.get(index)
    }

    /// Returns a mutable reference to the slot at `index`, or `None`.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut EffectSlot> {
        self.slots.get_mut(index)
    }

    /// Iterates every slot, empty ones included, in processing order.
    pub fn iter(&self) -> impl Iterator<Item = &EffectSlot> {
        self.slots.iter()
    }

    /// Iterates the slots that will actually process audio, in order.
    pub fn processing(&self) -> impl Iterator<Item = &EffectSlot> {
        self.slots.iter().filter(|s| s.is_processing())
    }

    /// Number of occupied slots, bypassed or not.
    #[must_use]
    pub fn occupied(&self) -> usize {
        self.slots.iter().filter(|s| !s.is_empty()).count()
    }

    /// Inserts `kind` at `index`, pushing later slots one position right.
    ///
    /// Returns `false` when `index` is out of range, or when the chain is
    /// already full and the insert would push a slot off the end. Refusing the
    /// insert (rather than silently dropping the last effect) keeps the user's
    /// chain intact when they add one effect too many.
    pub fn insert(&mut self, index: usize, kind: u32) -> bool {
        if index >= MAX_EFFECT_SLOTS {
            return false;
        }
        if self.slots[MAX_EFFECT_SLOTS - 1].kind.is_some() {
            return false;
        }
        // Shift right from the end, so no slot is overwritten.
        let mut i = MAX_EFFECT_SLOTS - 1;
        while i > index {
            self.slots[i] = self.slots[i - 1];
            i -= 1;
        }
        self.slots[index] = EffectSlot::with_kind(kind);
        true
    }

    /// Removes the slot at `index`, pulling later slots one position left.
    ///
    /// Returns the removed slot, or `None` when out of range.
    pub fn remove(&mut self, index: usize) -> Option<EffectSlot> {
        if index >= MAX_EFFECT_SLOTS {
            return None;
        }
        let removed = self.slots[index];
        let mut i = index;
        while i + 1 < MAX_EFFECT_SLOTS {
            self.slots[i] = self.slots[i + 1];
            i += 1;
        }
        self.slots[MAX_EFFECT_SLOTS - 1] = EffectSlot::empty();
        Some(removed)
    }

    /// Moves the slot at `from` to `to`, shifting the slots between them.
    ///
    /// Returns `false` when either index is out of range or they are equal.
    pub fn move_slot(&mut self, from: usize, to: usize) -> bool {
        if from >= MAX_EFFECT_SLOTS || to >= MAX_EFFECT_SLOTS || from == to {
            return false;
        }
        let moved = self.slots[from];
        if from < to {
            // Shift the intervening slots left to close the gap.
            let mut i = from;
            while i < to {
                self.slots[i] = self.slots[i + 1];
                i += 1;
            }
        } else {
            // Shift them right to open a gap.
            let mut i = from;
            while i > to {
                self.slots[i] = self.slots[i - 1];
                i -= 1;
            }
        }
        self.slots[to] = moved;
        true
    }

    /// Empties every slot.
    pub fn clear(&mut self) {
        self.slots = [EffectSlot::empty(); MAX_EFFECT_SLOTS];
    }

    /// Clears any slot whose sidechain points at `channel`.
    ///
    /// Called when a channel is removed, so no effect is left reading a
    /// channel that no longer exists.
    pub fn drop_sidechain_from(&mut self, channel: u32) {
        for slot in &mut self.slots {
            if slot.sidechain_source == Some(channel) {
                slot.sidechain_source = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_chain_has_ten_empty_slots() {
        let chain = EffectChain::new();
        assert_eq!(chain.iter().count(), MAX_EFFECT_SLOTS);
        assert_eq!(MAX_EFFECT_SLOTS, 10, "PLAN requires ten insert slots");
        assert_eq!(chain.occupied(), 0);
        assert!(chain.processing().next().is_none());
    }

    #[test]
    fn an_empty_slot_never_processes() {
        assert!(!EffectSlot::empty().is_processing());
        assert!(EffectSlot::empty().is_empty());
    }

    #[test]
    fn bypass_and_full_dry_both_stop_processing_but_keep_the_kind() {
        let mut slot = EffectSlot::with_kind(5);
        assert!(slot.is_processing());

        slot.bypassed = true;
        assert!(!slot.is_processing(), "bypassed slot must not process");
        assert_eq!(slot.kind, Some(5), "bypass must not discard the effect");

        slot.bypassed = false;
        slot.set_wet(0.0);
        assert!(
            !slot.is_processing(),
            "a fully dry slot must be skipped so it adds no latency"
        );
        assert_eq!(slot.kind, Some(5), "fully dry must not discard the effect");

        slot.set_wet(0.5);
        assert!(slot.is_processing());
    }

    #[test]
    fn wet_is_clamped_and_nan_safe() {
        let mut slot = EffectSlot::with_kind(1);
        slot.set_wet(2.0);
        assert_eq!(slot.wet, 1.0);
        slot.set_wet(-1.0);
        assert_eq!(slot.wet, 0.0);
        slot.set_wet(f32::NAN);
        assert_eq!(slot.wet, 1.0, "NaN wet should fail safe to fully wet");
    }

    #[test]
    fn inserting_shifts_later_slots_right() {
        let mut chain = EffectChain::new();
        assert!(chain.insert(0, 10));
        assert!(chain.insert(0, 20));
        assert!(chain.insert(1, 30));

        let kinds: Vec<Option<u32>> = chain.iter().map(|s| s.kind).collect();
        assert_eq!(kinds[0], Some(20));
        assert_eq!(kinds[1], Some(30));
        assert_eq!(kinds[2], Some(10));
        assert_eq!(kinds[3], None, "untouched slots stay empty");
        assert_eq!(chain.occupied(), 3);
    }

    #[test]
    fn a_full_chain_refuses_an_insert_instead_of_dropping_an_effect() {
        let mut chain = EffectChain::new();
        for i in 0..MAX_EFFECT_SLOTS {
            assert!(chain.insert(i, i as u32 + 1), "insert {i} should fit");
        }
        assert_eq!(chain.occupied(), MAX_EFFECT_SLOTS);

        let before: Vec<Option<u32>> = chain.iter().map(|s| s.kind).collect();
        assert!(
            !chain.insert(0, 99),
            "inserting into a full chain must be refused"
        );
        let after: Vec<Option<u32>> = chain.iter().map(|s| s.kind).collect();
        assert_eq!(before, after, "a refused insert must not alter the chain");
    }

    #[test]
    fn out_of_range_insert_is_refused() {
        let mut chain = EffectChain::new();
        assert!(!chain.insert(MAX_EFFECT_SLOTS, 1));
        assert!(!chain.insert(usize::MAX, 1));
        assert_eq!(chain.occupied(), 0);
    }

    #[test]
    fn removing_pulls_later_slots_left_and_clears_the_tail() {
        let mut chain = EffectChain::new();
        chain.insert(0, 1);
        chain.insert(1, 2);
        chain.insert(2, 3);

        let removed = chain.remove(1).expect("slot 1 exists");
        assert_eq!(removed.kind, Some(2));

        let kinds: Vec<Option<u32>> = chain.iter().map(|s| s.kind).collect();
        assert_eq!(kinds[0], Some(1));
        assert_eq!(kinds[1], Some(3));
        assert_eq!(kinds[2], None, "the tail must be vacated");
        assert_eq!(chain.occupied(), 2);
    }

    #[test]
    fn removing_out_of_range_returns_none() {
        let mut chain = EffectChain::new();
        assert!(chain.remove(MAX_EFFECT_SLOTS).is_none());
        assert!(chain.remove(usize::MAX).is_none());
    }

    #[test]
    fn reordering_preserves_every_occupied_slot_exactly_once() {
        let mut chain = EffectChain::new();
        for i in 0..4 {
            chain.insert(i, 100 + i as u32);
        }

        // Move the first slot to the end.
        assert!(chain.move_slot(0, 3));
        let kinds: Vec<Option<u32>> = chain.iter().take(4).map(|s| s.kind).collect();
        assert_eq!(kinds, vec![Some(101), Some(102), Some(103), Some(100)]);

        // Move it back to the front.
        assert!(chain.move_slot(3, 0));
        let kinds: Vec<Option<u32>> = chain.iter().take(4).map(|s| s.kind).collect();
        assert_eq!(kinds, vec![Some(100), Some(101), Some(102), Some(103)]);
        assert_eq!(chain.occupied(), 4, "reordering must not lose or add slots");
    }

    #[test]
    fn reordering_carries_the_slots_settings_with_it() {
        let mut chain = EffectChain::new();
        chain.insert(0, 1);
        chain.insert(1, 2);
        {
            let s = chain.get_mut(0).expect("in range");
            s.bypassed = true;
            s.set_wet(0.25);
            s.sidechain_source = Some(7);
        }

        assert!(chain.move_slot(0, 1));

        let moved = chain.get(1).expect("in range");
        assert_eq!(moved.kind, Some(1));
        assert!(moved.bypassed, "bypass must travel with the effect");
        assert!((moved.wet - 0.25).abs() < 1e-6, "wet must travel too");
        assert_eq!(moved.sidechain_source, Some(7), "routing must travel too");
    }

    #[test]
    fn a_no_op_or_out_of_range_move_is_refused() {
        let mut chain = EffectChain::new();
        chain.insert(0, 1);
        assert!(!chain.move_slot(0, 0), "moving onto itself is a no-op");
        assert!(!chain.move_slot(0, MAX_EFFECT_SLOTS));
        assert!(!chain.move_slot(MAX_EFFECT_SLOTS, 0));
        assert_eq!(chain.get(0).expect("in range").kind, Some(1));
    }

    #[test]
    fn dropping_a_channel_clears_only_sidechains_that_reference_it() {
        let mut chain = EffectChain::new();
        chain.insert(0, 1);
        chain.insert(1, 2);
        chain.get_mut(0).expect("in range").sidechain_source = Some(5);
        chain.get_mut(1).expect("in range").sidechain_source = Some(6);

        chain.drop_sidechain_from(5);

        assert_eq!(chain.get(0).expect("in range").sidechain_source, None);
        assert_eq!(chain.get(1).expect("in range").sidechain_source, Some(6));
        assert_eq!(chain.get(0).expect("in range").kind, Some(1), "kind kept");
    }

    #[test]
    fn processing_iterates_in_slot_order_and_skips_bypassed_slots() {
        let mut chain = EffectChain::new();
        chain.insert(0, 1);
        chain.insert(1, 2);
        chain.insert(2, 3);
        chain.get_mut(1).expect("in range").bypassed = true;

        let kinds: Vec<u32> = chain.processing().filter_map(|s| s.kind).collect();
        assert_eq!(kinds, vec![1, 3]);
    }

    #[test]
    fn clearing_empties_the_whole_chain() {
        let mut chain = EffectChain::new();
        for i in 0..3 {
            chain.insert(i, i as u32 + 1);
        }
        chain.clear();
        assert_eq!(chain.occupied(), 0);
        assert!(chain.iter().all(EffectSlot::is_empty));
    }
}

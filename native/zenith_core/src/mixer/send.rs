//! Sends: auxiliary feeds from a channel into a return bus.
//!
//! Each channel carries a fixed four sends (PLAN §3.S3 item 3). A send has an
//! independent enable, level, and a tap point that decides whether it listens
//! *before* the channel fader (pre-fader) or *after* it (post-fader).
//!
//! ## Why the tap point matters
//!
//! It is the difference between two kinds of processing:
//!
//! * **Post-fader** listens to the signal as the audience hears it. Pulling the
//!   channel down pulls its reverb down with it, which is what a mix usually
//!   wants.
//! * **Pre-fader** listens to the raw signal, so a performer's headphone
//!   monitor mix stays put while the engineer rides the fader. It also lets a
//!   channel be silenced in the main mix while still feeding an effect.
//!
//! Getting this backwards is a classic and very audible mixing bug, so the tap
//! is an explicit enum rather than a boolean whose meaning has to be guessed.

/// Sends available on every channel (PLAN §3.S3 item 3).
pub const MAX_SENDS_PER_CHANNEL: usize = 4;

/// Where a send taps the channel signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SendTap {
    /// Post-fader: the send follows the channel fader and pan.
    ///
    /// The default, because it keeps an effect's balance with the dry signal
    /// constant as the mix is built.
    #[default]
    PostFader,
    /// Pre-fader: the send ignores the channel fader.
    ///
    /// Still honours mute, so muting a channel silences its sends too — a
    /// muted channel that keeps feeding a reverb is almost never intended.
    PreFader,
}

/// One send slot.
#[derive(Debug, Clone, Copy)]
pub struct Send {
    /// Whether this send contributes to its return bus.
    pub enabled: bool,
    /// Send level in decibels, `MIN_GAIN_DB..=MAX_GAIN_DB`.
    pub level_db: f32,
    /// Where the send taps the channel.
    pub tap: SendTap,
    /// Destination return-bus index, or `None` when unrouted.
    pub destination: Option<u32>,
}

impl Send {
    /// Creates a disabled, unrouted send.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            level_db: 0.0,
            tap: SendTap::PostFader,
            destination: None,
        }
    }

    /// Creates an enabled send at the given level, routed to `destination`.
    #[must_use]
    pub fn to(destination: u32, level_db: f32) -> Self {
        Self {
            enabled: true,
            level_db: super::channel::clamp_db(level_db),
            tap: SendTap::PostFader,
            destination: Some(destination),
        }
    }

    /// Whether this send currently contributes.
    ///
    /// A send needs both its own enable flag and a destination; enabling a send
    /// before choosing where it goes must not send the signal to bus zero.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.enabled && self.destination.is_some()
    }

    /// Linear gain this send applies, or `0.0` when inactive.
    #[must_use]
    pub fn linear_gain(&self) -> f32 {
        if !self.is_active() {
            return 0.0;
        }
        super::channel::db_to_gain(self.level_db)
    }

    /// Sets the send level in decibels, clamped to the legal range.
    pub fn set_level_db(&mut self, db: f32) {
        self.level_db = super::channel::clamp_db(db);
    }
}

/// The fixed array of sends belonging to one channel.
///
/// A plain array rather than a `Vec`: the count is fixed by the plan, and a
/// fixed-size array is what lets the audio path iterate without a bounds check
/// against a length that could change underneath it.
#[derive(Debug, Clone, Copy)]
pub struct SendBank {
    sends: [Send; MAX_SENDS_PER_CHANNEL],
}

impl Default for SendBank {
    fn default() -> Self {
        Self::new()
    }
}

impl SendBank {
    /// Creates four disabled sends.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sends: [Send::disabled(); MAX_SENDS_PER_CHANNEL],
        }
    }

    /// Returns the send at `index`, or `None` when out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Send> {
        self.sends.get(index)
    }

    /// Returns a mutable reference to the send at `index`, or `None`.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Send> {
        self.sends.get_mut(index)
    }

    /// Iterates every send slot, active or not.
    pub fn iter(&self) -> impl Iterator<Item = &Send> {
        self.sends.iter()
    }

    /// Iterates the slots that currently contribute.
    pub fn active(&self) -> impl Iterator<Item = &Send> {
        self.sends.iter().filter(|s| s.is_active())
    }

    /// Silences and disconnects every send.
    pub fn clear(&mut self) {
        self.sends = [Send::disabled(); MAX_SENDS_PER_CHANNEL];
    }

    /// Disconnects any send pointing at `destination`.
    ///
    /// Called when a return bus is removed, so no send is left addressing a
    /// slot that no longer exists.
    pub fn disconnect_from(&mut self, destination: u32) {
        for send in &mut self.sends {
            if send.destination == Some(destination) {
                send.destination = None;
                send.enabled = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::MIN_GAIN_DB;

    #[test]
    fn a_new_bank_has_exactly_four_disabled_sends() {
        let bank = SendBank::new();
        assert_eq!(bank.iter().count(), MAX_SENDS_PER_CHANNEL);
        assert_eq!(MAX_SENDS_PER_CHANNEL, 4, "PLAN 3.S3 requires four sends");
        assert!(bank.active().next().is_none(), "no send starts active");
    }

    #[test]
    fn an_enabled_send_without_a_destination_stays_inactive() {
        let mut send = Send::disabled();
        send.enabled = true;
        assert!(
            !send.is_active(),
            "enabling a send before routing it must not route it to bus 0"
        );
        assert_eq!(send.linear_gain(), 0.0);
    }

    #[test]
    fn an_unrouted_send_contributes_nothing() {
        let send = Send::disabled();
        assert!(!send.is_active());
        assert_eq!(send.linear_gain(), 0.0);
    }

    #[test]
    fn a_routed_send_at_unity_has_unit_gain() {
        let send = Send::to(3, 0.0);
        assert!(send.is_active());
        assert_eq!(send.destination, Some(3));
        assert!((send.linear_gain() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn send_level_is_a_db_curve_and_clamps() {
        let mut send = Send::to(1, 6.0);
        assert!((send.linear_gain() - 1.995_262).abs() < 1e-3, "+6 dB ≈ ×2");
        send.set_level_db(99.0);
        assert_eq!(send.level_db, super::super::MAX_GAIN_DB);
        send.set_level_db(-999.0);
        assert_eq!(send.level_db, MIN_GAIN_DB);
        // The bottom of the travel is a true silence, not a tiny gain.
        assert_eq!(send.linear_gain(), 0.0);
    }

    #[test]
    fn post_fader_is_the_default_tap() {
        assert_eq!(SendTap::default(), SendTap::PostFader);
        assert_eq!(Send::disabled().tap, SendTap::PostFader);
        assert_eq!(Send::to(0, 0.0).tap, SendTap::PostFader);
    }

    #[test]
    fn the_tap_point_can_be_switched_per_send() {
        let mut bank = SendBank::new();
        let s0 = bank.get_mut(0).expect("slot 0 exists");
        s0.enabled = true;
        s0.destination = Some(1);
        s0.tap = SendTap::PreFader;
        let s1 = bank.get_mut(1).expect("slot 1 exists");
        s1.enabled = true;
        s1.destination = Some(2);
        // slot 1 keeps the default post-fader tap.

        let taps: Vec<SendTap> = bank.active().map(|s| s.tap).collect();
        assert_eq!(taps, vec![SendTap::PreFader, SendTap::PostFader]);
    }

    #[test]
    fn out_of_range_slots_return_none_instead_of_panicking() {
        let mut bank = SendBank::new();
        assert!(bank.get(MAX_SENDS_PER_CHANNEL).is_none());
        assert!(bank.get(99).is_none());
        assert!(bank.get_mut(MAX_SENDS_PER_CHANNEL).is_none());
        assert!(bank.get_mut(usize::MAX).is_none());
    }

    #[test]
    fn clearing_removes_every_send() {
        let mut bank = SendBank::new();
        for i in 0..MAX_SENDS_PER_CHANNEL {
            let s = bank.get_mut(i).expect("in range");
            s.enabled = true;
            s.destination = Some(i as u32 + 1);
        }
        assert_eq!(bank.active().count(), MAX_SENDS_PER_CHANNEL);
        bank.clear();
        assert_eq!(bank.active().count(), 0);
    }

    #[test]
    fn removing_a_bus_disconnects_only_its_own_sends() {
        let mut bank = SendBank::new();
        bank.get_mut(0).expect("in range").destination = Some(7);
        bank.get_mut(0).expect("in range").enabled = true;
        bank.get_mut(1).expect("in range").destination = Some(8);
        bank.get_mut(1).expect("in range").enabled = true;

        bank.disconnect_from(7);

        let first = bank.get(0).expect("in range");
        assert!(!first.is_active(), "the removed bus must be disconnected");
        assert_eq!(first.destination, None);
        let second = bank.get(1).expect("in range");
        assert!(second.is_active(), "an unrelated send must be untouched");
        assert_eq!(second.destination, Some(8));
    }
}

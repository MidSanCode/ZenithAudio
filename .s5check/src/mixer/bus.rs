//! Buses: the sum points that channels route into.
//!
//! There are three kinds and they differ in where their output goes, which is
//! the only thing that distinguishes them structurally:
//!
//! | Kind | Fed by | Output goes to |
//! |---|---|---|
//! | Insert | a track's instrument | a bus or another group |
//! | Return | sends from other channels | a bus, typically master |
//! | Group | other channels' routing | a bus or another group |
//! | Master | everything upstream | the audio device |
//!
//! Modelling them as one type with a `BusKind` rather than four separate types
//! keeps the routing code uniform: the graph only ever asks "what does this
//! channel feed?", and the answer is another channel index.

use super::channel::ChannelRole;

/// Prefix applied to a bus index when it is written to a project file.
///
/// Keeps persisted ids readable (`bus:3`) while the engine works in plain
/// integers, and stops a bus id from being confused with a track id.
pub const BUS_ID_PREFIX: &str = "bus:";

/// What a bus is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusKind {
    /// A return bus fed by channel sends.
    Return,
    /// A group bus that sums several channels.
    Group,
    /// The single master bus.
    Master,
}

impl BusKind {
    /// The channel role a bus of this kind plays in the graph.
    ///
    /// Return and group buses are both [`ChannelRole::Return`] and
    /// [`ChannelRole::Group`] respectively; only master differs in that it
    /// terminates routing.
    #[must_use]
    pub const fn channel_role(self) -> ChannelRole {
        match self {
            Self::Return => ChannelRole::Return,
            Self::Group => ChannelRole::Group,
            Self::Master => ChannelRole::Master,
        }
    }

    /// Whether a bus of this kind may itself be routed onward.
    ///
    /// Master is the terminal node: routing it anywhere would either create a
    /// cycle or produce a second output path to the device.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Master)
    }
}

/// A bus and its presentation metadata.
#[derive(Debug, Clone, Copy)]
pub struct Bus {
    /// Index of the channel that implements this bus.
    pub channel: u32,
    /// What the bus is for.
    pub kind: BusKind,
    /// Display name.
    ///
    /// A fixed-size array rather than a `String`: bus names are edited on the
    /// control thread but read by the UI, and a fixed buffer keeps the struct
    /// `Copy` and free of allocation on every path.
    name: [u8; MAX_BUS_NAME_LEN],
    /// Length of the valid prefix of `name`, in bytes.
    name_len: usize,
}

/// Maximum stored name length, in bytes.
///
/// 32 bytes fits a reasonable label in UTF-8 without pushing the struct to an
/// awkward size.
pub const MAX_BUS_NAME_LEN: usize = 32;

impl Bus {
    /// Creates a bus with an empty name.
    #[must_use]
    pub const fn new(channel: u32, kind: BusKind) -> Self {
        Self {
            channel,
            kind,
            name: [0; MAX_BUS_NAME_LEN],
            name_len: 0,
        }
    }

    /// Sets the display name, truncating on a UTF-8 character boundary.
    ///
    /// Truncation respects char boundaries so a clipped name can never produce
    /// invalid UTF-8 when it is read back.
    pub fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let mut len = bytes.len().min(MAX_BUS_NAME_LEN);
        // Walk back to a boundary if the cut landed mid-character.
        while len > 0 && !name.is_char_boundary(len) {
            len -= 1;
        }
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name_len = len;
    }

    /// Returns the display name, or an empty string when unset.
    #[must_use]
    pub fn name(&self) -> &str {
        // The buffer is only ever written through `set_name`, which keeps it
        // valid UTF-8, so this cannot fail.
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
}

/// The master bus.
///
/// A distinct type because there is exactly one and it always exists: callers
/// should not have to handle its absence.
#[derive(Debug, Clone, Copy)]
pub struct MasterBus {
    /// The channel index backing master. Always
    /// [`super::channel::ChannelId::MASTER`].
    pub channel: u32,
    /// Bus metadata (name, kind).
    pub bus: Bus,
}

impl Default for MasterBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MasterBus {
    /// Creates the master bus at the reserved channel index.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            channel: 0,
            bus: Bus::new(0, BusKind::Master),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_is_terminal_and_others_are_not() {
        assert!(BusKind::Master.is_terminal());
        assert!(!BusKind::Return.is_terminal());
        assert!(!BusKind::Group.is_terminal());
    }

    #[test]
    fn bus_kinds_map_onto_the_right_channel_roles() {
        assert_eq!(BusKind::Return.channel_role(), ChannelRole::Return);
        assert_eq!(BusKind::Group.channel_role(), ChannelRole::Group);
        assert_eq!(BusKind::Master.channel_role(), ChannelRole::Master);
    }

    #[test]
    fn a_new_bus_has_an_empty_name() {
        let bus = Bus::new(3, BusKind::Return);
        assert_eq!(bus.name(), "");
        assert_eq!(bus.channel, 3);
        assert_eq!(bus.kind, BusKind::Return);
    }

    #[test]
    fn names_round_trip() {
        let mut bus = Bus::new(1, BusKind::Group);
        bus.set_name("Drum Bus");
        assert_eq!(bus.name(), "Drum Bus");
    }

    #[test]
    fn long_names_are_truncated_at_a_char_boundary() {
        let mut bus = Bus::new(1, BusKind::Group);
        // 20 three-byte characters = 60 bytes, well past the 32-byte cap.
        let long = "混".repeat(20);
        bus.set_name(&long);

        let stored = bus.name();
        assert!(stored.len() <= MAX_BUS_NAME_LEN, "stored {} bytes", stored.len());
        // The important property: the result is still valid UTF-8, which the
        // accessor guarantees by falling back to "" on a bad slice. A byte-wise
        // truncation would have sliced a character in half.
        assert!(!stored.is_empty(), "truncation should keep a prefix");
        assert!(stored.chars().all(|c| c == '混'), "stored a broken character");
    }

    #[test]
    fn renaming_shorter_replaces_the_previous_name_entirely() {
        let mut bus = Bus::new(1, BusKind::Group);
        bus.set_name("A Long Bus Name");
        bus.set_name("Snare");
        assert_eq!(bus.name(), "Snare", "stale bytes must not leak through");
    }

    #[test]
    fn an_exactly_max_length_name_is_kept_whole() {
        let mut bus = Bus::new(1, BusKind::Group);
        let exact = "x".repeat(MAX_BUS_NAME_LEN);
        bus.set_name(&exact);
        assert_eq!(bus.name().len(), MAX_BUS_NAME_LEN);
        assert_eq!(bus.name(), exact);
    }

    #[test]
    fn the_master_bus_lives_at_channel_zero() {
        let m = MasterBus::new();
        assert_eq!(m.channel, 0, "master must occupy the reserved index");
        assert_eq!(m.bus.kind, BusKind::Master);
        assert_eq!(MasterBus::default().channel, m.channel);
    }
}

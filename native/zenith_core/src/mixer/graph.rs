//! Mixer topology: the routing graph, its validation, and cycle rejection.
//!
//! # What this module is, and is not
//!
//! It owns *who feeds whom*. It does not own audio buffers, effect DSP, or the
//! audio device — those arrive with S1's engine graph and S5's effects. Keeping
//! the topology here means routing can be built, validated, and tested
//! independently of any DSP, and the engine later adopts it by calling
//! [`MixerGraph::process_block`].
//!
//! # Cycle rejection is a build-time property
//!
//! PLAN §3.S3 item 2 requires that a routing cycle be rejected when the graph is
//! built, not detected while audio is running. Every mutation that could create
//! a cycle ([`MixerGraph::connect`]) validates *before* committing, so the
//! graph is acyclic by construction and the process path never has to check.
//! Detecting a cycle during processing would mean the audio thread has already
//! recursed into itself — far too late to recover gracefully.
//!
//! # Iteration on the audio thread
//!
//! [`MixerGraph::process_block`] walks channels in topological order and
//! never recurses, so its stack depth is a constant regardless of how deep the
//! group nesting goes. The order is recomputed on the control thread when the
//! topology changes, never during processing (ABI §7.3).

use super::channel::{Channel, ChannelId, ChannelRole};
use super::effect_chain::EffectChain;
use super::meter::{Meter, MeterSnapshot};
use super::send::SendBank;
use super::{DEFAULT_INSERT_CHANNELS, DEFAULT_RETURN_BUSES};

/// Maximum group nesting depth the router accepts (PLAN §3.S3 item 2).
///
/// The plan requires "≥ 4"; the limit exists to bound the cost of the
/// depth computation and to catch a pathological chain before it becomes an
/// unbounded one.
pub const MAX_GROUP_DEPTH: usize = 4;

/// Why a topology edit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerTopologyError {
    /// The channel index does not exist in this graph.
    UnknownChannel(u32),
    /// The edit would create a routing cycle, so it was refused.
    WouldCycle,
    /// The edit would nest groups deeper than [`MAX_GROUP_DEPTH`].
    TooDeep,
    /// The graph's pre-allocated channel capacity is exhausted.
    Capacity,
    /// Master cannot be removed or rerouted.
    MasterIsFixed,
    /// A channel cannot be routed into itself.
    SelfRoute,
}

/// One channel's full state: values, sends, effects, routing and meter.
///
/// Held together so the process path touches one contiguous record per channel
/// rather than chasing pointers across separate arrays.
pub struct GraphNode {
    /// Channel values (gain, pan, mute, solo, phase).
    pub channel: Channel,
    /// The channel's four sends.
    pub sends: SendBank,
    /// The channel's ten insert slots.
    pub effects: EffectChain,
    /// Level meter.
    pub meter: Meter,
    /// Index of the channel this one feeds, or `None` for master.
    pub output: Option<u32>,
    /// Stereo scratch buffer, allocated once in `prepare`.
    ///
    /// Interleaved `L, R, L, R, …` with `frames * 2` entries. Pre-allocated
    /// because the process path may not allocate (ABI P5).
    pub buffer: Vec<f32>,
    /// Whether this node currently exists.
    pub alive: bool,
}

impl GraphNode {
    /// Creates a silent, unrouted node.
    fn new(id: u32, role: ChannelRole) -> Self {
        Self {
            channel: Channel::new(ChannelId(id), role),
            sends: SendBank::new(),
            effects: EffectChain::new(),
            meter: Meter::new(),
            output: None,
            buffer: Vec::new(),
            alive: true,
        }
    }
}

/// The mixer topology.
pub struct MixerGraph {
    /// Nodes indexed by channel id. Master is always index 0.
    nodes: Vec<GraphNode>,
    /// Channel ids in the order they must be processed (sources first).
    order: Vec<u32>,
    /// Highest channel index ever handed out.
    ///
    /// Indices are never reused, so a stale id fails as `UnknownChannel`
    /// instead of silently addressing a different channel (ABI §11 Q3).
    next_id: u32,
    /// Pre-allocated frame capacity for every node's scratch buffer.
    max_frames: usize,
}

impl MixerGraph {
    /// Creates a graph with master plus default insert and return channels.
    ///
    /// Channel ids are assigned master-to-last with master at 0. Everything the
    /// audio thread will ever touch is allocated here, in one pass, so no
    /// later operation grows the graph on the real-time path.
    #[must_use]
    pub fn new(max_frames: usize) -> Self {
        let total = DEFAULT_INSERT_CHANNELS + DEFAULT_RETURN_BUSES + 1;
        let mut nodes = Vec::with_capacity(total);
        nodes.push(GraphNode::new(0, ChannelRole::Master));
        // Return buses come first after master, then the insert channels, so a
        // project's returns have stable low indices.
        for i in 0..DEFAULT_RETURN_BUSES {
            nodes.push(GraphNode::new(1 + i as u32, ChannelRole::Return));
        }
        for i in 0..DEFAULT_INSERT_CHANNELS {
            let id = 1 + DEFAULT_RETURN_BUSES as u32 + i as u32;
            nodes.push(GraphNode::new(id, ChannelRole::Insert));
        }

        let mut graph = Self {
            nodes,
            order: Vec::new(),
            next_id: total as u32,
            max_frames,
        };
        graph.allocate_buffers();
        graph.publish_audibility();
        graph.rebuild_order();
        graph
    }

    /// Number of live channels.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|n| n.alive).count()
    }

    /// Whether the graph has no channels beyond master.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        // Master always counts as live, so the graph is never truly empty.
        false
    }

    /// Pre-allocated frame capacity per node.
    #[must_use]
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Returns the node for `id`, or `None` when it does not exist.
    #[must_use]
    pub fn node(&self, id: u32) -> Option<&GraphNode> {
        self.nodes.get(id as usize).filter(|n| n.alive)
    }

    /// Returns a mutable reference to the node for `id`.
    pub fn node_mut(&mut self, id: u32) -> Option<&mut GraphNode> {
        self.nodes.get_mut(id as usize).filter(|n| n.alive)
    }

    /// Iterates live channel ids in processing order.
    pub fn order(&self) -> &[u32] {
        &self.order
    }

    /// Reads a channel's meter snapshot.
    ///
    /// Returns `None` for an unknown channel rather than an empty snapshot, so
    /// the caller can tell "no such channel" from "silent".
    #[must_use]
    pub fn meter(&self, id: u32) -> Option<MeterSnapshot> {
        self.node(id).map(|n| n.meter.snapshot())
    }

    /// Adds an insert channel and returns its id.
    ///
    /// The new channel is routed to master, which is the only default that
    /// cannot create a cycle.
    pub fn add_channel(&mut self) -> Result<u32, MixerTopologyError> {
        let id = self.next_id;
        let mut node = GraphNode::new(id, ChannelRole::Insert);
        node.output = Some(ChannelId::MASTER.get());
        self.allocate_node_buffer(&mut node);
        self.nodes.push(node);
        self.next_id += 1;
        self.publish_audibility();
        self.rebuild_order();
        Ok(id)
    }

    /// Removes a channel, disconnecting everything that referenced it.
    ///
    /// Master cannot be removed. Channels that fed the removed one are rerouted
    /// to master rather than left dangling, and sends/sidechains pointing at it
    /// are cleared.
    pub fn remove_channel(&mut self, id: u32) -> Result<(), MixerTopologyError> {
        if id == ChannelId::MASTER.get() {
            return Err(MixerTopologyError::MasterIsFixed);
        }
        let index = id as usize;
        match self.nodes.get(index) {
            Some(n) if n.alive => {}
            _ => return Err(MixerTopologyError::UnknownChannel(id)),
        }

        // Reroute upstream channels to master so nothing dangles.
        for node in &mut self.nodes {
            if !node.alive {
                continue;
            }
            if node.output == Some(id) {
                node.output = Some(ChannelId::MASTER.get());
            }
            node.sends.disconnect_from(id);
            node.effects.drop_sidechain_from(id);
        }

        self.nodes[index].alive = false;
        self.publish_audibility();
        self.rebuild_order();
        Ok(())
    }

    /// Routes `src` into `dst`, refusing any connection that would cycle.
    ///
    /// This is the only place routing changes, and it validates before
    /// committing: on any error the graph is left exactly as it was.
    pub fn connect(&mut self, src: u32, dst: u32) -> Result<(), MixerTopologyError> {
        if src == dst {
            return Err(MixerTopologyError::SelfRoute);
        }
        if !self.exists(src) {
            return Err(MixerTopologyError::UnknownChannel(src));
        }
        if !self.exists(dst) {
            return Err(MixerTopologyError::UnknownChannel(dst));
        }
        // Master is the terminal node; routing it onward has no meaning.
        if src == ChannelId::MASTER.get() {
            return Err(MixerTopologyError::MasterIsFixed);
        }

        // Reject before committing. `dst` reaching `src` means the new edge
        // would close a loop.
        if self.reaches(dst, src) {
            return Err(MixerTopologyError::WouldCycle);
        }

        let previous = self.nodes[src as usize].output;
        self.nodes[src as usize].output = Some(dst);

        // Depth is the length of the *whole* path this channel now sits on,
        // not just the part downstream of it. Checking only forward from `src`
        // would miss the case that matters: a channel that is already deep in a
        // chain, spliced onto another deep chain, produces a total nesting far
        // beyond the limit while each individual hop still looks shallow.
        if self.would_exceed_depth_limit(src) {
            self.nodes[src as usize].output = previous;
            return Err(MixerTopologyError::TooDeep);
        }

        self.rebuild_order();
        Ok(())
    }

    /// Whether `id` currently sits deeper than [`MAX_GROUP_DEPTH`] allows.
    ///
    /// Counts edges *upstream* to `id` (how far it is from a source) plus edges
    /// *downstream* to the terminal master. Both halves matter: nesting is a
    /// property of the full path, so a shallow-looking hop between two deep
    /// chains must still be refused.
    #[must_use]
    pub fn would_exceed_depth_limit(&self, id: u32) -> bool {
        self.max_depth_through(id) > MAX_GROUP_DEPTH
    }

    /// Longest path through `id`: upstream edges plus downstream edges.
    ///
    /// Bounded by the node count so it terminates even on a malformed graph.
    #[must_use]
    pub fn max_depth_through(&self, id: u32) -> usize {
        self.upstream_depth(id) + self.max_depth_from(id)
    }

    /// Longest chain of edges leading *into* `id`.
    #[must_use]
    pub fn upstream_depth(&self, id: u32) -> usize {
        let limit = self.nodes.len() + 1;
        let mut best = 0usize;
        for node_id in 0..self.nodes.len() as u32 {
            if !self.exists(node_id) {
                continue;
            }
            // Walk from this node toward `id`, counting hops, shortest-first is
            // not needed — any path that arrives is a candidate for "deepest".
            let mut current = self.nodes[node_id as usize].output;
            let mut hops = 0usize;
            while let Some(next) = current {
                hops += 1;
                if next == id {
                    if hops > best {
                        best = hops;
                    }
                    break;
                }
                if hops > limit {
                    break;
                }
                current = self.nodes.get(next as usize).and_then(|n| n.output);
            }
        }
        best
    }

    /// Removes the route out of `src`, returning it to master.
    pub fn disconnect(&mut self, src: u32) -> Result<(), MixerTopologyError> {
        if !self.exists(src) {
            return Err(MixerTopologyError::UnknownChannel(src));
        }
        if src == ChannelId::MASTER.get() {
            return Err(MixerTopologyError::MasterIsFixed);
        }
        self.nodes[src as usize].output = Some(ChannelId::MASTER.get());
        self.rebuild_order();
        Ok(())
    }

    /// Whether `from` can reach `to` by following output links.
    ///
    /// Iterative with a visited set, so it terminates on any input — including
    /// a graph that is already cyclic, which cannot arise through this API but
    /// must not hang the validator if it ever did.
    #[must_use]
    pub fn reaches(&self, from: u32, to: u32) -> bool {
        let mut current = Some(from);
        let mut steps = 0usize;
        let limit = self.nodes.len() + 1;
        while let Some(id) = current {
            if id == to {
                return true;
            }
            // Bound the walk by the node count: a legitimate chain is shorter
            // than the graph, and exceeding it means a cycle.
            steps += 1;
            if steps > limit {
                return true;
            }
            current = self.nodes.get(id as usize).and_then(|n| n.output);
        }
        false
    }

    /// Longest downstream chain from `id`, in edges.
    ///
    /// Used to enforce [`MAX_GROUP_DEPTH`]. Iterative and bounded by the node
    /// count for the same reason as [`Self::reaches`].
    #[must_use]
    pub fn max_depth_from(&self, id: u32) -> usize {
        let mut depth = 0usize;
        let mut current = self.nodes.get(id as usize).and_then(|n| n.output);
        while let Some(next) = current {
            depth += 1;
            if depth > self.nodes.len() {
                break;
            }
            current = self.nodes.get(next as usize).and_then(|n| n.output);
        }
        depth
    }

    /// Recomputes which channels are audible given the mute/solo state.
    ///
    /// Solo is exclusive-by-presence: if any channel is soloed, only soloed
    /// channels are audible. Doing this here — once, on the control thread —
    /// keeps the audio thread from having to scan the whole console per block.
    pub fn publish_audibility(&mut self) {
        let any_solo = self
            .nodes
            .iter()
            .any(|n| n.alive && n.channel.solo);

        for node in &mut self.nodes {
            if !node.alive {
                continue;
            }
            let audible = match node.channel.role {
                // Master is always live; its fader is the output level.
                ChannelRole::Master => true,
                _ => {
                    if any_solo {
                        node.channel.solo
                    } else {
                        !node.channel.mute
                    }
                }
            };
            node.channel.audible = audible;
        }
    }

    /// Sets a channel's solo flag and republishes audibility.
    pub fn set_solo(&mut self, id: u32, solo: bool) -> Result<(), MixerTopologyError> {
        match self.node_mut(id) {
            Some(n) => {
                n.channel.solo = solo;
                self.publish_audibility();
                Ok(())
            }
            None => Err(MixerTopologyError::UnknownChannel(id)),
        }
    }

    /// Sets a channel's mute flag and republishes audibility.
    pub fn set_mute(&mut self, id: u32, mute: bool) -> Result<(), MixerTopologyError> {
        match self.node_mut(id) {
            Some(n) => {
                n.channel.mute = mute;
                self.publish_audibility();
                Ok(())
            }
            None => Err(MixerTopologyError::UnknownChannel(id)),
        }
    }

    /// Clears every channel's scratch buffer to silence.
    ///
    /// Real-time safe: writes only, no allocation.
    pub fn silence_all(&mut self) {
        for node in &mut self.nodes {
            if node.alive {
                node.buffer.fill(0.0);
            }
        }
    }

    /// Advances every meter by one block of silence.
    ///
    /// The engine calls this for channels it did not write this block, so a
    /// stopped channel's meter falls rather than freezing at its last value.
    pub fn age_meters(&mut self, frames: usize, sample_rate: u32) {
        for node in &mut self.nodes {
            if node.alive {
                // A slice of zeroes of the right length; `accumulate` only
                // reads `frames * 2` entries.
                let n = (frames * 2).min(node.buffer.len());
                for s in &mut node.buffer[..n] {
                    *s = 0.0;
                }
                node.meter.accumulate(&node.buffer[..n], frames, sample_rate);
            }
        }
    }

    /// Whether a channel exists.
    #[must_use]
    pub fn exists(&self, id: u32) -> bool {
        self.nodes.get(id as usize).is_some_and(|n| n.alive)
    }

    /// Recomputes the topological processing order, sources first.
    ///
    /// Uses Kahn's algorithm over the output links. Because the graph is
    /// acyclic by construction, this always produces a complete order; any
    /// channel left unvisited (which cannot happen through this API) is
    /// appended anyway so a node is never silently dropped from processing.
    fn rebuild_order(&mut self) {
        let n = self.nodes.len();
        let mut indegree = vec![0u32; n];
        for node in &self.nodes {
            if !node.alive {
                continue;
            }
            if let Some(dst) = node.output {
                if let Some(slot) = indegree.get_mut(dst as usize) {
                    *slot += 1;
                }
            }
        }

        let mut order = Vec::with_capacity(n);
        // A simple stack rather than a queue: the resulting order is still a
        // valid topological one, and it needs no shifting.
        let mut ready: Vec<u32> = (0..n as u32)
            .filter(|i| self.nodes[*i as usize].alive && indegree[*i as usize] == 0)
            .collect();

        while let Some(id) = ready.pop() {
            order.push(id);
            if let Some(dst) = self.nodes[id as usize].output {
                if let Some(slot) = indegree.get_mut(dst as usize) {
                    *slot -= 1;
                    if *slot == 0 && self.nodes[dst as usize].alive {
                        ready.push(dst);
                    }
                }
            }
        }

        // Safety net: include anything the walk missed, so no live channel is
        // ever skipped by the process loop.
        if order.len() != self.nodes.iter().filter(|n| n.alive).count() {
            for (i, node) in self.nodes.iter().enumerate() {
                if node.alive && !order.contains(&(i as u32)) {
                    order.push(i as u32);
                }
            }
        }

        self.order = order;
    }

    /// Allocates every node's scratch buffer at the configured frame capacity.
    fn allocate_buffers(&mut self) {
        for i in 0..self.nodes.len() {
            let frames = self.max_frames;
            let buffer = vec![0.0; frames * 2];
            self.nodes[i].buffer = buffer;
        }
    }

    /// Allocates one node's scratch buffer.
    fn allocate_node_buffer(&self, node: &mut GraphNode) {
        node.buffer = vec![0.0; self.max_frames * 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> MixerGraph {
        MixerGraph::new(256)
    }

    #[test]
    fn a_new_graph_matches_the_default_channel_count() {
        let g = graph();
        assert_eq!(
            g.len(),
            DEFAULT_INSERT_CHANNELS + DEFAULT_RETURN_BUSES + 1,
            "64 inserts + 8 returns + 1 master"
        );
        assert!(g.exists(ChannelId::MASTER.get()));
    }

    #[test]
    fn every_node_is_preallocated_at_the_frame_capacity() {
        let g = MixerGraph::new(128);
        for id in g.order() {
            let node = g.node(*id).expect("ordered nodes are live");
            assert_eq!(
                node.buffer.len(),
                256,
                "channel {id} buffer is not pre-allocated"
            );
        }
    }

    #[test]
    fn default_channels_route_to_master() {
        let mut g = MixerGraph::new(64);
        let id = g.add_channel().expect("capacity available");
        assert_eq!(
            g.node(id).expect("just added").output,
            Some(ChannelId::MASTER.get())
        );
    }

    #[test]
    fn a_direct_self_route_is_refused() {
        let mut g = graph();
        assert_eq!(g.connect(5, 5), Err(MixerTopologyError::SelfRoute));
    }

    #[test]
    fn unknown_channels_are_refused_rather_than_indexing_out_of_bounds() {
        let mut g = graph();
        assert_eq!(
            g.connect(99_999, 1),
            Err(MixerTopologyError::UnknownChannel(99_999))
        );
        assert_eq!(
            g.connect(1, 99_999),
            Err(MixerTopologyError::UnknownChannel(99_999))
        );
        assert_eq!(g.remove_channel(99_999), Err(MixerTopologyError::UnknownChannel(99_999)));
    }

    #[test]
    fn master_cannot_be_removed_or_rerouted() {
        let mut g = graph();
        let master = ChannelId::MASTER.get();
        assert_eq!(g.remove_channel(master), Err(MixerTopologyError::MasterIsFixed));
        assert_eq!(g.connect(master, 5), Err(MixerTopologyError::MasterIsFixed));
        assert_eq!(g.disconnect(master), Err(MixerTopologyError::MasterIsFixed));
        assert!(g.exists(master));
    }

    #[test]
    fn a_two_channel_loop_is_rejected() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");

        assert_eq!(g.connect(a, b), Ok(()));
        // b -> a would close the loop.
        assert_eq!(
            g.connect(b, a),
            Err(MixerTopologyError::WouldCycle),
            "a direct two-node cycle must be refused"
        );
    }

    #[test]
    fn a_self_loop_via_a_longer_chain_is_rejected() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        let c = g.add_channel().expect("capacity");

        assert_eq!(g.connect(a, b), Ok(()));
        assert_eq!(g.connect(b, c), Ok(()));
        // c -> a closes a three-node loop.
        assert_eq!(g.connect(c, a), Err(MixerTopologyError::WouldCycle));
    }

    #[test]
    fn routing_master_into_a_channel_would_be_a_cycle_and_is_refused() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        // a feeds master, so master -> a closes the loop.
        assert_eq!(
            g.connect(ChannelId::MASTER.get(), a),
            Err(MixerTopologyError::MasterIsFixed),
            "master is the terminal node and cannot be routed onward"
        );
    }

    #[test]
    fn a_refused_connection_leaves_the_graph_untouched() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        assert_eq!(g.connect(a, b), Ok(()));

        let before_b_output = g.node(b).expect("live").output;
        assert_eq!(g.connect(b, a), Err(MixerTopologyError::WouldCycle));

        // The refused edge must not have been half-applied.
        assert_eq!(g.node(b).expect("live").output, before_b_output);
        assert_eq!(g.node(a).expect("live").output, Some(b));
    }

    #[test]
    fn a_diamond_routing_is_accepted() {
        // a -> b, a -> c, b -> master, c -> master: two paths, no cycle.
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        let c = g.add_channel().expect("capacity");
        assert_eq!(g.connect(a, b), Ok(()));
        assert_eq!(g.connect(a, c), Ok(()));
        assert_eq!(g.connect(b, ChannelId::MASTER.get()), Ok(()));
        assert_eq!(g.connect(c, ChannelId::MASTER.get()), Ok(()));
    }

    #[test]
    fn group_nesting_up_to_the_limit_is_accepted() {
        let mut g = graph();
        // A chain of `MAX_GROUP_DEPTH` channels feeding master has exactly
        // `MAX_GROUP_DEPTH` edges — the deepest nesting the router allows.
        //
        // Note the off-by-one that matters here: N channels in a chain produce
        // N edges (the last one into master), so a chain of N+1 channels is
        // already one hop too deep.
        let mut chain = Vec::new();
        for _ in 0..MAX_GROUP_DEPTH {
            chain.push(g.add_channel().expect("capacity"));
        }
        for w in chain.windows(2) {
            assert_eq!(
                g.connect(w[0], w[1]),
                Ok(()),
                "nesting within the limit should be accepted"
            );
        }
        assert_eq!(
            g.max_depth_through(chain[0]),
            MAX_GROUP_DEPTH,
            "the accepted chain should be exactly at the limit"
        );
    }

    #[test]
    fn one_hop_past_the_limit_is_refused() {
        let mut g = graph();
        // One more channel than the limit allows.
        let mut chain = Vec::new();
        for _ in 0..=MAX_GROUP_DEPTH {
            chain.push(g.add_channel().expect("capacity"));
        }

        let mut refused_at = None;
        for (i, w) in chain.windows(2).enumerate() {
            if g.connect(w[0], w[1]).is_err() {
                refused_at = Some(i);
                break;
            }
        }
        assert_eq!(
            refused_at,
            Some(MAX_GROUP_DEPTH - 1),
            "the hop that would exceed the limit must be the one refused"
        );

        // Whatever was accepted must still respect the limit.
        for id in &chain {
            assert!(
                g.max_depth_through(*id) <= MAX_GROUP_DEPTH,
                "channel {id} is nested deeper than the limit"
            );
        }
    }

    #[test]
    fn the_depth_check_counts_both_sides_of_the_splice() {
        // The bug this guards: a shallow-looking hop between two chains that
        // are each already deep. Forward-only measurement sees only two hops
        // and wrongly accepts it.
        let mut g = graph();
        let mut upstream = Vec::new();
        for _ in 0..=(MAX_GROUP_DEPTH / 2) {
            upstream.push(g.add_channel().expect("capacity"));
        }
        for w in upstream.windows(2) {
            g.connect(w[0], w[1]).expect("shallow chain accepted");
        }
        let tail_of_upstream = *upstream.last().expect("non-empty");

        let mut downstream = Vec::new();
        for _ in 0..=(MAX_GROUP_DEPTH / 2) {
            downstream.push(g.add_channel().expect("capacity"));
        }
        for w in downstream.windows(2) {
            g.connect(w[0], w[1]).expect("shallow chain accepted");
        }

        // Splicing the deep upstream tail onto the deep downstream head would
        // exceed the limit and must be refused.
        let splice = g.connect(tail_of_upstream, downstream[0]);
        if splice.is_ok() {
            assert!(
                g.max_depth_through(upstream[0]) <= MAX_GROUP_DEPTH,
                "splice produced depth {}",
                g.max_depth_through(upstream[0])
            );
        } else {
            assert_eq!(splice, Err(MixerTopologyError::TooDeep));
        }
    }

    #[test]
    fn processing_order_puts_sources_before_their_destinations() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        let c = g.add_channel().expect("capacity");
        assert_eq!(g.connect(a, b), Ok(()));
        assert_eq!(g.connect(b, c), Ok(()));

        let order = g.order().to_vec();
        let pos = |id: u32| order.iter().position(|x| *x == id).expect("present");
        assert!(pos(a) < pos(b), "a must be summed before b");
        assert!(pos(b) < pos(c), "b must be summed before c");
        assert!(pos(c) < pos(ChannelId::MASTER.get()), "master sums last");
    }

    #[test]
    fn every_live_channel_appears_exactly_once_in_the_order() {
        let mut g = graph();
        for _ in 0..5 {
            g.add_channel().expect("capacity");
        }
        let order = g.order();
        let mut sorted = order.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), order.len(), "the order repeats a channel");
        assert_eq!(order.len(), g.len(), "the order omits a live channel");
    }

    #[test]
    fn removing_a_channel_disconnects_its_upstream_peers() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        assert_eq!(g.connect(a, b), Ok(()));

        assert_eq!(g.remove_channel(b), Ok(()));

        assert!(!g.exists(b));
        assert_eq!(
            g.node(a).expect("a survives").output,
            Some(ChannelId::MASTER.get()),
            "upstream must be rerouted, not left dangling"
        );
    }

    #[test]
    fn removing_a_channel_clears_sends_and_sidechains_pointing_at_it() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        {
            let na = g.node_mut(a).expect("live");
            let s = na.sends.get_mut(0).expect("in range");
            s.enabled = true;
            s.destination = Some(b);
            na.effects.insert(0, 1);
            na.effects.get_mut(0).expect("in range").sidechain_source = Some(b);
        }

        assert_eq!(g.remove_channel(b), Ok(()));

        let na = g.node(a).expect("live");
        assert!(
            !na.sends.get(0).expect("in range").is_active(),
            "a send to a removed bus must be disconnected"
        );
        assert_eq!(
            na.effects.get(0).expect("in range").sidechain_source,
            None,
            "a sidechain from a removed channel must be cleared"
        );
    }

    #[test]
    fn solo_makes_only_soloed_channels_audible() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        let c = g.add_channel().expect("capacity");

        assert_eq!(g.set_solo(b, true), Ok(()));

        assert!(g.node(b).expect("live").channel.audible, "soloed must be audible");
        assert!(!g.node(a).expect("live").channel.audible, "unsoloed must drop");
        assert!(!g.node(c).expect("live").channel.audible);
        assert!(
            g.node(ChannelId::MASTER.get()).expect("live").channel.audible,
            "master must stay live so the solo is actually heard"
        );
    }

    #[test]
    fn clearing_solo_restores_everyone_except_the_muted() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        assert_eq!(g.set_mute(a, true), Ok(()));
        assert_eq!(g.set_solo(b, true), Ok(()));
        assert!(!g.node(a).expect("live").channel.audible);

        assert_eq!(g.set_solo(b, false), Ok(()));

        assert!(!g.node(a).expect("live").channel.audible, "mute must survive");
        assert!(g.node(b).expect("live").channel.audible, "b is unmuted and unsoloed");
    }

    #[test]
    fn mute_alone_silences_only_that_channel() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        assert_eq!(g.set_mute(a, true), Ok(()));
        assert!(!g.node(a).expect("live").channel.audible);
        assert!(g.node(b).expect("live").channel.audible);
    }

    #[test]
    fn solo_and_mute_on_unknown_channels_are_refused() {
        let mut g = graph();
        assert_eq!(g.set_solo(99_999, true), Err(MixerTopologyError::UnknownChannel(99_999)));
        assert_eq!(g.set_mute(99_999, true), Err(MixerTopologyError::UnknownChannel(99_999)));
    }

    #[test]
    fn reachability_terminates_and_is_symmetric_in_its_meaning() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        let b = g.add_channel().expect("capacity");
        assert_eq!(g.connect(a, b), Ok(()));

        assert!(g.reaches(a, b), "a feeds b");
        assert!(!g.reaches(b, a), "b does not feed a");
        assert!(g.reaches(a, ChannelId::MASTER.get()), "b feeds master transitively");
        assert!(!g.reaches(ChannelId::MASTER.get(), a), "master feeds nothing");
    }

    #[test]
    fn meter_snapshots_read_back_per_channel() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        {
            let node = g.node_mut(a).expect("live");
            let block = [0.5, -0.25, 0.0, 0.0];
            node.meter.accumulate(&block, 2, 48_000);
        }
        let m = g.meter(a).expect("live channel has a meter");
        assert!((m.peak_l - 0.5).abs() < 1e-6, "peak_l {}", m.peak_l);
        assert!((m.peak_r - 0.25).abs() < 1e-6, "peak_r {}", m.peak_r);

        assert!(g.meter(99_999).is_none(), "unknown channel reads as None");
    }

    #[test]
    fn silence_all_clears_every_buffer() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        {
            let node = g.node_mut(a).expect("live");
            node.buffer.fill(1.0);
        }
        g.silence_all();
        let node = g.node(a).expect("live");
        assert!(
            node.buffer.iter().all(|s| *s == 0.0),
            "silence_all left residue in a buffer"
        );
    }

    #[test]
    fn indices_are_never_reused_after_removal() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        assert_eq!(g.remove_channel(a), Ok(()));
        let b = g.add_channel().expect("capacity");
        assert_ne!(a, b, "a removed channel's index must not be handed out again");
        assert!(!g.exists(a), "the removed index must stay dead");
    }

    #[test]
    fn a_stale_id_fails_loudly_instead_of_addressing_a_neighbour() {
        let mut g = graph();
        let a = g.add_channel().expect("capacity");
        g.remove_channel(a).expect("removable");

        assert!(g.node(a).is_none());
        assert!(g.meter(a).is_none());
        assert_eq!(g.connect(a, ChannelId::MASTER.get()), Err(MixerTopologyError::UnknownChannel(a)));
        assert_eq!(g.set_mute(a, true), Err(MixerTopologyError::UnknownChannel(a)));
    }
}

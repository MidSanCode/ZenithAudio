//! A serial chain of [`DspNode`]s with a shared, preallocated planar buffer.
//!
//! The engine's master chain is a [`DspGraph`]. It is a serial chain rather
//! than a routed graph because S1's acceptance is *sample-accurate series
//! processing* on the master bus, and the routed topology is already provided by
//! the mixer. If a later stage needs a real graph here, this is the seam to grow.
//!
//! # Real-time discipline
//!
//! [`DspGraph::prepare`] owns every allocation. [`DspGraph::process`] walks a
//! fixed `Vec` of nodes over one preallocated planar scratch buffer, so it
//! allocates nothing and never rebuilds a per-channel slice array.

use super::node::DspNode;
use super::render_context::RenderContext;

/// Why a graph edit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphError {
    /// The graph is full; it never grows on the audio thread.
    Capacity,
    /// The node index does not exist.
    UnknownNode(usize),
}

/// A serial DSP chain.
pub struct DspGraph {
    /// Nodes, in processing order.
    nodes: alloc::vec::Vec<alloc::boxed::Box<dyn DspNode>>,
    /// Planar scratch, `channels * capacity` samples, channel-major.
    scratch: alloc::vec::Vec<f32>,
    /// Number of channels the scratch is sized for.
    channels: usize,
    /// Frames the scratch is sized for.
    capacity: usize,
    /// Sample rate last supplied to `prepare`, used to prepare late additions.
    sample_rate_hint: f32,
    /// Whether prepare has run.
    prepared: bool,
}

impl Default for DspGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl DspGraph {
    /// Creates an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: alloc::vec::Vec::new(),
            scratch: alloc::vec::Vec::new(),
            channels: 0,
            capacity: 0,
            sample_rate_hint: 48_000.0,
            prepared: false,
        }
    }

    /// Whether the graph holds no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Number of nodes in the chain.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether prepare has run.
    #[must_use]
    pub const fn is_prepared(&self) -> bool {
        self.prepared
    }

    /// Appends a node, preparing it for the recorded shape.
    ///
    /// Control thread only. The graph never grows on the audio thread.
    pub fn push(&mut self, mut node: alloc::boxed::Box<dyn DspNode>) -> Result<(), GraphError> {
        if self.prepared {
            node.prepare(self.sample_rate_hint, self.capacity, self.channels);
        }
        self.nodes.push(node);
        Ok(())
    }

    /// Removes the node at `index`.
    pub fn remove(&mut self, index: usize) -> Result<(), GraphError> {
        if index >= self.nodes.len() {
            return Err(GraphError::UnknownNode(index));
        }
        self.nodes.remove(index);
        Ok(())
    }

    /// Allocates the shared scratch and prepares every node.
    pub fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sample_rate_hint = sample_rate;
        self.channels = channels;
        self.capacity = max_block;
        self.scratch = alloc::vec![0.0; channels * max_block];
        for node in &mut self.nodes {
            node.prepare(sample_rate, max_block, channels);
        }
        self.prepared = true;
    }

    /// Clears every node's state.
    pub fn reset(&mut self) {
        for node in &mut self.nodes {
            node.reset();
        }
    }

    /// Total latency of the chain, in samples.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        self.nodes.iter().map(|n| n.latency_samples()).sum()
    }

    /// Runs the chain over `buffer`, which holds `frames * 2` interleaved
    /// samples.
    ///
    /// The buffer is de-interleaved into the graph's planar scratch, each node
    /// runs, and the result is re-interleaved. This is the only layout
    /// conversion in the engine, so nodes only ever see planar blocks.
    ///
    /// Real-time safe.
    pub fn process(
        &mut self,
        buffer: &mut crate::effects::buffer::AudioBuffer<'_>,
        ctx: &RenderContext,
    ) {
        if self.nodes.is_empty() {
            return;
        }
        let frames = buffer.frames().min(self.capacity);
        let channels = buffer.channel_count().min(self.channels);
        if frames == 0 || channels == 0 {
            return;
        }

        let stride = self.capacity;
        for c in 0..channels {
            if let Some(src) = buffer.channel(c) {
                let base = c * stride;
                let n = frames.min(src.len());
                self.scratch[base..base + n].copy_from_slice(&src[..n]);
            }
        }

        for node in &mut self.nodes {
            node.process(self.scratch.as_mut_slice(), stride, channels, frames, ctx);
        }

        for c in 0..channels {
            let base = c * stride;
            if let Some(dst) = buffer.channel_mut(c) {
                let n = frames.min(dst.len());
                dst[..n].copy_from_slice(&self.scratch[base..base + n]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::buffer::AudioBuffer;

    /// A node that adds a constant, so a chain's ordering is observable.
    struct AddNode {
        amount: f32,
    }

    impl DspNode for AddNode {
        fn name(&self) -> &'static str {
            "add"
        }
        fn prepare(&mut self, _sr: f32, _max: usize, _ch: usize) {}
        fn process(
            &mut self,
            planar: &mut [f32],
            stride: usize,
            channels: usize,
            frames: usize,
            _ctx: &RenderContext,
        ) {
            for c in 0..channels {
                let base = c * stride;
                for s in &mut planar[base..base + frames] {
                    *s += self.amount;
                }
            }
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn an_empty_graph_is_a_pass_through() {
        let mut g = DspGraph::new();
        assert!(g.is_empty());
        let mut left = [1.0f32, 2.0];
        let mut right = [3.0f32, 4.0];
        let mut views: [&mut [f32]; 2] = [&mut left, &mut right];
        let mut buf = AudioBuffer::new(&mut views);
        g.process(&mut buf, &RenderContext::default());
        assert_eq!(left, [1.0, 2.0]);
    }

    #[test]
    fn nodes_run_in_push_order() {
        let mut g = DspGraph::new();
        g.prepare(48_000.0, 64, 2);
        g.push(alloc::boxed::Box::new(AddNode { amount: 1.0 })).unwrap();
        g.push(alloc::boxed::Box::new(AddNode { amount: 10.0 }))
            .unwrap();

        let mut left = [0.0f32; 2];
        let mut right = [0.0f32; 2];
        let mut views: [&mut [f32]; 2] = [&mut left, &mut right];
        let mut buf = AudioBuffer::new(&mut views);
        g.process(&mut buf, &RenderContext::default());
        assert_eq!(left, [11.0, 11.0], "order should be 0 + 1 + 10");
    }

    #[test]
    fn latency_is_the_sum_of_the_nodes() {
        struct Lat {
            n: usize,
        }
        impl DspNode for Lat {
            fn name(&self) -> &'static str {
                "lat"
            }
            fn prepare(&mut self, _sr: f32, _m: usize, _c: usize) {}
            fn process(
                &mut self,
                _p: &mut [f32],
                _s: usize,
                _c: usize,
                _f: usize,
                _x: &RenderContext,
            ) {
            }
            fn reset(&mut self) {}
            fn latency_samples(&self) -> usize {
                self.n
            }
        }
        let mut g = DspGraph::new();
        g.push(alloc::boxed::Box::new(Lat { n: 3 })).unwrap();
        g.push(alloc::boxed::Box::new(Lat { n: 5 })).unwrap();
        assert_eq!(g.latency_samples(), 8);
    }

    #[test]
    fn removing_an_unknown_node_is_refused() {
        let mut g = DspGraph::new();
        assert_eq!(g.remove(0), Err(GraphError::UnknownNode(0)));
    }
}

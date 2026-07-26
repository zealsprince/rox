//! The processing chain (ADR 19): DSP on the decode thread, after the stereo
//! fold and resample, immediately before the push into the sample ring. The
//! RT callback is untouched by it, keeping exactly its two jobs: draining
//! the ring and applying user volume. The chain runs at the device rate, so
//! nodes see one stable rate for the life of the stream; the events that
//! change it (a device rebuild) rebuild the stream and reset the chain with
//! the resampler.
//!
//! The bypass rule, which makes bit-perfect checkable: with the chain empty,
//! the samples pushed into the ring are the decoder's output unchanged.

/// One DSP node. The contract (ADR 19): process an interleaved stereo f32
/// buffer in place, same length out as in, at the rate given by the last
/// reset. Allocate at construction and reset, never in process. Nodes are
/// zero-latency by contract: anything that needs lookahead or introduces
/// group delay (convolution, a limiter) stays out until a latency-reporting
/// extension is worth designing, and the position clock stays honest.
///
/// Parameters are atomics shared with the UI, owned by the node, so a knob
/// write is a store with no command round trip; structural edits (adding,
/// removing, reordering nodes) ride the engine's command channel.
pub trait Node: Send {
    /// Called at stream open and on every discontinuity the engine already
    /// knows, the seek flush and the device rebuild, and never at the
    /// gapless boundary, so filter history carries across a track splice.
    fn reset(&mut self, rate: u32);
    /// Process one buffer of interleaved stereo in place.
    fn process(&mut self, buf: &mut [f32]);
}

/// The chain of nodes the decoded stream passes through, in order. Owned by
/// the decode thread; never touched from the RT callback.
pub struct Chain {
    nodes: Vec<Box<dyn Node>>,
    /// The rate handed to the last reset, so a node added mid-stream can be
    /// reset to it on arrival.
    rate: u32,
}

impl Chain {
    pub fn new() -> Self {
        Chain {
            nodes: Vec::new(),
            rate: 0,
        }
    }

    /// Reset every node to `rate`. Stream open, seek flush, device rebuild.
    pub fn reset(&mut self, rate: u32) {
        self.rate = rate;
        for node in &mut self.nodes {
            node.reset(rate);
        }
    }

    /// Append a node, resetting it to the chain's rate on the way in; a
    /// structural edit is a discontinuity for the arriving node alone.
    pub fn push(&mut self, mut node: Box<dyn Node>) {
        node.reset(self.rate);
        self.nodes.push(node);
    }

    /// Run the buffer through every node in order. Empty chain, untouched
    /// buffer; that is the bypass rule, held structurally rather than by a
    /// flag.
    pub fn process(&mut self, buf: &mut [f32]) {
        for node in &mut self.nodes {
            node.process(buf);
        }
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the last reset rate and applies gain + offset, enough to
    /// observe ordering and reset propagation.
    struct Affine {
        gain: f32,
        offset: f32,
        rate: u32,
    }

    impl Node for Affine {
        fn reset(&mut self, rate: u32) {
            self.rate = rate;
        }
        fn process(&mut self, buf: &mut [f32]) {
            for s in buf {
                *s = *s * self.gain + self.offset;
            }
        }
    }

    #[test]
    fn empty_chain_is_bit_exact_passthrough() {
        let mut chain = Chain::new();
        chain.reset(48000);
        let original = vec![0.1f32, -0.5, 1.0, f32::MIN_POSITIVE];
        let mut buf = original.clone();
        chain.process(&mut buf);
        assert_eq!(buf, original, "bypass rule: empty chain changes nothing");
    }

    #[test]
    fn nodes_process_in_chain_order() {
        let mut chain = Chain::new();
        chain.reset(48000);
        // (x + 1) then (x * 2): order matters, 0.0 -> 2.0 not 1.0.
        chain.push(Box::new(Affine { gain: 1.0, offset: 1.0, rate: 0 }));
        chain.push(Box::new(Affine { gain: 2.0, offset: 0.0, rate: 0 }));
        let mut buf = vec![0.0f32, 0.5];
        chain.process(&mut buf);
        assert_eq!(buf, vec![2.0, 3.0]);
    }

    #[test]
    fn push_resets_arriving_node_to_chain_rate() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        /// Publishes its reset rate, the shape a real node's shared
        /// parameter atomics take.
        struct RateProbe(Arc<AtomicU32>);
        impl Node for RateProbe {
            fn reset(&mut self, rate: u32) {
                self.0.store(rate, Ordering::Relaxed);
            }
            fn process(&mut self, _buf: &mut [f32]) {}
        }

        let seen = Arc::new(AtomicU32::new(0));
        let mut chain = Chain::new();
        chain.reset(44100);
        // Arrives after the stream opened; must be told the live rate.
        chain.push(Box::new(RateProbe(seen.clone())));
        assert_eq!(seen.load(Ordering::Relaxed), 44100);
        chain.reset(96000);
        assert_eq!(seen.load(Ordering::Relaxed), 96000);
    }
}

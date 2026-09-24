//! The processing chain (ADR 19): DSP on the decode thread, after the stereo
//! fold and resample, immediately before the ring push. The RT callback never
//! runs it. Nodes see the device rate, reset with the resampler.
//!
//! Bypass rule: with the chain empty, the ring gets the decoder's output unchanged.

/// One DSP node (ADR 19): process interleaved stereo in place, same length,
/// at the rate of the last reset. Allocate at construction and reset, never in
/// `process`. Zero-latency only: no lookahead or group delay, or the position
/// clock lies.
///
/// Parameters are node-owned atomics shared with the UI; structural edits go
/// over the engine's command channel.
pub trait Node: Send {
    /// Called at stream open, seek flush, and device rebuild, never at the
    /// gapless boundary, so filter history carries across a track splice.
    fn reset(&mut self, rate: u32);
    fn process(&mut self, buf: &mut [f32]);
}

/// Owned by the decode thread; never touched from the RT callback.
pub struct Chain {
    nodes: Vec<Box<dyn Node>>,
    /// The last reset's rate, handed to a node added mid-stream.
    rate: u32,
}

impl Chain {
    pub fn new() -> Self {
        Chain {
            nodes: Vec::new(),
            rate: 0,
        }
    }

    pub fn reset(&mut self, rate: u32) {
        self.rate = rate;
        for node in &mut self.nodes {
            node.reset(rate);
        }
    }

    pub fn push(&mut self, mut node: Box<dyn Node>) {
        node.reset(self.rate);
        self.nodes.push(node);
    }

    /// An empty chain leaves the buffer untouched: the bypass rule, held
    /// structurally.
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
        chain.push(Box::new(Affine {
            gain: 1.0,
            offset: 1.0,
            rate: 0,
        }));
        chain.push(Box::new(Affine {
            gain: 2.0,
            offset: 0.0,
            rate: 0,
        }));
        let mut buf = vec![0.0f32, 0.5];
        chain.process(&mut buf);
        assert_eq!(buf, vec![2.0, 3.0]);
    }

    #[test]
    fn push_resets_arriving_node_to_chain_rate() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

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

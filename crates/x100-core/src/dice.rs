//! Randomness the daemon's decisions use, such as a new agent's look. Like the clock it is a trait,
//! so a test or a scripted run fixes it with a seed and gets the same choices every time. The
//! operating system's randomness is I/O, so it lives with the callers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub trait Dice: Send + Sync {
    /// Any number, uniformly.
    fn roll(&self) -> u64;
}

/// The same numbers from the same seed, in the order they are asked for. Clones share the
/// sequence.
#[derive(Debug, Clone)]
pub struct SeededDice {
    state: Arc<AtomicU64>,
}

impl SeededDice {
    pub fn new(seed: u64) -> Self {
        Self {
            state: Arc::new(AtomicU64::new(seed)),
        }
    }
}

/// SplitMix64's increment, the fractional part of the golden ratio.
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

impl Dice for SeededDice {
    /// SplitMix64: a counter stepped by one atomic add, then mixed, so concurrent callers each
    /// get a distinct number of the sequence.
    fn roll(&self) -> u64 {
        let mut z = self
            .state
            .fetch_add(GAMMA, Ordering::Relaxed)
            .wrapping_add(GAMMA);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_gives_the_same_sequence_and_another_seed_another() {
        let roll = |seed| {
            let d = SeededDice::new(seed);
            (0..8).map(|_| d.roll()).collect::<Vec<_>>()
        };
        assert_eq!(roll(7), roll(7));
        assert_ne!(roll(7), roll(8));
        let first = roll(7);
        assert!(first.windows(2).all(|w| w[0] != w[1]), "{first:?}");
    }
}

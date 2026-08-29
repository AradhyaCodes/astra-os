//! Page-replacement policies.
//!
//! The [`super::MemoryManager`] fault path is identical for every policy; only
//! the victim choice differs, so each policy is a small independent type
//! implementing [`PageReplacementStrategy`] rather than a branch in the fault
//! handler.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The two supported page-replacement policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ReplacementPolicy {
    /// First-In, First-Out — evict the frame loaded longest ago.
    Fifo,
    /// Least Recently Used — evict the frame whose page was accessed longest ago.
    Lru,
}

impl ReplacementPolicy {
    pub fn label(self) -> &'static str {
        match self {
            ReplacementPolicy::Fifo => "FIFO",
            ReplacementPolicy::Lru => "LRU",
        }
    }
}

/// Parse a user-supplied policy name (`FIFO` / `LRU`).
pub fn parse_replacement_policy(raw: &str) -> Option<ReplacementPolicy> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fifo" | "first-in-first-out" => Some(ReplacementPolicy::Fifo),
        "lru" | "least-recently-used" => Some(ReplacementPolicy::Lru),
        _ => None,
    }
}

/// A pluggable victim-selection policy over the set of occupied frames.
pub trait PageReplacementStrategy: Send + Sync + std::fmt::Debug {
    fn policy(&self) -> ReplacementPolicy;

    /// A frame just became resident at simulation `clock`.
    fn on_load(&mut self, frame: usize, clock: u64);

    /// The page in `frame` was accessed at simulation `clock`.
    fn on_access(&mut self, frame: usize, clock: u64);

    /// `frame` was released (process exit or eviction).
    fn on_release(&mut self, frame: usize);

    /// Pick a frame to evict from `candidates` (all currently occupied frames).
    fn choose_victim(&self, candidates: &[usize]) -> Option<usize>;
}

/// FIFO — track load order, evict the oldest.
#[derive(Debug, Default)]
pub struct FifoPolicy {
    order: VecDeque<usize>,
}

impl PageReplacementStrategy for FifoPolicy {
    fn policy(&self) -> ReplacementPolicy {
        ReplacementPolicy::Fifo
    }

    fn on_load(&mut self, frame: usize, _clock: u64) {
        self.order.retain(|existing| *existing != frame);
        self.order.push_back(frame);
    }

    fn on_access(&mut self, _frame: usize, _clock: u64) {
        // FIFO does not care about accesses.
    }

    fn on_release(&mut self, frame: usize) {
        self.order.retain(|existing| *existing != frame);
    }

    fn choose_victim(&self, candidates: &[usize]) -> Option<usize> {
        self.order
            .iter()
            .copied()
            .find(|frame| candidates.contains(frame))
            .or_else(|| candidates.iter().copied().min())
    }
}

/// LRU — track the most recent access per frame, evict the stalest.
#[derive(Debug, Default)]
pub struct LruPolicy {
    last_touch: BTreeMap<usize, u64>,
}

impl PageReplacementStrategy for LruPolicy {
    fn policy(&self) -> ReplacementPolicy {
        ReplacementPolicy::Lru
    }

    fn on_load(&mut self, frame: usize, clock: u64) {
        self.last_touch.insert(frame, clock);
    }

    fn on_access(&mut self, frame: usize, clock: u64) {
        self.last_touch.insert(frame, clock);
    }

    fn on_release(&mut self, frame: usize) {
        self.last_touch.remove(&frame);
    }

    fn choose_victim(&self, candidates: &[usize]) -> Option<usize> {
        candidates
            .iter()
            .copied()
            .min_by_key(|frame| self.last_touch.get(frame).copied().unwrap_or(0))
    }
}

/// Build the strategy object for a policy identifier.
pub fn strategy_for(policy: ReplacementPolicy) -> Box<dyn PageReplacementStrategy> {
    match policy {
        ReplacementPolicy::Fifo => Box::<FifoPolicy>::default(),
        ReplacementPolicy::Lru => Box::<LruPolicy>::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_evicts_in_load_order_and_ignores_accesses() {
        let mut fifo = FifoPolicy::default();
        for (clock, frame) in [(1, 5), (2, 9), (3, 2)] {
            fifo.on_load(frame, clock);
        }
        fifo.on_access(5, 10); // must not change the order
        assert_eq!(fifo.choose_victim(&[5, 9, 2]), Some(5));
        fifo.on_release(5);
        assert_eq!(fifo.choose_victim(&[9, 2]), Some(9));
    }

    #[test]
    fn lru_evicts_the_stalest_frame() {
        let mut lru = LruPolicy::default();
        for (clock, frame) in [(1, 5), (2, 9), (3, 2)] {
            lru.on_load(frame, clock);
        }
        // Touch frame 5 so it is no longer the stalest.
        lru.on_access(5, 10);
        assert_eq!(lru.choose_victim(&[5, 9, 2]), Some(9));
    }

    #[test]
    fn policy_names_parse_case_insensitively() {
        assert_eq!(
            parse_replacement_policy("fifo"),
            Some(ReplacementPolicy::Fifo)
        );
        assert_eq!(
            parse_replacement_policy("LRU"),
            Some(ReplacementPolicy::Lru)
        );
        assert_eq!(parse_replacement_policy("nope"), None);
    }
}

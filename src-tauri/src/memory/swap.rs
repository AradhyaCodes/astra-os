//! Simulated swap (backing store) for pages evicted from RAM.

use super::config::SWAP_SLOTS;
use crate::process::Pid;
use serde::{Deserialize, Serialize};

/// The `(process, page)` held in a swap slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapSlot {
    pub pid: Pid,
    pub page: usize,
}

/// A fixed pool of [`SWAP_SLOTS`] backing-store slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapManager {
    slots: Vec<Option<SwapSlot>>,
    used: usize,
}

impl Default for SwapManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SwapManager {
    pub fn new() -> Self {
        Self {
            slots: vec![None; SWAP_SLOTS],
            used: 0,
        }
    }

    pub fn total(&self) -> usize {
        SWAP_SLOTS
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn free(&self) -> usize {
        SWAP_SLOTS - self.used
    }

    /// Write a page to the lowest free swap slot. `None` when swap is full.
    pub fn store(&mut self, slot: SwapSlot) -> Option<usize> {
        let index = self.slots.iter().position(Option::is_none)?;
        self.slots[index] = Some(slot);
        self.used += 1;
        Some(index)
    }

    /// Read a page back and free its slot.
    pub fn load(&mut self, index: usize) -> Option<SwapSlot> {
        let slot = self.slots.get_mut(index)?;
        let previous = slot.take();
        if previous.is_some() {
            self.used -= 1;
        }
        previous
    }

    /// Drop every swapped page belonging to `pid`; returns how many were freed.
    pub fn release_all_owned_by(&mut self, pid: Pid) -> usize {
        let mut freed = 0;
        for slot in self.slots.iter_mut() {
            if slot.is_some_and(|entry| entry.pid == pid) {
                *slot = None;
                self.used -= 1;
                freed += 1;
            }
        }
        freed
    }

    pub fn slots_owned_by(&self, pid: Pid) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.is_some_and(|entry| entry.pid == pid))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_then_load_round_trips_and_frees_the_slot() {
        let mut swap = SwapManager::new();
        let index = swap.store(SwapSlot { pid: 3, page: 9 }).unwrap();
        assert_eq!(swap.used(), 1);
        let restored = swap.load(index).unwrap();
        assert_eq!(restored, SwapSlot { pid: 3, page: 9 });
        assert_eq!(swap.used(), 0);
        assert!(swap.load(index).is_none());
    }

    #[test]
    fn release_all_owned_by_clears_one_process() {
        let mut swap = SwapManager::new();
        swap.store(SwapSlot { pid: 1, page: 0 });
        swap.store(SwapSlot { pid: 2, page: 0 });
        swap.store(SwapSlot { pid: 1, page: 1 });
        assert_eq!(swap.release_all_owned_by(1), 2);
        assert_eq!(swap.used(), 1);
        assert_eq!(swap.slots_owned_by(2), 1);
    }
}

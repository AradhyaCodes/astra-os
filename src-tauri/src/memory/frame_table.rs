//! Global physical frame table for simulated RAM.

use super::config::PHYSICAL_FRAMES;
use crate::process::Pid;
use serde::{Deserialize, Serialize};

/// Which `(process, page)` currently owns a physical frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameOwner {
    pub pid: Pid,
    pub page: usize,
}

/// The fixed array of [`PHYSICAL_FRAMES`] physical frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameTable {
    frames: Vec<Option<FrameOwner>>,
    used: usize,
}

impl Default for FrameTable {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameTable {
    pub fn new() -> Self {
        Self {
            frames: vec![None; PHYSICAL_FRAMES],
            used: 0,
        }
    }

    pub fn total(&self) -> usize {
        PHYSICAL_FRAMES
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn free(&self) -> usize {
        PHYSICAL_FRAMES - self.used
    }

    pub fn owner(&self, frame: usize) -> Option<FrameOwner> {
        self.frames.get(frame).copied().flatten()
    }

    /// Claim the lowest-indexed free frame for `owner`. `None` when RAM is full.
    pub fn allocate(&mut self, owner: FrameOwner) -> Option<usize> {
        let index = self.frames.iter().position(Option::is_none)?;
        self.frames[index] = Some(owner);
        self.used += 1;
        Some(index)
    }

    /// Release one frame, returning its previous owner.
    pub fn release(&mut self, frame: usize) -> Option<FrameOwner> {
        let slot = self.frames.get_mut(frame)?;
        let previous = slot.take();
        if previous.is_some() {
            self.used -= 1;
        }
        previous
    }

    /// `(frame_index, owner)` for every occupied frame, ascending.
    pub fn occupied(&self) -> impl Iterator<Item = (usize, FrameOwner)> + '_ {
        self.frames
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.map(|owner| (index, owner)))
    }

    /// Free every frame owned by `pid`; returns the freed frame indices.
    pub fn release_all_owned_by(&mut self, pid: Pid) -> Vec<usize> {
        let mut freed = Vec::new();
        for (index, slot) in self.frames.iter_mut().enumerate() {
            if slot.is_some_and(|owner| owner.pid == pid) {
                *slot = None;
                self.used -= 1;
                freed.push(index);
            }
        }
        freed
    }

    pub fn frames_owned_by(&self, pid: Pid) -> usize {
        self.frames
            .iter()
            .filter(|slot| slot.is_some_and(|owner| owner.pid == pid))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_uses_the_lowest_free_frame_and_tracks_counts() {
        let mut table = FrameTable::new();
        assert_eq!(table.total(), PHYSICAL_FRAMES);
        assert_eq!(table.free(), PHYSICAL_FRAMES);

        let a = table.allocate(FrameOwner { pid: 1, page: 0 }).unwrap();
        let b = table.allocate(FrameOwner { pid: 1, page: 1 }).unwrap();
        assert_eq!((a, b), (0, 1));
        assert_eq!(table.used(), 2);

        table.release(a);
        assert_eq!(table.used(), 1);
        // The lowest free frame is 0 again.
        assert_eq!(table.allocate(FrameOwner { pid: 2, page: 0 }), Some(0));
        assert_eq!(table.owner(0), Some(FrameOwner { pid: 2, page: 0 }));
    }

    #[test]
    fn release_all_owned_by_clears_one_process() {
        let mut table = FrameTable::new();
        table.allocate(FrameOwner { pid: 1, page: 0 });
        table.allocate(FrameOwner { pid: 2, page: 0 });
        table.allocate(FrameOwner { pid: 1, page: 1 });

        let freed = table.release_all_owned_by(1);
        assert_eq!(freed, vec![0, 2]);
        assert_eq!(table.used(), 1);
        assert_eq!(table.frames_owned_by(1), 0);
        assert_eq!(table.frames_owned_by(2), 1);
    }
}

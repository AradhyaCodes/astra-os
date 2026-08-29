//! Per-process page table: maps each virtual page to its current location.

use crate::process::Pid;
use serde::{Deserialize, Serialize};

/// Where a virtual page currently lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageLocation {
    /// Not backed by anything yet (transient — set during allocation).
    Unloaded,
    /// Resident in physical frame `index`.
    Frame(usize),
    /// Swapped out to backing-store slot `index`.
    Swapped(usize),
}

/// One virtual page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PageEntry {
    pub location: PageLocation,
}

impl PageEntry {
    fn unloaded() -> Self {
        Self {
            location: PageLocation::Unloaded,
        }
    }

    pub fn is_resident(&self) -> bool {
        matches!(self.location, PageLocation::Frame(_))
    }

    pub fn is_swapped(&self) -> bool {
        matches!(self.location, PageLocation::Swapped(_))
    }
}

/// A process's private table of virtual pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageTable {
    pid: Pid,
    pages: Vec<PageEntry>,
    /// Page faults this process has taken (a swapped or unloaded page touched).
    faults: u64,
}

impl PageTable {
    pub fn new(pid: Pid, page_count: usize) -> Self {
        Self {
            pid,
            pages: vec![PageEntry::unloaded(); page_count],
            faults: 0,
        }
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn faults(&self) -> u64 {
        self.faults
    }

    pub fn record_fault(&mut self) {
        self.faults += 1;
    }

    pub fn entry(&self, page: usize) -> Option<&PageEntry> {
        self.pages.get(page)
    }

    pub fn set_location(&mut self, page: usize, location: PageLocation) {
        if let Some(entry) = self.pages.get_mut(page) {
            entry.location = location;
        }
    }

    pub fn resident_count(&self) -> usize {
        self.pages
            .iter()
            .filter(|entry| entry.is_resident())
            .count()
    }

    pub fn swapped_count(&self) -> usize {
        self.pages.iter().filter(|entry| entry.is_swapped()).count()
    }

    /// `(page_index, frame_index)` for every resident page.
    pub fn resident_pages(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.pages.iter().enumerate().filter_map(|(page, entry)| {
            if let PageLocation::Frame(frame) = entry.location {
                Some((page, frame))
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_table_has_every_page_unloaded() {
        let table = PageTable::new(7, 5);
        assert_eq!(table.len(), 5);
        assert_eq!(table.resident_count(), 0);
        assert_eq!(table.swapped_count(), 0);
        assert!(table
            .entry(0)
            .is_some_and(|entry| entry.location == PageLocation::Unloaded));
        assert!(table.entry(5).is_none());
    }

    #[test]
    fn locations_and_fault_counter_update() {
        let mut table = PageTable::new(7, 3);
        table.set_location(0, PageLocation::Frame(12));
        table.set_location(1, PageLocation::Swapped(4));
        table.record_fault();
        assert_eq!(table.resident_count(), 1);
        assert_eq!(table.swapped_count(), 1);
        assert_eq!(table.faults(), 1);
        assert_eq!(table.resident_pages().collect::<Vec<_>>(), vec![(0, 12)]);
    }
}

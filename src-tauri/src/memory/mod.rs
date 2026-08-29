//! Aaru-OS — Simulated virtual memory subsystem (Phase 7).
//!
//! Aaru models a small paged machine: **4096 MB of RAM as 1024 physical frames
//! of 4 MB**, plus a fixed swap area. Aaru-native processes (`AARU_APP`,
//! `AARU_GAME`) request simulated memory; the manager builds a page table, maps
//! pages to frames, and on pressure evicts pages to swap using a pluggable
//! [`replacement::PageReplacementStrategy`] (FIFO or LRU).
//!
//! **This is a simulation and is deliberately isolated from the real machine.**
//! Windows host memory is never fed into the 4096 MB model; if host metrics are
//! available they are reported in their own section ([`HostMemory`]).
//!
//! Components (each its own type, no giant struct/function):
//! * [`MemoryManager`] — orchestration, faults, statistics;
//! * [`page_table::PageTable`] — one process's virtual pages;
//! * [`frame_table::FrameTable`] — the 1024 physical frames;
//! * [`swap::SwapManager`] — the backing store;
//! * [`replacement::PageReplacementStrategy`] — FIFO / LRU victim choice.

pub mod config;
pub mod frame_table;
pub mod page_table;
pub mod replacement;
pub mod swap;

use crate::error::AaruError;
use crate::process::Pid;
use frame_table::FrameTable;
use page_table::PageTable;
use replacement::{strategy_for, PageReplacementStrategy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use swap::{SwapManager, SwapSlot};

pub use config::{resident_mb_for, resident_pages_for, PAGE_SIZE_MB};
pub use frame_table::FrameOwner;
pub use page_table::PageLocation;
pub use replacement::{parse_replacement_policy, ReplacementPolicy};

/// Outcome of touching a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessOutcome {
    /// The page was already resident.
    Hit,
    /// The page was unloaded / swapped and has now been paged in.
    Fault { swapped_in: bool },
}

/// Result of a successful [`MemoryManager::allocate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub pages: usize,
    pub resident: usize,
    pub swapped: usize,
}

// ---------------------------------------------------------------------------
// Serializable snapshot
// ---------------------------------------------------------------------------

/// Simulated per-process memory, for Task Manager / `almanac memory`.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessMemoryView {
    pub pid: Pid,
    pub pages: usize,
    pub resident_pages: usize,
    pub swapped_pages: usize,
    pub resident_mb: u32,
    pub faults: u64,
}

/// A `(pid, frame_count)` span for the aggregated frame visualisation.
#[derive(Debug, Clone, Serialize)]
pub struct FrameSpan {
    pub pid: Option<Pid>,
    pub frames: usize,
}

/// Real Windows physical-memory usage — reported entirely separately from the
/// simulated Aaru RAM model, never mixed into it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct HostMemory {
    pub total_mb: u64,
    pub used_mb: u64,
    pub load_percent: u32,
}

/// Everything the UI needs to render the simulated memory subsystem.
#[derive(Debug, Clone, Serialize)]
pub struct MemorySnapshot {
    pub policy: ReplacementPolicy,
    pub page_size_mb: u32,

    pub ram_total_mb: u32,
    pub ram_used_mb: u32,
    pub ram_free_mb: u32,

    pub frames_total: usize,
    pub frames_used: usize,
    pub frames_free: usize,

    pub swap_total_mb: u32,
    pub swap_used_mb: u32,
    pub swap_free_mb: u32,

    pub page_faults: u64,
    pub page_hits: u64,
    pub swap_ins: u64,
    pub swap_outs: u64,

    pub processes: Vec<ProcessMemoryView>,
    /// Frame occupancy grouped by owner (+ a trailing free span), capped so the
    /// UI never has to draw 1024 elements.
    pub frame_spans: Vec<FrameSpan>,

    /// Real Windows memory, when it could be sampled — shown on its own.
    pub host: Option<HostMemory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRuntimeSnapshot {
    frames: FrameTable,
    swap: SwapManager,
    tables: BTreeMap<Pid, PageTable>,
    policy: ReplacementPolicy,
    clock: u64,
    page_faults: u64,
    page_hits: u64,
    swap_ins: u64,
    swap_outs: u64,
}

// ---------------------------------------------------------------------------
// MemoryManager
// ---------------------------------------------------------------------------

/// Orchestrates the page table set, the frame table, swap, and the replacement
/// policy.
#[derive(Debug)]
pub struct MemoryManager {
    frames: FrameTable,
    swap: SwapManager,
    tables: BTreeMap<Pid, PageTable>,
    policy: Box<dyn PageReplacementStrategy>,
    /// Monotonic simulation counter backing FIFO order and LRU recency.
    clock: u64,
    page_faults: u64,
    page_hits: u64,
    swap_ins: u64,
    swap_outs: u64,
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryManager {
    pub fn new() -> Self {
        Self::with_policy(ReplacementPolicy::Fifo)
    }

    pub fn with_policy(policy: ReplacementPolicy) -> Self {
        Self {
            frames: FrameTable::new(),
            swap: SwapManager::new(),
            tables: BTreeMap::new(),
            policy: strategy_for(policy),
            clock: 0,
            page_faults: 0,
            page_hits: 0,
            swap_ins: 0,
            swap_outs: 0,
        }
    }

    pub fn policy(&self) -> ReplacementPolicy {
        self.policy.policy()
    }

    pub fn runtime_snapshot(&self) -> MemoryRuntimeSnapshot {
        MemoryRuntimeSnapshot {
            frames: self.frames.clone(),
            swap: self.swap.clone(),
            tables: self.tables.clone(),
            policy: self.policy(),
            clock: self.clock,
            page_faults: self.page_faults,
            page_hits: self.page_hits,
            swap_ins: self.swap_ins,
            swap_outs: self.swap_outs,
        }
    }

    pub fn from_runtime_snapshot(snapshot: MemoryRuntimeSnapshot) -> Self {
        let mut policy = strategy_for(snapshot.policy);
        let mut seed = snapshot.clock;
        for (frame, _) in snapshot.frames.occupied() {
            seed = seed.saturating_add(1);
            policy.on_load(frame, seed);
        }
        Self {
            frames: snapshot.frames,
            swap: snapshot.swap,
            tables: snapshot.tables,
            policy,
            clock: seed,
            page_faults: snapshot.page_faults,
            page_hits: snapshot.page_hits,
            swap_ins: snapshot.swap_ins,
            swap_outs: snapshot.swap_outs,
        }
    }

    pub fn is_tracked(&self, pid: Pid) -> bool {
        self.tables.contains_key(&pid)
    }

    /// Current location of one virtual page (for tests / introspection).
    pub fn page_location(&self, pid: Pid, page: usize) -> Option<PageLocation> {
        self.tables
            .get(&pid)
            .and_then(|table| table.entry(page))
            .map(|entry| entry.location)
    }

    /// Owner of a physical frame (for tests / introspection).
    pub fn frame_owner(&self, frame: usize) -> Option<FrameOwner> {
        self.frames.owner(frame)
    }

    pub fn page_faults(&self) -> u64 {
        self.page_faults
    }

    pub fn page_hits(&self) -> u64 {
        self.page_hits
    }

    // ------------------------------------------------------------------
    // Allocation / release
    // ------------------------------------------------------------------

    /// Give `pid` a working set of `pages` pages: fill RAM first, spill the
    /// remainder into swap. Fails cleanly with [`AaruError::OutOfMemory`] (and
    /// no state change) when neither RAM nor swap can hold the request.
    pub fn allocate(&mut self, pid: Pid, pages: usize) -> Result<Allocation, AaruError> {
        if let Some(table) = self.tables.get(&pid) {
            return Ok(Allocation {
                pages: table.len(),
                resident: table.resident_count(),
                swapped: table.swapped_count(),
            });
        }

        let capacity = self.frames.free() + self.swap.free();
        if pages > capacity {
            return Err(AaruError::OutOfMemory {
                requested_mb: u64::from(pages as u32 * PAGE_SIZE_MB),
                available_mb: u64::from(capacity as u32 * PAGE_SIZE_MB),
            });
        }

        let mut table = PageTable::new(pid, pages);
        for page in 0..pages {
            self.clock += 1;
            if let Some(frame) = self.frames.allocate(FrameOwner { pid, page }) {
                table.set_location(page, PageLocation::Frame(frame));
                self.policy.on_load(frame, self.clock);
            } else {
                let slot = self
                    .swap
                    .store(SwapSlot { pid, page })
                    .expect("capacity was checked up front");
                table.set_location(page, PageLocation::Swapped(slot));
                self.swap_outs += 1;
            }
        }

        let allocation = Allocation {
            pages,
            resident: table.resident_count(),
            swapped: table.swapped_count(),
        };
        self.tables.insert(pid, table);
        Ok(allocation)
    }

    /// Release every frame, swap slot and the page table for `pid`. Safe to
    /// call for an unknown PID (host processes never have a table).
    pub fn release(&mut self, pid: Pid) {
        if self.tables.remove(&pid).is_none() {
            return;
        }
        for frame in self.frames.release_all_owned_by(pid) {
            self.policy.on_release(frame);
        }
        self.swap.release_all_owned_by(pid);
    }

    // ------------------------------------------------------------------
    // Access / fault handling
    // ------------------------------------------------------------------

    /// Touch page `page` of process `pid`. Returns whether it was a hit or a
    /// fault (with a page-in). Errors only on an unknown PID / page index, or a
    /// genuine out-of-memory during the page-in — never panics.
    pub fn access(&mut self, pid: Pid, page: usize) -> Result<AccessOutcome, AaruError> {
        let location = {
            let table = self.tables.get(&pid).ok_or_else(|| {
                AaruError::Memory(format!("PID {pid} has no simulated memory allocation"))
            })?;
            table
                .entry(page)
                .ok_or_else(|| {
                    AaruError::InvalidArgument(format!(
                        "PID {pid} has no virtual page {page} (0..{})",
                        table.len()
                    ))
                })?
                .location
        };

        self.clock += 1;
        let clock = self.clock;

        match location {
            PageLocation::Frame(frame) => {
                self.policy.on_access(frame, clock);
                self.page_hits += 1;
                Ok(AccessOutcome::Hit)
            }
            PageLocation::Swapped(slot) => {
                self.page_faults += 1;
                if let Some(table) = self.tables.get_mut(&pid) {
                    table.record_fault();
                }
                // Free the swap slot first so a victim always has somewhere to go.
                let restored = self.swap.load(slot).expect("swapped page has a slot");
                debug_assert_eq!(restored.pid, pid);
                let frame = self.acquire_frame(pid, page)?;
                self.tables
                    .get_mut(&pid)
                    .expect("table still present")
                    .set_location(page, PageLocation::Frame(frame));
                self.policy.on_load(frame, clock);
                self.swap_ins += 1;
                Ok(AccessOutcome::Fault { swapped_in: true })
            }
            PageLocation::Unloaded => {
                self.page_faults += 1;
                if let Some(table) = self.tables.get_mut(&pid) {
                    table.record_fault();
                }
                let frame = self.acquire_frame(pid, page)?;
                self.tables
                    .get_mut(&pid)
                    .expect("table still present")
                    .set_location(page, PageLocation::Frame(frame));
                self.policy.on_load(frame, clock);
                Ok(AccessOutcome::Fault { swapped_in: false })
            }
        }
    }

    /// Deterministic background pressure: each running process touches one of
    /// its pages, cycling through them. Errors are swallowed (a transient OOM
    /// on one tick simply means that page was not paged in).
    pub fn tick_access(&mut self, running: &[Pid]) {
        for &pid in running {
            let Some(pages) = self.tables.get(&pid).map(PageTable::len) else {
                continue;
            };
            if pages == 0 {
                continue;
            }
            let page = (self.clock as usize).wrapping_add(1) % pages;
            let _ = self.access(pid, page);
        }
    }

    /// Obtain a free frame for `(pid, page)`, evicting a victim (swap-out) via
    /// the replacement policy when RAM is full.
    fn acquire_frame(&mut self, pid: Pid, page: usize) -> Result<usize, AaruError> {
        if let Some(frame) = self.frames.allocate(FrameOwner { pid, page }) {
            return Ok(frame);
        }

        let occupied: Vec<usize> = self.frames.occupied().map(|(index, _)| index).collect();
        let victim = self
            .policy
            .choose_victim(&occupied)
            .ok_or(AaruError::OutOfMemory {
                requested_mb: u64::from(PAGE_SIZE_MB),
                available_mb: 0,
            })?;
        let evicted = self
            .frames
            .release(victim)
            .expect("victim frame is occupied");
        self.policy.on_release(victim);

        match self.swap.store(SwapSlot {
            pid: evicted.pid,
            page: evicted.page,
        }) {
            Some(slot) => {
                if let Some(table) = self.tables.get_mut(&evicted.pid) {
                    table.set_location(evicted.page, PageLocation::Swapped(slot));
                }
                self.swap_outs += 1;
            }
            None => {
                // Swap is full — undo the eviction and fail cleanly.
                let frame = self
                    .frames
                    .allocate(FrameOwner {
                        pid: evicted.pid,
                        page: evicted.page,
                    })
                    .expect("frame was just freed");
                if let Some(table) = self.tables.get_mut(&evicted.pid) {
                    table.set_location(evicted.page, PageLocation::Frame(frame));
                }
                self.policy.on_load(frame, self.clock);
                return Err(AaruError::OutOfMemory {
                    requested_mb: u64::from(PAGE_SIZE_MB),
                    available_mb: 0,
                });
            }
        }

        Ok(self
            .frames
            .allocate(FrameOwner { pid, page })
            .expect("a frame was just freed"))
    }

    // ------------------------------------------------------------------
    // Policy switch
    // ------------------------------------------------------------------

    /// Switch the replacement policy. Existing resident frames are re-seeded
    /// into the new policy in ascending frame order (prior load/access history
    /// is not recoverable across a switch).
    pub fn set_policy(&mut self, policy: ReplacementPolicy) {
        if policy == self.policy() {
            return;
        }
        let mut next = strategy_for(policy);
        for (frame, _owner) in self.frames.occupied() {
            self.clock += 1;
            next.on_load(frame, self.clock);
        }
        self.policy = next;
    }

    // ------------------------------------------------------------------
    // Snapshot
    // ------------------------------------------------------------------

    pub fn snapshot(&self) -> MemorySnapshot {
        let frames_used = self.frames.used();
        let swap_used = self.swap.used();

        let mut processes: Vec<ProcessMemoryView> = self
            .tables
            .values()
            .map(|table| ProcessMemoryView {
                pid: table.pid(),
                pages: table.len(),
                resident_pages: table.resident_count(),
                swapped_pages: table.swapped_count(),
                resident_mb: table.len() as u32 * PAGE_SIZE_MB,
                faults: table.faults(),
            })
            .collect();
        processes.sort_by_key(|view| view.pid);

        MemorySnapshot {
            policy: self.policy(),
            page_size_mb: PAGE_SIZE_MB,

            ram_total_mb: config::RAM_MB,
            ram_used_mb: frames_used as u32 * PAGE_SIZE_MB,
            ram_free_mb: self.frames.free() as u32 * PAGE_SIZE_MB,

            frames_total: self.frames.total(),
            frames_used,
            frames_free: self.frames.free(),

            swap_total_mb: config::SWAP_MB,
            swap_used_mb: swap_used as u32 * PAGE_SIZE_MB,
            swap_free_mb: self.swap.free() as u32 * PAGE_SIZE_MB,

            page_faults: self.page_faults,
            page_hits: self.page_hits,
            swap_ins: self.swap_ins,
            swap_outs: self.swap_outs,

            frame_spans: self.frame_spans(),
            processes,

            host: host_memory::sample(),
        }
    }

    /// Frame occupancy grouped by owning PID (ascending), then a free span.
    fn frame_spans(&self) -> Vec<FrameSpan> {
        let mut by_pid: BTreeMap<Pid, usize> = BTreeMap::new();
        for (_frame, owner) in self.frames.occupied() {
            *by_pid.entry(owner.pid).or_default() += 1;
        }
        let mut spans: Vec<FrameSpan> = by_pid
            .into_iter()
            .map(|(pid, frames)| FrameSpan {
                pid: Some(pid),
                frames,
            })
            .collect();
        let free = self.frames.free();
        if free > 0 {
            spans.push(FrameSpan {
                pid: None,
                frames: free,
            });
        }
        spans
    }
}

// ---------------------------------------------------------------------------
// Host memory probe — entirely separate from the simulated model
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod host_memory {
    use super::HostMemory;

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    /// Best-effort read of real Windows physical-memory usage. `None` if the
    /// call fails — this never influences the simulated Aaru RAM model.
    pub fn sample() -> Option<HostMemory> {
        // SAFETY: `status` is a correctly-sized, fully-initialised C struct and
        // `GlobalMemoryStatusEx` only writes into it.
        let mut status: MemoryStatusEx = unsafe { std::mem::zeroed() };
        status.length = std::mem::size_of::<MemoryStatusEx>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return None;
        }
        let total_mb = status.total_phys / (1024 * 1024);
        let used_mb = total_mb.saturating_sub(status.avail_phys / (1024 * 1024));
        Some(HostMemory {
            total_mb,
            used_mb,
            load_percent: status.memory_load,
        })
    }
}

#[cfg(not(windows))]
mod host_memory {
    use super::HostMemory;

    pub fn sample() -> Option<HostMemory> {
        None
    }
}

#[cfg(test)]
mod tests;

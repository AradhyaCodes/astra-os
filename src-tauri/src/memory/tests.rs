//! Deterministic tests for the simulated memory subsystem. Every test drives
//! [`MemoryManager`] directly; nothing depends on wall-clock time.

use super::config::{PHYSICAL_FRAMES, SWAP_SLOTS};
use super::page_table::PageLocation;
use super::replacement::ReplacementPolicy;
use super::{AccessOutcome, MemoryManager};
use crate::error::AaruError;

fn swapped(location: Option<PageLocation>) -> bool {
    matches!(location, Some(PageLocation::Swapped(_)))
}

fn resident(location: Option<PageLocation>) -> bool {
    matches!(location, Some(PageLocation::Frame(_)))
}

#[test]
fn frame_allocation_maps_each_page_to_its_own_frame() {
    let mut memory = MemoryManager::new();
    let allocation = memory.allocate(1, 10).unwrap();
    assert_eq!(allocation.pages, 10);
    assert_eq!(allocation.resident, 10);
    assert_eq!(allocation.swapped, 0);

    let snapshot = memory.snapshot();
    assert_eq!(snapshot.frames_used, 10);
    assert_eq!(snapshot.ram_used_mb, 40);
    assert_eq!(snapshot.frames_free, PHYSICAL_FRAMES - 10);

    for page in 0..10 {
        assert_eq!(
            memory.page_location(1, page),
            Some(PageLocation::Frame(page))
        );
        let owner = memory.frame_owner(page).unwrap();
        assert_eq!((owner.pid, owner.page), (1, page));
    }
}

#[test]
fn frame_release_and_process_cleanup_return_every_frame() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, 10).unwrap();
    memory.allocate(2, 5).unwrap();
    assert_eq!(memory.snapshot().frames_used, 15);

    memory.release(1);
    assert!(!memory.is_tracked(1));
    let snapshot = memory.snapshot();
    assert_eq!(snapshot.frames_used, 5);
    assert_eq!(snapshot.processes.len(), 1);
    assert_eq!(snapshot.processes[0].pid, 2);

    // The freed frames are reused by the next allocation.
    memory.allocate(3, 10).unwrap();
    assert_eq!(memory.page_location(3, 0), Some(PageLocation::Frame(0)));
    // release() on an unknown / host PID is a harmless no-op.
    memory.release(999);
}

#[test]
fn page_table_mapping_is_consistent_after_a_swap_round_trip() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, PHYSICAL_FRAMES).unwrap(); // fill RAM
    memory.allocate(2, 1).unwrap(); // spills to swap
    assert!(swapped(memory.page_location(2, 0)));

    // Fault the swapped page in; it must now be resident and its frame owned by it.
    assert_eq!(
        memory.access(2, 0).unwrap(),
        AccessOutcome::Fault { swapped_in: true }
    );
    let PageLocation::Frame(frame) = memory.page_location(2, 0).unwrap() else {
        panic!("page 0 of PID 2 should be resident");
    };
    let owner = memory.frame_owner(frame).unwrap();
    assert_eq!((owner.pid, owner.page), (2, 0));
}

#[test]
fn fifo_replacement_evicts_frames_in_load_order() {
    let mut memory = MemoryManager::new();
    assert_eq!(memory.policy(), ReplacementPolicy::Fifo);
    memory.allocate(1, PHYSICAL_FRAMES).unwrap(); // frames loaded 0,1,2,...
    memory.allocate(2, 2).unwrap(); // both pages in swap

    memory.access(2, 0).unwrap();
    assert!(swapped(memory.page_location(1, 0)), "page 0 evicted first");
    assert!(resident(memory.page_location(1, 1)));

    memory.access(2, 1).unwrap();
    assert!(swapped(memory.page_location(1, 1)), "page 1 evicted next");
    assert!(resident(memory.page_location(1, 2)));
}

#[test]
fn lru_replacement_evicts_the_least_recently_used_frame() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, PHYSICAL_FRAMES).unwrap();
    memory.set_policy(ReplacementPolicy::Lru);
    assert_eq!(memory.policy(), ReplacementPolicy::Lru);

    // Touch page 0 so its frame is the most-recently-used, not the stalest.
    assert_eq!(memory.access(1, 0).unwrap(), AccessOutcome::Hit);
    memory.allocate(2, 1).unwrap();

    memory.access(2, 0).unwrap();
    assert!(
        resident(memory.page_location(1, 0)),
        "the recently-touched page survives"
    );
    assert!(
        swapped(memory.page_location(1, 1)),
        "the stalest page is evicted"
    );
}

#[test]
fn page_faults_and_hits_are_counted_separately() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, PHYSICAL_FRAMES).unwrap();
    memory.allocate(2, 2).unwrap();

    let snapshot = memory.snapshot();
    assert_eq!(snapshot.page_faults, 0);
    assert_eq!(snapshot.page_hits, 0);

    assert_eq!(memory.access(1, 0).unwrap(), AccessOutcome::Hit);
    assert_eq!(
        memory.access(2, 0).unwrap(),
        AccessOutcome::Fault { swapped_in: true }
    );
    assert_eq!(memory.access(2, 0).unwrap(), AccessOutcome::Hit);

    let snapshot = memory.snapshot();
    assert_eq!(snapshot.page_hits, 2);
    assert_eq!(snapshot.page_faults, 1);
    assert_eq!(snapshot.swap_ins, 1);
    let pid2 = snapshot.processes.iter().find(|p| p.pid == 2).unwrap();
    assert_eq!(pid2.faults, 1);
}

#[test]
fn swap_out_happens_when_ram_is_full() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, PHYSICAL_FRAMES).unwrap();
    memory.allocate(2, 4).unwrap();

    let snapshot = memory.snapshot();
    assert_eq!(snapshot.frames_used, PHYSICAL_FRAMES);
    assert_eq!(snapshot.swap_used_mb, 16);
    assert_eq!(snapshot.swap_outs, 4);
    let pid2 = snapshot.processes.iter().find(|p| p.pid == 2).unwrap();
    assert_eq!(pid2.swapped_pages, 4);
    assert_eq!(pid2.resident_pages, 0);
}

#[test]
fn swap_in_pages_a_process_back_into_ram() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, PHYSICAL_FRAMES).unwrap();
    memory.allocate(2, 4).unwrap();
    assert!(swapped(memory.page_location(2, 0)));

    memory.access(2, 0).unwrap();
    assert!(resident(memory.page_location(2, 0)));
    assert!(
        swapped(memory.page_location(1, 0)),
        "a resident page was swapped out to make room"
    );
    assert_eq!(memory.snapshot().swap_ins, 1);
}

#[test]
fn out_of_memory_fails_cleanly_without_changing_state() {
    let mut memory = MemoryManager::new();

    // More pages than RAM + swap together.
    let err = memory
        .allocate(1, PHYSICAL_FRAMES + SWAP_SLOTS + 1)
        .unwrap_err();
    assert!(matches!(err, AaruError::OutOfMemory { .. }));
    assert!(!memory.is_tracked(1));
    assert_eq!(memory.snapshot().frames_used, 0);

    // Fill RAM and swap exactly, then the next page cannot be placed.
    memory.allocate(2, PHYSICAL_FRAMES).unwrap();
    memory.allocate(3, SWAP_SLOTS).unwrap();
    let err = memory.allocate(4, 1).unwrap_err();
    assert!(matches!(err, AaruError::OutOfMemory { .. }));
    assert!(!memory.is_tracked(4));
    let snapshot = memory.snapshot();
    assert_eq!(snapshot.frames_used, PHYSICAL_FRAMES);
    assert_eq!(snapshot.swap_used_mb, SWAP_SLOTS as u32 * 4);
}

#[test]
fn terminating_a_process_frees_only_its_memory() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, 10).unwrap();
    memory.allocate(2, 10).unwrap();
    memory.allocate(3, 10).unwrap();

    memory.release(2);
    assert_eq!(memory.snapshot().frames_used, 20);
    assert!(resident(memory.page_location(1, 5)));
    assert!(resident(memory.page_location(3, 5)));
    assert!(memory.page_location(2, 0).is_none());

    // PID 2's frames (10..20) are the ones reused next.
    memory.allocate(4, 10).unwrap();
    assert_eq!(memory.page_location(4, 0), Some(PageLocation::Frame(10)));
}

#[test]
fn switching_policy_keeps_state_and_changes_future_victims() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, PHYSICAL_FRAMES).unwrap();
    memory.allocate(2, 1).unwrap();

    // Under FIFO the first-loaded frame (page 0) is the victim.
    memory.set_policy(ReplacementPolicy::Lru);
    assert_eq!(memory.snapshot().frames_used, PHYSICAL_FRAMES);
    assert_eq!(memory.snapshot().processes.len(), 2);

    // Touch page 0 -> under LRU it now survives, page 1 is evicted instead.
    memory.access(1, 0).unwrap();
    memory.access(2, 0).unwrap();
    assert!(resident(memory.page_location(1, 0)));
    assert!(swapped(memory.page_location(1, 1)));
}

#[test]
fn tick_access_drives_hits_without_panicking() {
    let mut memory = MemoryManager::new();
    memory.allocate(1, 8).unwrap();
    memory.allocate(2, 8).unwrap();
    for _ in 0..50 {
        memory.tick_access(&[1, 2]);
    }
    let snapshot = memory.snapshot();
    // Working sets fit in RAM, so every touch is a hit and nothing swaps.
    assert_eq!(snapshot.page_faults, 0);
    assert!(snapshot.page_hits >= 100);
    assert_eq!(snapshot.swap_used_mb, 0);
    // Unknown PIDs are simply skipped.
    memory.tick_access(&[999]);
}

//! Centralised configuration for the simulated Aaru memory subsystem.
//!
//! Every number the paging model depends on lives here so it can never drift
//! and never changes at runtime. **None of this describes the real Windows
//! host** — it is a fixed simulation of a small machine.

/// Simulated page / frame size, in megabytes.
pub const PAGE_SIZE_MB: u32 = 4;

/// Physical frames in simulated RAM (`1024 × 4 MB = 4096 MB`).
pub const PHYSICAL_FRAMES: usize = 1024;

/// Simulated RAM, in megabytes. Kept in lock-step with the kernel constant.
pub const RAM_MB: u32 = crate::kernel::RAM_MB;

/// Backing-store slots for swapped-out pages (`1024 × 4 MB = 4096 MB` of the
/// 16 GB virtual disk reserved for swap).
pub const SWAP_SLOTS: usize = 1024;

/// Simulated swap capacity, in megabytes.
pub const SWAP_MB: u32 = SWAP_SLOTS as u32 * PAGE_SIZE_MB;

// The frame count and page size must describe exactly the advertised RAM.
const _: () = assert!(PHYSICAL_FRAMES as u32 * PAGE_SIZE_MB == RAM_MB);

/// Number of pages needed to hold `mb` megabytes (rounded up, never zero).
pub const fn pages_for_mb(mb: u32) -> usize {
    let pages = mb.div_ceil(PAGE_SIZE_MB) as usize;
    if pages == 0 {
        1
    } else {
        pages
    }
}

/// A centrally defined initial simulated-memory footprint for one Aaru process.
pub struct MemoryProfile {
    /// Matched case-insensitively against the launched application's name.
    pub key: &'static str,
    /// Resident simulated memory, in megabytes (a multiple of [`PAGE_SIZE_MB`]).
    pub resident_mb: u32,
}

/// Footprint used for any Aaru-native process without an explicit profile.
pub const DEFAULT_RESIDENT_MB: u32 = 16;

/// The initial working set of every built-in, in one place. These are *fixed*
/// design values — they are not sampled and do not change between renders.
///
/// `VSCode` is listed as a representation only: real host applications are
/// never placed in the simulated RAM model (their Windows memory, if shown, is
/// reported separately).
pub const PROFILES: &[MemoryProfile] = &[
    MemoryProfile {
        key: "Calculator",
        resident_mb: 16,
    },
    MemoryProfile {
        key: "TextEditor",
        resident_mb: 32,
    },
    MemoryProfile {
        key: "ImageViewer",
        resident_mb: 48,
    },
    MemoryProfile {
        key: "Snake",
        resident_mb: 32,
    },
    MemoryProfile {
        key: "Pong",
        resident_mb: 40,
    },
    MemoryProfile {
        key: "Minesweeper",
        resident_mb: 24,
    },
    MemoryProfile {
        key: "Tetris",
        resident_mb: 48,
    },
    MemoryProfile {
        key: "TaskManager",
        resident_mb: 24,
    },
    MemoryProfile {
        key: "Almanac",
        resident_mb: 40,
    },
    MemoryProfile {
        key: "Terminal",
        resident_mb: 32,
    },
    MemoryProfile {
        key: "Settings",
        resident_mb: 24,
    },
    MemoryProfile {
        key: "VSCode",
        resident_mb: 256,
    },
];

/// Resident MB for an application by name, falling back to [`DEFAULT_RESIDENT_MB`].
pub fn resident_mb_for(name: &str) -> u32 {
    PROFILES
        .iter()
        .find(|profile| profile.key.eq_ignore_ascii_case(name))
        .map(|profile| profile.resident_mb)
        .unwrap_or(DEFAULT_RESIDENT_MB)
}

/// Initial page count for an application by name.
pub fn resident_pages_for(name: &str) -> usize {
    pages_for_mb(resident_mb_for(name))
}

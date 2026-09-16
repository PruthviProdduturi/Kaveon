//! What the process really holds, and the limit it really has.
//!
//! Query budgets are reservations — estimates the operators make before
//! they allocate. This module is the ground truth beside them: a counting
//! allocator that knows every live byte, the container's memory limit read
//! from the cgroup, and a guard that refuses a reservation the process could
//! not honour, so a query fails closed instead of the pod being killed.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

/// The system allocator with a live-byte counter. Install it with
/// `#[global_allocator]` in the binary; every allocation and free updates
/// two atomics, which is all the accounting costs.
pub struct CountingAllocator;

// SAFETY: every method forwards to `System` unchanged; the counters are
// updated with atomics and never touch the allocation itself.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        ALLOCATED_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            let old = layout.size() as u64;
            let new = new_size as u64;
            if new >= old {
                record_alloc((new - old) as usize);
            } else {
                ALLOCATED_BYTES.fetch_sub(old - new, Ordering::Relaxed);
            }
        }
        moved
    }
}

fn record_alloc(bytes: usize) {
    let now = ALLOCATED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed) + bytes as u64;
    PEAK_ALLOCATED_BYTES.fetch_max(now, Ordering::Relaxed);
}

/// Live heap bytes, when the counting allocator is installed; zero otherwise.
pub fn allocated_bytes() -> u64 {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

/// The most live heap bytes seen since the process started.
pub fn peak_allocated_bytes() -> u64 {
    PEAK_ALLOCATED_BYTES.load(Ordering::Relaxed)
}

/// The container's memory limit, from cgroup v2 then v1; None when the
/// process is not limited (or not on Linux).
pub fn cgroup_memory_limit_bytes() -> Option<u64> {
    for path in [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ] {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Some(limit) = parse_cgroup_limit(&text)
        {
            return Some(limit);
        }
    }
    None
}

/// `max` (v2) and the v1 sentinel of roughly 2^63 both mean unlimited.
fn parse_cgroup_limit(text: &str) -> Option<u64> {
    let text = text.trim();
    if text == "max" {
        return None;
    }
    let limit = text.parse::<u64>().ok()?;
    // cgroup v1 reports "unlimited" as PAGE_COUNTER_MAX, a value near 2^63.
    (limit < 1 << 62).then_some(limit)
}

/// The guard: a limit, a headroom kept free for what no operator reserves
/// (scan buffers in flight, sockets, the allocator's own slack), and the
/// reading of what is allocated now. `probe` is the live-byte reading —
/// the counting allocator in production, anything in a test.
#[derive(Clone)]
pub struct ProcessMemory {
    inner: Arc<ProcessMemoryInner>,
}

struct ProcessMemoryInner {
    limit_bytes: u64,
    headroom_bytes: u64,
    probe: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for ProcessMemory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessMemory")
            .field("limit_bytes", &self.inner.limit_bytes)
            .field("headroom_bytes", &self.inner.headroom_bytes)
            .finish()
    }
}

/// Kept free below the limit: the larger of 256 MiB and 15% of it.
pub fn default_headroom_bytes(limit_bytes: u64) -> u64 {
    (limit_bytes / 100 * 15)
        .max(256 * 1024 * 1024)
        .min(limit_bytes)
}

impl ProcessMemory {
    /// A guard over the counting allocator's reading.
    pub fn new(limit_bytes: u64) -> Self {
        Self::with_probe(
            limit_bytes,
            default_headroom_bytes(limit_bytes),
            allocated_bytes,
        )
    }

    /// A guard whose live-byte reading comes from `probe`.
    pub fn with_probe(
        limit_bytes: u64,
        headroom_bytes: u64,
        probe: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(ProcessMemoryInner {
                limit_bytes,
                headroom_bytes: headroom_bytes.min(limit_bytes),
                probe: Box::new(probe),
            }),
        }
    }

    /// The guard for this process, from the cgroup limit; None when the
    /// process is not limited, in which case reservations answer only to
    /// their query budgets.
    pub fn from_cgroup() -> Option<Self> {
        cgroup_memory_limit_bytes().map(Self::new)
    }

    pub fn limit_bytes(&self) -> u64 {
        self.inner.limit_bytes
    }

    pub fn headroom_bytes(&self) -> u64 {
        self.inner.headroom_bytes
    }

    /// Live bytes as the probe sees them now.
    pub fn allocated_bytes(&self) -> u64 {
        (self.inner.probe)()
    }

    /// Bytes a reservation may still take: the limit less the headroom
    /// less what is already allocated.
    pub fn available_bytes(&self) -> u64 {
        self.inner
            .limit_bytes
            .saturating_sub(self.inner.headroom_bytes)
            .saturating_sub(self.allocated_bytes())
    }

    /// Whether `bytes` more can be honoured now.
    pub fn can_reserve(&self, bytes: u64) -> bool {
        bytes <= self.available_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// The crate's test binary counts its own allocations.
    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    #[test]
    fn the_counting_allocator_sees_live_bytes_rise_and_fall() {
        // Other tests allocate and free on their own threads meanwhile, so
        // the block is far larger than any of them and the checks allow
        // a few mebibytes of their noise.
        const BLOCK: u64 = 64 << 20;
        const NOISE: u64 = 8 << 20;
        let before = allocated_bytes();
        let block = vec![7u8; BLOCK as usize];
        let held = allocated_bytes();
        assert!(held + NOISE >= before + BLOCK, "{held} >= {before} + block");
        assert!(peak_allocated_bytes() + NOISE >= held);
        let mut grown = block;
        grown.resize((2 * BLOCK) as usize, 1);
        assert!(allocated_bytes() + NOISE >= before + 2 * BLOCK);
        drop(grown);
        assert!(allocated_bytes() <= held - BLOCK + NOISE);
    }

    #[test]
    fn cgroup_limits_parse_and_unlimited_is_none() {
        assert_eq!(parse_cgroup_limit("max\n"), None);
        assert_eq!(parse_cgroup_limit("6442450944\n"), Some(6442450944));
        assert_eq!(parse_cgroup_limit("9223372036854771712"), None);
        assert_eq!(parse_cgroup_limit("garbage"), None);
    }

    #[test]
    fn headroom_is_the_larger_of_a_floor_and_a_share() {
        assert_eq!(default_headroom_bytes(1 << 30), 256 * 1024 * 1024);
        assert_eq!(default_headroom_bytes(6 << 30), (6u64 << 30) / 100 * 15);
        assert_eq!(default_headroom_bytes(1024), 1024);
    }

    #[test]
    fn guard_refuses_what_the_process_cannot_hold() {
        let live = Arc::new(AtomicU64::new(0));
        let probe = live.clone();
        let guard = ProcessMemory::with_probe(1_000, 100, move || probe.load(Ordering::Relaxed));
        assert_eq!(guard.available_bytes(), 900);
        assert!(guard.can_reserve(900));
        assert!(!guard.can_reserve(901));
        live.store(850, Ordering::Relaxed);
        assert_eq!(guard.available_bytes(), 50);
        assert!(!guard.can_reserve(51));
        live.store(2_000, Ordering::Relaxed);
        assert_eq!(guard.available_bytes(), 0);
        assert!(guard.can_reserve(0));
    }
}

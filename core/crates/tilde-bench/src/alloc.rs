//! A global allocator that counts: allocations made, bytes asked for, bytes held now and
//! the most held at once. Counters are process-wide (every thread), relaxed atomics.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);

fn grew(by: usize) {
    let live = LIVE.fetch_add(by as i64, Ordering::Relaxed) + by as i64;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        grew(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        grew(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        grew(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Heap {
    pub allocs: u64,
    pub bytes: u64,
    /// Bytes held now.
    pub live: i64,
}

pub fn heap() -> Heap {
    Heap {
        allocs: ALLOCS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        live: LIVE.load(Ordering::Relaxed),
    }
}

/// Waits (up to `max`) until the heap stops changing for a moment: background threads
/// still freeing an index dropped earlier would otherwise blur a memory measurement.
pub fn settle(max: std::time::Duration) {
    let started = std::time::Instant::now();
    let mut last = heap().live;
    let mut steady = std::time::Instant::now();
    while started.elapsed() < max {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let now = heap().live;
        if (now - last).abs() > 4096 {
            last = now;
            steady = std::time::Instant::now();
        } else if steady.elapsed() >= std::time::Duration::from_millis(250) {
            return;
        }
    }
}

/// Starts a new peak window: the peak becomes what's held now.
pub fn reset_peak() {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// The most bytes held at once since `reset_peak`.
pub fn peak() -> i64 {
    PEAK.load(Ordering::Relaxed)
}

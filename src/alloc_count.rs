//! Test-only global allocator that counts allocations and frees made by the
//! current thread inside `count`, so real-time paths can assert they never
//! touch the heap. Counting is per thread: parallel tests never pollute it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static FREES: Cell<u64> = const { Cell::new(0) };
}

/// Heap traffic observed on one thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HeapTraffic {
    pub(crate) allocs: u64,
    pub(crate) frees: u64,
}

impl std::ops::AddAssign for HeapTraffic {
    fn add_assign(&mut self, other: Self) {
        self.allocs += other.allocs;
        self.frees += other.frees;
    }
}

/// Run `f` and return its result with the heap traffic it caused on this thread.
/// A reallocation counts as one allocation and one free.
pub(crate) fn count<T>(f: impl FnOnce() -> T) -> (T, HeapTraffic) {
    let before = (ALLOCS.get(), FREES.get());
    let was_counting = COUNTING.replace(true);
    let out = f();
    COUNTING.set(was_counting);
    let traffic = HeapTraffic {
        allocs: ALLOCS.get() - before.0,
        frees: FREES.get() - before.1,
    };
    (out, traffic)
}

struct CountingAlloc;

fn note(counter: &'static std::thread::LocalKey<Cell<u64>>) {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = COUNTING.try_with(|counting| {
        if counting.get() {
            let _ = counter.try_with(|n| n.set(n.get() + 1));
        }
    });
}

// SAFETY: every call forwards unchanged to `System`; counting touches only
// const-initialised thread-locals, which never allocate.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(&FREES);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(&ALLOCS);
        note(&FREES);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

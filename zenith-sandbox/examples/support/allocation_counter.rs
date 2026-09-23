use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static COUNTS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

struct Counter;

#[global_allocator]
static ALLOCATOR: Counter = Counter;

fn record(bytes: usize) {
    let _ = COUNTS.try_with(|counts| {
        if let Some((allocations, total)) = counts.get() {
            counts.set(Some((allocations + 1, total + bytes as u64)));
        }
    });
}

unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record(layout.size());
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let ptr = unsafe { System.realloc(ptr, layout, size) };
        if !ptr.is_null() {
            record(size);
        }
        ptr
    }
}

pub fn take() -> (u64, u64) {
    COUNTS.with(|counts| counts.replace(Some((0, 0))).unwrap_or_default())
}

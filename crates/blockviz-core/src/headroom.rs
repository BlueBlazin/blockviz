//! The system allocator, growing WebAssembly memory at least 4 MB at a time.
//! On its own it grows memory by what each allocation lacks, 64 KB at a
//! time, and every growth costs the engine far more than an allocation:
//! reaching 400 MB that way takes 6,000 growths and 390 ms.

use core::arch::wasm32::memory_size;
use std::alloc::{GlobalAlloc, Layout, System};

#[global_allocator]
static ALLOCATOR: Headroom = Headroom;

struct Headroom;

unsafe impl GlobalAlloc for Headroom {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        with_headroom(|| System.alloc(layout))
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        with_headroom(|| System.alloc_zeroed(layout))
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        with_headroom(|| System.realloc(ptr, layout, size))
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

/// Makes an allocation; if it grew memory, allocates 4 MB past it and frees
/// them, so that the allocations after it find room without growing it.
fn with_headroom(alloc: impl FnOnce() -> *mut u8) -> *mut u8 {
    let pages = memory_size(0);
    let ptr = alloc();
    if memory_size(0) > pages {
        let room = Layout::from_size_align(4 << 20, 1).unwrap();
        unsafe {
            let block = System.alloc(room);
            if !block.is_null() {
                System.dealloc(block, room);
            }
        }
    }
    ptr
}

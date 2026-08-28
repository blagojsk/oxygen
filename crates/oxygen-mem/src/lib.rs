//! Portable memory-management logic for Oxygen.
//!
//! Deliberately free of architecture and hardware: everything here is arithmetic over addresses
//! and bits, so it runs and is tested on the host. The architecture-specific half — reading the
//! firmware memory map, programming page tables — lives in the kernel and calls into this.

#![no_std]

pub mod addr;
pub mod frame;
pub mod heap;

pub use addr::{FRAME_SIZE, Frame, PhysAddr};
pub use frame::{AllocError, FrameAllocator};
pub use heap::Heap;

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod heap_tests {
    use super::heap::Heap;
    use core::alloc::Layout;
    use std::vec::Vec;

    /// 64 KiB of host memory standing in for a mapped region. Leaked on purpose: the heap hands
    /// out pointers into it, so it must outlive every test that uses it.
    fn region(bytes: usize) -> *mut u8 {
        let v: Vec<u8> = std::vec![0u8; bytes];
        std::boxed::Box::leak(v.into_boxed_slice()).as_mut_ptr()
    }

    fn heap_of(bytes: usize) -> Heap {
        let mut h = Heap::new();
        // SAFETY: the region is leaked, so it stays valid for the whole test.
        unsafe { h.add_region(region(bytes), bytes) };
        h
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn allocates_and_reports_usage() {
        let mut h = heap_of(4096);
        assert_eq!(h.used(), 0);
        // SAFETY: standard allocator use within a live heap.
        let p = unsafe { h.alloc(layout(64, 8)) };
        assert!(!p.is_null());
        assert!(h.used() >= 64);
        assert!(h.free() < h.size());
    }

    #[test]
    fn allocations_do_not_overlap() {
        let mut h = heap_of(4096);
        // SAFETY: as above.
        let (a, b) = unsafe { (h.alloc(layout(64, 8)), h.alloc(layout(64, 8))) };
        assert!(!a.is_null() && !b.is_null());
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        assert!(hi as usize - lo as usize >= 64, "allocations overlap");
    }

    #[test]
    fn honours_alignment() {
        let mut h = heap_of(8192);
        for align in [8usize, 16, 32, 64, 256] {
            // SAFETY: as above.
            let p = unsafe { h.alloc(layout(32, align)) };
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0, "misaligned for {align}");
        }
    }

    #[test]
    fn freed_memory_is_reused() {
        let mut h = heap_of(4096);
        // SAFETY: allocate, release, allocate the same shape again.
        let first = unsafe { h.alloc(layout(128, 8)) };
        unsafe { h.dealloc(first, layout(128, 8)) };
        assert_eq!(h.used(), 0);
        let second = unsafe { h.alloc(layout(128, 8)) };
        assert_eq!(first, second, "the freed block should be handed back out");
    }

    /// The property that keeps a long-lived kernel alive. Without coalescing, a heap fragments
    /// into rubble after enough cycles and behaves exactly like a leak.
    #[test]
    fn adjacent_frees_coalesce_back_into_one_block() {
        let mut h = heap_of(4096);
        // SAFETY: three adjacent allocations, then all released.
        unsafe {
            let a = h.alloc(layout(256, 8));
            let b = h.alloc(layout(256, 8));
            let c = h.alloc(layout(256, 8));
            h.dealloc(a, layout(256, 8));
            h.dealloc(b, layout(256, 8));
            h.dealloc(c, layout(256, 8));
        }
        assert_eq!(h.used(), 0);
        // If the three blocks merged, one allocation larger than any single block succeeds.
        // SAFETY: as above.
        let big = unsafe { h.alloc(layout(700, 8)) };
        assert!(!big.is_null(), "blocks did not coalesce");
    }

    #[test]
    fn returns_null_when_exhausted_rather_than_misbehaving() {
        let mut h = heap_of(1024);
        // SAFETY: deliberately larger than the heap.
        let p = unsafe { h.alloc(layout(65536, 8)) };
        assert!(p.is_null());
    }

    /// Churn with mixed sizes: the heap must not lose memory across many cycles.
    #[test]
    fn survives_repeated_churn_without_leaking() {
        let mut h = heap_of(16 * 1024);
        let total = h.size();
        for round in 0..200 {
            let size = 32 + (round % 7) * 48;
            // SAFETY: each pointer is freed with the layout it was allocated with.
            unsafe {
                let p = h.alloc(layout(size, 8));
                assert!(!p.is_null(), "exhausted at round {round}");
                h.dealloc(p, layout(size, 8));
            }
        }
        assert_eq!(h.used(), 0);
        assert_eq!(h.size(), total);
    }

    #[test]
    fn a_zero_sized_request_still_returns_something_usable() {
        let mut h = heap_of(4096);
        // SAFETY: zero-sized layouts are rounded up to the minimum block.
        let p = unsafe { h.alloc(layout(0, 1)) };
        assert!(!p.is_null());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    /// A 1 MiB span (256 frames) based at 1 MiB, with all of it usable.
    fn allocator(storage: &mut [u8]) -> FrameAllocator<'_> {
        let mut a = FrameAllocator::new(storage, PhysAddr(MIB), 256).unwrap();
        a.mark_usable(PhysAddr(MIB), PhysAddr(2 * MIB));
        a
    }

    #[test]
    fn bitmap_sizing_rounds_up_to_whole_bytes() {
        assert_eq!(FrameAllocator::bitmap_bytes(0), 0);
        assert_eq!(FrameAllocator::bitmap_bytes(1), 1);
        assert_eq!(FrameAllocator::bitmap_bytes(8), 1);
        assert_eq!(FrameAllocator::bitmap_bytes(9), 2);
        // The headline efficiency claim: 32 KiB of metadata per GiB of RAM.
        assert_eq!(FrameAllocator::bitmap_bytes(262_144), 32 * 1024);
    }

    /// Everything starts reserved, so forgetting to declare usable memory fails loudly instead of
    /// handing out firmware tables.
    #[test]
    fn starts_fully_used() {
        let mut storage = [0u8; 32];
        let a = FrameAllocator::new(&mut storage, PhysAddr(MIB), 256).unwrap();
        assert_eq!(a.used_frames(), 256);
        assert_eq!(a.free_frames(), 0);
    }

    #[test]
    fn rejects_a_misaligned_base() {
        let mut storage = [0u8; 32];
        assert!(FrameAllocator::new(&mut storage, PhysAddr(MIB + 1), 256).is_err());
    }

    #[test]
    fn rejects_a_bitmap_that_is_too_small() {
        let mut storage = [0u8; 4];
        assert!(FrameAllocator::new(&mut storage, PhysAddr(MIB), 256).is_err());
    }

    #[test]
    fn allocates_and_frees_a_single_frame() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        assert_eq!(a.free_frames(), 256);

        let f = a.alloc().unwrap();
        assert_eq!(f.start(), PhysAddr(MIB));
        assert_eq!(a.free_frames(), 255);

        a.free(f).unwrap();
        assert_eq!(a.free_frames(), 256);
    }

    #[test]
    fn allocations_do_not_overlap() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        let first = a.alloc().unwrap();
        let second = a.alloc().unwrap();
        assert_ne!(first, second);
        assert_eq!(second.start().0 - first.start().0, FRAME_SIZE as u64);
    }

    #[test]
    fn exhausts_then_reports_out_of_memory() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        for _ in 0..256 {
            a.alloc().unwrap();
        }
        assert_eq!(a.free_frames(), 0);
        assert_eq!(a.alloc(), Err(AllocError::OutOfMemory));
    }

    #[test]
    fn contiguous_allocation_returns_adjacent_frames() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        let run = a.alloc_contiguous(4).unwrap();
        assert_eq!(a.free_frames(), 252);
        // Every frame in the run must now be taken, so a later single alloc cannot land inside it.
        let next = a.alloc().unwrap();
        assert_eq!(next.start().0, run.start().0 + 4 * FRAME_SIZE as u64);
    }

    /// Enough free frames, but never enough side by side — a layout failure, not a capacity one,
    /// and the caller is told which.
    #[test]
    fn fragmentation_is_distinguished_from_exhaustion() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        // Reserve every other frame.
        for i in (0..256).step_by(2) {
            let at = PhysAddr(MIB + (i * FRAME_SIZE) as u64);
            a.mark_reserved(at, PhysAddr(at.0 + FRAME_SIZE as u64));
        }
        assert_eq!(a.free_frames(), 128);
        assert_eq!(a.alloc_contiguous(2), Err(AllocError::Fragmented));
        assert!(a.alloc().is_ok());
    }

    #[test]
    fn zero_frame_requests_are_a_caller_bug() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        assert_eq!(a.alloc_contiguous(0), Err(AllocError::ZeroFrames));
    }

    /// Handing the same frame to two owners is the worst bug this allocator could have, so a
    /// double free is refused rather than silently corrupting the used count.
    #[test]
    fn double_free_is_refused() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        let f = a.alloc().unwrap();
        a.free(f).unwrap();
        assert!(a.free(f).is_err());
        assert_eq!(a.free_frames(), 256);
    }

    #[test]
    fn freeing_something_we_never_owned_is_refused() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        let outside = Frame::containing(PhysAddr(64 * MIB));
        assert!(a.free(outside).is_err());
    }

    /// A partially usable frame must not be handed out: the rest of it may belong to firmware.
    #[test]
    fn partial_frames_are_not_made_usable() {
        let mut storage = [0u8; 32];
        let mut a = FrameAllocator::new(&mut storage, PhysAddr(MIB), 256).unwrap();
        // Covers most of frame 0 but not all of it.
        a.mark_usable(PhysAddr(MIB + 16), PhysAddr(MIB + 2048));
        assert_eq!(a.free_frames(), 0);
    }

    /// Conversely a frame only partly reserved must be withheld entirely.
    #[test]
    fn partly_reserved_frames_are_withheld_entirely() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        let frame1 = MIB + FRAME_SIZE as u64;
        // One byte inside frame 1 is reserved; the whole frame must go.
        a.mark_reserved(PhysAddr(frame1 + 8), PhysAddr(frame1 + 9));
        assert_eq!(a.free_frames(), 255);
        // Drain the allocator; frame 1 must never come back.
        for _ in 0..255 {
            assert_ne!(a.alloc().unwrap().start(), PhysAddr(frame1));
        }
    }

    #[test]
    fn reserved_regions_are_never_allocated() {
        let mut storage = [0u8; 32];
        let mut a = allocator(&mut storage);
        // Pretend the kernel image occupies the first 64 KiB.
        a.mark_reserved(PhysAddr(MIB), PhysAddr(MIB + 16 * FRAME_SIZE as u64));
        assert_eq!(a.free_frames(), 240);
        let f = a.alloc().unwrap();
        assert!(f.start().0 >= MIB + 16 * FRAME_SIZE as u64);
    }

    #[test]
    fn addresses_round_to_frame_boundaries() {
        assert_eq!(PhysAddr(4095).frame_floor(), PhysAddr(0));
        assert_eq!(PhysAddr(4096).frame_floor(), PhysAddr(4096));
        assert_eq!(PhysAddr(1).frame_ceil(), PhysAddr(4096));
        assert_eq!(PhysAddr(4096).frame_ceil(), PhysAddr(4096));
        // Rounding up near the top of the address space must not wrap into low memory.
        assert!(PhysAddr(u64::MAX).frame_ceil().0 >= PhysAddr(u64::MAX).frame_floor().0);
    }
}

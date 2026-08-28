//! Kernel memory: what RAM exists, who owns it, and where `alloc` gets its bytes.
//!
//! Two allocators sit on top of each other, deliberately. The frame allocator owns physical
//! memory in 4 KiB units and is what page tables and DMA buffers will come from; the heap sits on
//! frames handed to it and serves the small, irregular allocations that `Box` and `Vec` make.
//! Collapsing them into one would mean either tracking every byte with page-sized granularity or
//! letting a byte-granular allocator hand out page tables, and neither ends well.

use core::alloc::{GlobalAlloc, Layout};

use oxygen_mem::{FRAME_SIZE, FrameAllocator, Heap, PhysAddr};

use crate::arch::target::mmu;
use crate::println;
use crate::sync::SpinLock;

/// How much RAM to assume.
///
/// A placeholder, and knowingly so: the real extent is in the device tree QEMU hands us in x0 at
/// entry, and on a real board it is the only way to know. Assuming 128 MiB is safe here because it
/// is less than the 256 MiB the run scripts give the machine — the failure mode of guessing too
/// high is writing to memory that does not exist, which is silent and awful, so the guess is
/// deliberately low until the device tree is parsed.
const ASSUMED_RAM: u64 = 128 * 1024 * 1024;
const RAM_BASE: u64 = 0x4000_0000;

/// How much of that to give the heap up front.
///
/// Small on purpose. On a machine with 64 MiB, a kernel that reserves tens of megabytes for itself
/// has already spent what the user wanted. Growing the heap from free frames on demand is the
/// right answer and is straightforward once there is a fault handler to trigger it.
const HEAP_FRAMES: usize = 512; // 2 MiB

/// Bitmap backing the frame allocator: one bit per 4 KiB frame.
///
/// Sized for `ASSUMED_RAM` and stored in `.bss`, because the allocator has to describe memory
/// before anything can allocate from it. At 32 KiB per GiB this costs 4 KiB for 128 MiB.
const BITMAP_BYTES: usize = (ASSUMED_RAM as usize / FRAME_SIZE).div_ceil(8);
static mut FRAME_BITMAP: [u8; BITMAP_BYTES] = [0; BITMAP_BYTES];

static FRAMES: SpinLock<Option<FrameAllocator<'static>>> = SpinLock::new(None);

#[global_allocator]
static HEAP: LockedHeap = LockedHeap(SpinLock::new(Heap::new()));

struct LockedHeap(SpinLock<Heap>);

// SAFETY: every entry point takes the lock, so the inner heap is never touched concurrently, and
// it hands out pointers into a region it exclusively owns.
unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the heap is initialised before any allocation can occur — `init` runs before
        // anything in the kernel allocates, and an allocation before that returns null, which the
        // caller must already handle.
        unsafe { self.0.lock().alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the pointer came from this allocator, per the GlobalAlloc contract.
        unsafe { self.0.lock().dealloc(ptr, layout) }
    }
}

/// Describes physical memory and brings the heap up.
///
/// # Safety
/// Runs once, after the MMU is on — the heap's memory is only usable because the identity map
/// covers it.
pub unsafe fn init() {
    let frames_total = (ASSUMED_RAM as usize) / FRAME_SIZE;

    // Built straight from the raw pointer rather than by taking a reference to the static: a
    // reference to a `static mut` is denied outright in this edition, and the intermediate
    // deref-of-addr-of that works around it is the same thing wearing a hat.
    // SAFETY: sole writer of the bitmap, on the boot core, before any other allocation exists, and
    // the length matches the array's declared size exactly.
    let bitmap: &'static mut [u8] = unsafe {
        core::slice::from_raw_parts_mut((&raw mut FRAME_BITMAP).cast::<u8>(), BITMAP_BYTES)
    };
    let mut allocator = match FrameAllocator::new(bitmap, PhysAddr(RAM_BASE), frames_total) {
        Ok(a) => a,
        Err(e) => panic!("frame allocator: {e}"),
    };

    // Everything starts reserved, so only memory that is genuinely free is declared usable —
    // which here means RAM above the kernel image. Getting this backwards would hand out the
    // kernel's own code as scratch space.
    let kernel_end = mmu::kernel_end();
    allocator.mark_usable(PhysAddr(kernel_end), PhysAddr(RAM_BASE + ASSUMED_RAM));

    let free_before = allocator.free_frames();

    // Carve the heap out of frames rather than a static array: a static would count against the
    // kernel image, which has to fit in the 2 MiB its page table covers.
    let heap_start = match allocator.alloc_contiguous(HEAP_FRAMES) {
        Ok(frame) => frame.start().0,
        Err(e) => panic!("could not reserve {HEAP_FRAMES} frames for the heap: {e:?}"),
    };
    let heap_bytes = HEAP_FRAMES * FRAME_SIZE;

    // SAFETY: the frames were just allocated to us and nothing else refers to them, and the
    // identity map covers this range as writable, never-executable RAM.
    unsafe { HEAP.0.lock().add_region(heap_start as *mut u8, heap_bytes) };

    *FRAMES.lock() = Some(allocator);

    println!(
        "  [mm]   {} MiB RAM assumed, {} MiB free after kernel, {} KiB heap",
        ASSUMED_RAM / (1024 * 1024),
        (free_before * FRAME_SIZE) / (1024 * 1024),
        heap_bytes / 1024,
    );
}

/// Frames currently unallocated. The scheduler and the eventual memory-pressure reporting both
/// need this; nothing calls it yet.
#[allow(dead_code)]
pub fn free_frames() -> usize {
    FRAMES.lock().as_ref().map_or(0, |a| a.free_frames())
}

/// Bytes the heap has handed out.
pub fn heap_used() -> usize {
    HEAP.0.lock().used()
}

/// Bytes the heap could still hand out.
pub fn heap_free() -> usize {
    HEAP.0.lock().free()
}

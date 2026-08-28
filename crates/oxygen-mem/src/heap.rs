//! A free-list heap allocator.
//!
//! Chosen for the same reason as the bitmap frame allocator: it spends almost nothing on
//! bookkeeping. Free blocks store their own header, so the metadata lives inside memory that is by
//! definition not in use — an allocated byte costs nothing but itself, and a heap that is entirely
//! allocated has zero overhead. A size-class or buddy allocator answers faster but keeps
//! per-block structures resident whether or not anything is allocated, which is the wrong trade on
//! a machine with 64 MiB.
//!
//! Blocks are kept sorted by address. That is what makes coalescing possible: without it, a heap
//! survives a few thousand alloc/free cycles and then fragments into rubble, which on a long-lived
//! kernel is indistinguishable from a leak.

use core::alloc::Layout;
use core::mem::{align_of, size_of};
use core::ptr::NonNull;

/// Header of a free block, stored in the block's own first bytes.
struct FreeBlock {
    size: usize,
    next: Option<NonNull<FreeBlock>>,
}

/// Smallest block worth tracking: anything smaller cannot hold the header that makes it findable
/// again, so splitting down to it would leak the remainder.
const MIN_BLOCK: usize = size_of::<FreeBlock>();
const MIN_ALIGN: usize = align_of::<FreeBlock>();

/// A heap over one contiguous region.
///
/// Not thread-safe on its own — the caller wraps it in whatever lock the system uses. Keeping the
/// lock out of here is what lets the whole allocator be tested on the host.
pub struct Heap {
    head: Option<NonNull<FreeBlock>>,
    size: usize,
    used: usize,
}

// SAFETY: Heap owns its region exclusively and holds no thread-affine state; the raw pointers it
// carries are into that region. Sharing it between threads still requires external locking, which
// is why only Send is claimed and not Sync.
unsafe impl Send for Heap {}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub const fn new() -> Self {
        Heap {
            head: None,
            size: 0,
            used: 0,
        }
    }

    /// Adds a region to the heap.
    ///
    /// # Safety
    /// `start` must point to `size` bytes that are mapped, writable, and not owned by anything
    /// else for as long as this heap lives.
    pub unsafe fn add_region(&mut self, start: *mut u8, size: usize) {
        let aligned = (start as usize).next_multiple_of(MIN_ALIGN);
        let lost = aligned - start as usize;
        if size < lost + MIN_BLOCK {
            return;
        }
        let size = (size - lost) & !(MIN_ALIGN - 1);
        self.size += size;
        // SAFETY: the region is caller-guaranteed usable, and we have just aligned within it.
        unsafe { self.push_free(aligned as *mut u8, size) };
    }

    pub const fn size(&self) -> usize {
        self.size
    }

    pub const fn used(&self) -> usize {
        self.used
    }

    pub const fn free(&self) -> usize {
        self.size - self.used
    }

    /// Allocates, or returns null if no block can satisfy the layout.
    ///
    /// # Safety
    /// Standard allocator contract: the returned pointer is valid for `layout` until freed.
    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let (size, align) = adjust(layout);

        let mut prev: Option<NonNull<FreeBlock>> = None;
        let mut current = self.head;

        while let Some(mut block) = current {
            // SAFETY: every node in the list is a live free block we placed there.
            let (block_start, block_size) =
                unsafe { (block.as_ptr() as usize, block.as_ref().size) };
            let payload = block_start.next_multiple_of(align);
            let front_padding = payload - block_start;

            // First fit. Best fit would waste less but walks the whole list on every allocation,
            // and coalescing already keeps the list short enough that the difference does not pay.
            if front_padding + size <= block_size {
                // SAFETY: block is live and `prev`, if present, precedes it in this list.
                unsafe { self.unlink(prev, block) };
                let tail_start = payload + size;
                let tail_size = block_start + block_size - tail_start;

                // Padding created by alignment is returned to the heap rather than absorbed, or a
                // heavily aligned workload would bleed memory a few bytes at a time.
                if front_padding >= MIN_BLOCK {
                    // SAFETY: inside the block we just removed, and large enough for a header.
                    unsafe { self.push_free(block_start as *mut u8, front_padding) };
                }
                if tail_size >= MIN_BLOCK {
                    // SAFETY: as above, for the remainder past the allocation.
                    unsafe { self.push_free(tail_start as *mut u8, tail_size) };
                }

                let charged = block_size
                    - if front_padding >= MIN_BLOCK {
                        front_padding
                    } else {
                        0
                    }
                    - if tail_size >= MIN_BLOCK { tail_size } else { 0 };
                self.used += charged;
                return payload as *mut u8;
            }

            prev = Some(block);
            // SAFETY: block is live; reading its next pointer.
            current = unsafe { block.as_mut().next };
        }
        core::ptr::null_mut()
    }

    /// Returns memory to the heap.
    ///
    /// # Safety
    /// `ptr` must have come from this heap with this `layout`, and must not be freed twice.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _) = adjust(layout);
        self.used -= size;
        // SAFETY: caller guarantees the block came from here and is no longer in use.
        unsafe { self.push_free(ptr, size) };
    }

    /// Inserts a block in address order and merges it with whatever it now touches.
    ///
    /// # Safety
    /// `ptr` must own `size` usable bytes not present anywhere else in the list.
    unsafe fn push_free(&mut self, ptr: *mut u8, size: usize) {
        if size < MIN_BLOCK {
            return;
        }
        let addr = ptr as usize;
        // SAFETY: caller guarantees the region; writing the header into its first bytes.
        let node = unsafe {
            let node = ptr as *mut FreeBlock;
            node.write(FreeBlock { size, next: None });
            NonNull::new_unchecked(node)
        };

        let mut prev: Option<NonNull<FreeBlock>> = None;
        let mut current = self.head;
        while let Some(block) = current {
            if block.as_ptr() as usize > addr {
                break;
            }
            prev = Some(block);
            // SAFETY: block is a live node.
            current = unsafe { block.as_ref().next };
        }

        // SAFETY: all three nodes are live and ordered; linking then merging neighbours.
        unsafe {
            (*node.as_ptr()).next = current;
            match prev {
                Some(mut p) => p.as_mut().next = Some(node),
                None => self.head = Some(node),
            }
            merge(node);
            if let Some(p) = prev {
                merge(p);
            }
        }
    }

    /// # Safety
    /// `prev` must immediately precede `block`, or be `None` when `block` is the head.
    unsafe fn unlink(&mut self, prev: Option<NonNull<FreeBlock>>, block: NonNull<FreeBlock>) {
        // SAFETY: both nodes are live and adjacent in the list.
        unsafe {
            let next = block.as_ref().next;
            match prev {
                Some(mut p) => p.as_mut().next = next,
                None => self.head = next,
            }
        }
    }
}

/// Merges `block` with its successor when they are physically adjacent.
///
/// # Safety
/// `block` must be a live node in a list sorted by address.
unsafe fn merge(mut block: NonNull<FreeBlock>) {
    // SAFETY: live node; reading its size and successor.
    unsafe {
        let end = block.as_ptr() as usize + block.as_ref().size;
        if let Some(next) = block.as_ref().next
            && next.as_ptr() as usize == end
        {
            block.as_mut().size += next.as_ref().size;
            block.as_mut().next = next.as_ref().next;
        }
    }
}

/// Rounds a layout up to something the free list can represent: at least a header's worth, and at
/// least the header's alignment, so any freed block can hold the node describing it.
fn adjust(layout: Layout) -> (usize, usize) {
    let size = layout.size().max(MIN_BLOCK).next_multiple_of(MIN_ALIGN);
    (size, layout.align().max(MIN_ALIGN))
}

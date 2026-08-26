//! Physical frame allocator.
//!
//! A bitmap, deliberately: one bit per frame is 32 KiB of metadata per GiB of RAM, and on a 64 MiB
//! machine — the kind this OS exists to keep useful — that is two kilobytes. A buddy allocator
//! would serve larger allocations faster but carries per-block structures that a free-list or tree
//! must store somewhere, which is exactly the memory we are trying not to spend.
//!
//! The allocator owns no memory of its own. The caller supplies the bitmap storage, because at the
//! point this runs there is nothing else to allocate from.

use crate::addr::{FRAME_SIZE, Frame, PhysAddr};

/// Why an allocation could not be satisfied. Distinguishing these matters: exhaustion is a
/// capacity problem, fragmentation is a layout problem, and they call for different responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocError {
    /// No free frame anywhere.
    OutOfMemory,
    /// Free frames exist, but not `count` of them side by side.
    Fragmented,
    /// A zero-frame request, which is a caller bug rather than a memory condition.
    ZeroFrames,
}

/// Bitmap allocator over a contiguous span of physical memory.
///
/// A set bit means the frame is in use. Frames start out used, so a caller that forgets to mark
/// its usable regions gets an allocator that hands out nothing — a loud failure — rather than one
/// that hands out firmware tables.
pub struct FrameAllocator<'a> {
    bitmap: &'a mut [u8],
    base: PhysAddr,
    frames: usize,
    /// Where the next search begins, so repeated allocations do not rescan the same used prefix.
    cursor: usize,
    used: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Storage needed to track `frames` frames, rounded up to whole bytes.
    pub const fn bitmap_bytes(frames: usize) -> usize {
        frames.div_ceil(8)
    }

    /// Creates an allocator covering `frames` frames starting at `base`, with everything marked
    /// used. `base` must be frame-aligned and `bitmap` large enough; both are caller errors.
    pub fn new(bitmap: &'a mut [u8], base: PhysAddr, frames: usize) -> Result<Self, &'static str> {
        if !base.is_frame_aligned() {
            return Err("frame allocator base is not frame-aligned");
        }
        if bitmap.len() < Self::bitmap_bytes(frames) {
            return Err("frame allocator bitmap is too small for the frame count");
        }
        bitmap.fill(0xFF);
        Ok(Self {
            bitmap,
            base,
            frames,
            cursor: 0,
            used: frames,
        })
    }

    pub const fn total_frames(&self) -> usize {
        self.frames
    }

    pub const fn used_frames(&self) -> usize {
        self.used
    }

    pub const fn free_frames(&self) -> usize {
        self.frames - self.used
    }

    /// Marks the frames overlapping `[start, end)` as usable.
    ///
    /// The range is narrowed to whole frames that lie *entirely* inside it — a frame only
    /// partially covered by a usable region may have its remainder owned by firmware, and handing
    /// it out would corrupt whatever lives there.
    pub fn mark_usable(&mut self, start: PhysAddr, end: PhysAddr) {
        self.for_each_index(start.frame_ceil(), end.frame_floor(), |s, i| s.clear(i));
    }

    /// Marks the frames overlapping `[start, end)` as reserved.
    ///
    /// Here the range is widened to every frame it touches at all, for the mirror-image reason: a
    /// partially reserved frame must not be allocated.
    pub fn mark_reserved(&mut self, start: PhysAddr, end: PhysAddr) {
        self.for_each_index(start.frame_floor(), end.frame_ceil(), |s, i| s.set(i));
    }

    /// Allocates one frame.
    pub fn alloc(&mut self) -> Result<Frame, AllocError> {
        self.alloc_contiguous(1)
    }

    /// Allocates `count` physically contiguous frames — needed for page tables and for devices
    /// that do DMA without an IOMMU.
    ///
    /// Searches from the cursor and wraps once, so a long-lived allocator does not rescan an
    /// exhausted prefix on every call.
    pub fn alloc_contiguous(&mut self, count: usize) -> Result<Frame, AllocError> {
        if count == 0 {
            return Err(AllocError::ZeroFrames);
        }
        if self.free_frames() < count {
            return Err(AllocError::OutOfMemory);
        }
        if let Some(start) = self.find_run(self.cursor, self.frames, count) {
            return Ok(self.take(start, count));
        }
        if let Some(start) = self.find_run(0, self.cursor.min(self.frames), count) {
            return Ok(self.take(start, count));
        }
        Err(AllocError::Fragmented)
    }

    /// Returns `count` frames starting at `frame`.
    ///
    /// Double-freeing is a caller bug that would corrupt the used count and hand the same memory
    /// to two owners, so it is reported rather than tolerated.
    pub fn free_contiguous(&mut self, frame: Frame, count: usize) -> Result<(), &'static str> {
        let start = self
            .index_of(frame)
            .ok_or("freed frame is outside this allocator")?;
        if start + count > self.frames {
            return Err("freed range extends past the end of the allocator");
        }
        for i in start..start + count {
            if !self.is_set(i) {
                return Err("double free: frame was already marked free");
            }
            self.clear(i);
        }
        // Reuse recently freed memory first: it is the most likely to still be cached.
        self.cursor = start;
        Ok(())
    }

    pub fn free(&mut self, frame: Frame) -> Result<(), &'static str> {
        self.free_contiguous(frame, 1)
    }

    fn find_run(&self, from: usize, to: usize, count: usize) -> Option<usize> {
        let mut run = 0usize;
        for i in from..to {
            if self.is_set(i) {
                run = 0;
                continue;
            }
            run += 1;
            if run == count {
                return Some(i + 1 - count);
            }
        }
        None
    }

    fn take(&mut self, start: usize, count: usize) -> Frame {
        for i in start..start + count {
            self.set(i);
        }
        self.cursor = start + count;
        Frame::containing(PhysAddr(self.base.0 + (start * FRAME_SIZE) as u64))
    }

    fn for_each_index(
        &mut self,
        start: PhysAddr,
        end: PhysAddr,
        mut f: impl FnMut(&mut Self, usize),
    ) {
        if end <= start {
            return;
        }
        let first = self.index_of(Frame::containing(start)).unwrap_or(0);
        let last = match self.index_of(Frame::containing(end)) {
            Some(i) => i,
            // Past the end of our span: clamp rather than ignore, so a region that starts inside
            // and runs off the end is still applied to the part we own.
            None if end.0 > self.base.0 => self.frames,
            None => return,
        };
        for i in first..last.min(self.frames) {
            f(self, i);
        }
    }

    fn index_of(&self, frame: Frame) -> Option<usize> {
        let addr = frame.start().0;
        if addr < self.base.0 {
            return None;
        }
        let index = ((addr - self.base.0) / FRAME_SIZE as u64) as usize;
        (index < self.frames).then_some(index)
    }

    fn is_set(&self, i: usize) -> bool {
        self.bitmap[i / 8] & (1 << (i % 8)) != 0
    }

    /// Both `set` and `clear` maintain `used`, so every path that changes a bit keeps the count
    /// honest. An earlier version incremented only in `alloc`, which meant `mark_reserved` moved
    /// bits without moving the count and the allocator believed it had memory it had reserved.
    fn set(&mut self, i: usize) {
        if !self.is_set(i) {
            self.used += 1;
        }
        self.bitmap[i / 8] |= 1 << (i % 8);
    }

    fn clear(&mut self, i: usize) {
        if self.is_set(i) {
            self.used -= 1;
        }
        self.bitmap[i / 8] &= !(1 << (i % 8));
    }
}

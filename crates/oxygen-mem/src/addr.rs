//! Physical address and frame types.
//!
//! Physical and virtual addresses are both integers and are catastrophic to confuse, so they get
//! distinct types rather than a `u64` alias and a naming convention. The cost is a few explicit
//! conversions; the benefit is that passing one where the other belongs stops compiling.

/// Size of a physical frame. 4 KiB is the smallest granule every target we care about supports,
/// and small granules waste less on machines with little memory — which is the point of this OS.
pub const FRAME_SIZE: usize = 4096;

/// A physical address. Never dereferenceable directly once paging is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Rounds down to the frame containing this address.
    pub const fn frame_floor(self) -> PhysAddr {
        PhysAddr(self.0 & !(FRAME_SIZE as u64 - 1))
    }

    /// Rounds up to the next frame boundary, saturating rather than wrapping at the top of the
    /// address space — a wrap here would silently produce an address inside low memory.
    pub const fn frame_ceil(self) -> PhysAddr {
        match self.0.checked_add(FRAME_SIZE as u64 - 1) {
            Some(v) => PhysAddr(v & !(FRAME_SIZE as u64 - 1)),
            None => PhysAddr(!(FRAME_SIZE as u64 - 1)),
        }
    }

    pub const fn is_frame_aligned(self) -> bool {
        self.0 & (FRAME_SIZE as u64 - 1) == 0
    }
}

/// A single physical frame, identified by its base address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frame(PhysAddr);

impl Frame {
    /// The frame containing `addr`.
    pub const fn containing(addr: PhysAddr) -> Frame {
        Frame(addr.frame_floor())
    }

    pub const fn start(self) -> PhysAddr {
        self.0
    }
}

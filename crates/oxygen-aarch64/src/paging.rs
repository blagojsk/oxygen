//! AArch64 translation-table descriptors.
//!
//! This is the highest-risk arithmetic in the kernel. A descriptor with one wrong bit does not
//! raise an error — the CPU either faults on an address you believed was mapped, or, far worse,
//! enabling the MMU stops the machine at the very next instruction with no output and no fault.
//! There is nothing to print to and nothing to catch. So the encoding lives here, away from the
//! hardware, where every bit position can be asserted on the host.
//!
//! Layout in use: 4 KiB granule with a 39-bit virtual address space, which gives three levels —
//! L1 mapping 1 GiB blocks, L2 mapping 2 MiB blocks, L3 mapping 4 KiB pages. Three levels rather
//! than four because a smaller address space needs one less table to walk and one less table to
//! store, and on the machines this OS targets that is memory worth not spending.

/// Bytes spanned by one entry at each level, with a 4 KiB granule.
pub const L1_BLOCK_SIZE: u64 = 1 << 30; // 1 GiB
pub const L2_BLOCK_SIZE: u64 = 1 << 21; // 2 MiB
pub const L3_PAGE_SIZE: u64 = 1 << 12; // 4 KiB

/// Entries per table. 4 KiB / 8 bytes per descriptor.
pub const ENTRIES: usize = 512;

/// Index into `MAIR_EL1` for device memory: strongly ordered, no gathering, reordering or early
/// write acknowledgement. Anything else lets the CPU coalesce or reorder MMIO, which turns a
/// correct driver into an intermittently broken one.
pub const ATTR_DEVICE: u64 = 0;
/// Index for ordinary RAM: write-back cacheable.
pub const ATTR_NORMAL: u64 = 1;

/// `MAIR_EL1` value pairing the two indices above.
///
/// 0x00 is Device-nGnRnE. 0xFF is Normal memory, inner and outer write-back non-transient with
/// read and write allocate — the fastest ordinary-memory setting.
pub const MAIR_VALUE: u64 = (0xFF << (8 * ATTR_NORMAL)) | (0x00 << (8 * ATTR_DEVICE));

// Descriptor bits, from the ARM Architecture Reference Manual (D5, VMSAv8-64).
const VALID: u64 = 1 << 0;
/// Bit 1 distinguishes a block from a table at L1/L2 — and at L3 a *page* also sets it, which is
/// the single most confusing part of the format: the same bit means opposite things by level.
const TABLE_OR_PAGE: u64 = 1 << 1;
const ATTR_INDEX_SHIFT: u64 = 2;
const AP_SHIFT: u64 = 6;
const SH_SHIFT: u64 = 8;
/// Access flag. If this is clear the first touch raises an access fault, and a kernel that does
/// not handle those simply stops. Every mapping made here sets it.
const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

/// Output address bits. The address occupies [47:12]; everything else is attributes.
const ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

/// Who may touch a mapping, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Kernel read/write, never executable. Data.
    KernelData,
    /// Kernel read-only and executable. Code.
    KernelCode,
    /// Kernel read-only, never executable. Constants.
    KernelReadOnly,
    /// User read-only and executable at EL0. The kernel may read it and must never execute it.
    ///
    /// PXN is what makes the second half true. Without it, any kernel bug that transfers control
    /// to a user address runs user-supplied bytes with kernel authority — the single cheapest
    /// privilege escalation there is, and one page-table bit removes it.
    UserCode,
    /// User read/write, never executable at either level.
    ///
    /// The kernel can write it too, which is how arguments and results cross the boundary. What
    /// it cannot do is run it, so a buffer a user filled is inert no matter who jumps to it.
    UserData,
}

impl Access {
    const fn bits(self) -> u64 {
        match self {
            // AP=00 is EL1 read/write with no EL0 access. Both execute-never bits set, because
            // data should never be executable — this is what makes an overflowed buffer inert.
            Access::KernelData => (0b00 << AP_SHIFT) | PXN | UXN,
            // AP=10 is EL1 read-only. UXN still set: kernel code is not for userspace to run.
            Access::KernelCode => (0b10 << AP_SHIFT) | UXN,
            Access::KernelReadOnly => (0b10 << AP_SHIFT) | PXN | UXN,
            // AP=11 is read-only at both levels. UXN clear so EL0 can execute; PXN set so EL1
            // cannot. Writable user code would defeat W^X on the only pages a user controls.
            Access::UserCode => (0b11 << AP_SHIFT) | PXN,
            // AP=01 is read/write at both levels — the kernel needs write access to load a
            // program and to deliver results into it.
            Access::UserData => (0b01 << AP_SHIFT) | PXN | UXN,
        }
    }
}

/// Shareability. Inner-shareable is the correct default for RAM on a multicore system: it makes
/// the hardware keep caches coherent between cores. Device memory is marked outer-shareable
/// because it is not cached and coherence is meaningless for it.
const SH_INNER: u64 = 0b11 << SH_SHIFT;
const SH_OUTER: u64 = 0b10 << SH_SHIFT;

/// What kind of memory a mapping describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// Ordinary RAM: cacheable, coherent between cores.
    Normal,
    /// MMIO: uncached and strongly ordered, so accesses reach the device in program order.
    Device,
}

impl MemoryKind {
    const fn bits(self) -> u64 {
        match self {
            MemoryKind::Normal => (ATTR_NORMAL << ATTR_INDEX_SHIFT) | SH_INNER,
            MemoryKind::Device => (ATTR_DEVICE << ATTR_INDEX_SHIFT) | SH_OUTER,
        }
    }
}

/// A block descriptor for L1 or L2 — one entry covering a large, naturally aligned span.
///
/// Blocks rather than pages wherever a region allows it: one L1 descriptor maps a gigabyte, where
/// 4 KiB pages would need 262,144 descriptors and the two megabytes of tables to hold them. On a
/// machine with 64 MiB of RAM that difference is the whole point.
pub const fn block(pa: u64, kind: MemoryKind, access: Access) -> u64 {
    (pa & ADDR_MASK) | kind.bits() | access.bits() | AF | VALID
}

/// A page descriptor for L3. Note it sets the same bit a table descriptor uses at higher levels.
pub const fn page(pa: u64, kind: MemoryKind, access: Access) -> u64 {
    block(pa, kind, access) | TABLE_OR_PAGE
}

/// A descriptor pointing at the next-level table. Attributes are carried by the leaf, not here.
pub const fn table(next_table_pa: u64) -> u64 {
    (next_table_pa & ADDR_MASK) | TABLE_OR_PAGE | VALID
}

pub const fn is_valid(descriptor: u64) -> bool {
    descriptor & VALID != 0
}

/// The address a descriptor points at, whether that is a block of memory or the next table.
pub const fn output_address(descriptor: u64) -> u64 {
    descriptor & ADDR_MASK
}

/// Index into the table at `level` (1, 2 or 3) for a virtual address.
pub const fn index_for(va: u64, level: u8) -> usize {
    let shift = match level {
        1 => 30,
        2 => 21,
        _ => 12,
    };
    ((va >> shift) as usize) & (ENTRIES - 1)
}

/// `TCR_EL1` for the layout above.
///
/// `t0sz` is expressed as 64 minus the address-space width, so 25 gives 39 bits. TTBR1 is disabled
/// outright: until the kernel moves to the top half of the address space there is no second table,
/// and leaving translation enabled for a region with no table invites a fault with no handler.
pub const fn tcr_el1(va_bits: u64, pa_range: u64) -> u64 {
    let t0sz = 64 - va_bits;
    // IRGN0/ORGN0 = write-back write-allocate cacheable page-table walks; SH0 = inner shareable.
    // Uncached walks are legal and drastically slower — every TLB miss becomes a memory round trip.
    let irgn0 = 0b01 << 8;
    let orgn0 = 0b01 << 10;
    let sh0 = 0b11 << 12;
    // TG0 = 0b00 selects the 4 KiB granule.
    let tg0 = 0b00 << 14;
    // EPD1 disables TTBR1 walks entirely.
    let epd1 = 1 << 23;
    t0sz | irgn0 | orgn0 | sh0 | tg0 | epd1 | (pa_range << 32)
}

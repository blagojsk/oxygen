//! AArch64 encodings that are pure arithmetic, and so can be tested off-hardware.
//!
//! Register and descriptor formats are architecture-specific but they are still just bit
//! manipulation. Keeping them out of the kernel binary means they can be asserted on the host,
//! which matters most for exactly the code whose failure mode is a silent hang.

#![no_std]

pub mod paging;

#[cfg(test)]
mod tests {
    use super::paging::*;

    const GIB: u64 = 1 << 30;

    /// Every mapping must set the access flag. Without it the first touch raises an access fault,
    /// and a kernel with no handler for that just stops.
    #[test]
    fn every_mapping_sets_the_access_flag() {
        let af = 1 << 10;
        assert_ne!(block(GIB, MemoryKind::Normal, Access::KernelData) & af, 0);
        assert_ne!(page(GIB, MemoryKind::Normal, Access::KernelCode) & af, 0);
    }

    #[test]
    fn descriptors_are_valid_and_carry_their_address() {
        let d = block(2 * GIB, MemoryKind::Normal, Access::KernelData);
        assert!(is_valid(d));
        assert_eq!(output_address(d), 2 * GIB);
    }

    /// Bit 1 is the format's worst trap: at L1/L2 it means "table", at L3 it means "page".
    /// A block with it set is read as a table pointer and the walk follows garbage.
    #[test]
    fn blocks_clear_bit1_and_pages_set_it() {
        let bit1 = 1 << 1;
        assert_eq!(block(GIB, MemoryKind::Normal, Access::KernelData) & bit1, 0);
        assert_ne!(page(GIB, MemoryKind::Normal, Access::KernelData) & bit1, 0);
        assert_ne!(table(GIB) & bit1, 0);
    }

    /// Device memory must be uncached and strongly ordered, or the CPU may reorder or coalesce
    /// MMIO writes and a correct driver becomes intermittently wrong.
    #[test]
    fn device_and_normal_select_different_mair_indices() {
        let idx = |d: u64| (d >> 2) & 0b111;
        assert_eq!(
            idx(block(0, MemoryKind::Device, Access::KernelData)),
            ATTR_DEVICE
        );
        assert_eq!(
            idx(block(GIB, MemoryKind::Normal, Access::KernelData)),
            ATTR_NORMAL
        );
    }

    #[test]
    fn mair_pairs_device_and_writeback_normal() {
        assert_eq!((MAIR_VALUE >> (8 * ATTR_DEVICE)) & 0xFF, 0x00);
        assert_eq!((MAIR_VALUE >> (8 * ATTR_NORMAL)) & 0xFF, 0xFF);
    }

    /// Data must never be executable — this is what makes an overflowed buffer inert rather than
    /// a way to run code.
    #[test]
    fn data_is_never_executable() {
        let d = block(GIB, MemoryKind::Normal, Access::KernelData);
        assert_ne!(d & (1 << 53), 0, "PXN must be set on data");
        assert_ne!(d & (1 << 54), 0, "UXN must be set on data");
    }

    /// Kernel code must stay executable at EL1, or enabling the MMU stops the machine on the very
    /// next instruction fetch.
    #[test]
    fn kernel_code_is_executable_at_el1_but_not_el0() {
        let d = block(GIB, MemoryKind::Normal, Access::KernelCode);
        assert_eq!(d & (1 << 53), 0, "PXN must be clear on kernel code");
        assert_ne!(d & (1 << 54), 0, "UXN must still be set");
    }

    /// The mapping the first identity map needs: writable so the stack works, executable so the
    /// instruction after the MMU is enabled can be fetched. Getting either wrong stops the machine
    /// with no diagnostic at all.
    #[test]
    fn rwx_is_writable_and_executable_at_el1() {
        let d = block(GIB, MemoryKind::Normal, Access::KernelRwx);
        assert_eq!((d >> 6) & 0b11, 0b00, "must be EL1 read/write");
        assert_eq!(d & (1 << 53), 0, "PXN must be clear so EL1 can execute it");
        assert_ne!(
            d & (1 << 54),
            0,
            "UXN must be set: EL0 has no business here"
        );
    }

    #[test]
    fn kernel_data_is_writable_and_code_is_not() {
        let ap = |d: u64| (d >> 6) & 0b11;
        assert_eq!(ap(block(GIB, MemoryKind::Normal, Access::KernelData)), 0b00);
        assert_eq!(ap(block(GIB, MemoryKind::Normal, Access::KernelCode)), 0b10);
        assert_eq!(
            ap(block(GIB, MemoryKind::Normal, Access::KernelReadOnly)),
            0b10
        );
    }

    /// RAM is inner-shareable so cores stay coherent; device memory is not cached, so coherence
    /// does not apply to it.
    #[test]
    fn shareability_differs_between_ram_and_mmio() {
        let sh = |d: u64| (d >> 8) & 0b11;
        assert_eq!(sh(block(GIB, MemoryKind::Normal, Access::KernelData)), 0b11);
        assert_eq!(sh(block(0, MemoryKind::Device, Access::KernelData)), 0b10);
    }

    /// Attribute bits must never bleed into the address field, or a mapping silently points
    /// somewhere else entirely.
    #[test]
    fn attributes_never_corrupt_the_output_address() {
        for kind in [MemoryKind::Normal, MemoryKind::Device] {
            for access in [
                Access::KernelData,
                Access::KernelCode,
                Access::KernelReadOnly,
            ] {
                assert_eq!(output_address(block(3 * GIB, kind, access)), 3 * GIB);
            }
        }
    }

    /// The walk indexes each level with a different slice of the address; getting the shift wrong
    /// maps the right memory to the wrong place.
    #[test]
    fn indices_select_the_right_slice_per_level() {
        assert_eq!(index_for(0, 1), 0);
        assert_eq!(index_for(GIB, 1), 1);
        assert_eq!(index_for(511 * GIB, 1), 511);
        // Wraps at 512 GiB, the top of a 39-bit space.
        assert_eq!(index_for(512 * GIB, 1), 0);

        assert_eq!(index_for(L2_BLOCK_SIZE, 2), 1);
        assert_eq!(index_for(L3_PAGE_SIZE, 3), 1);
    }

    #[test]
    fn a_39_bit_space_is_encoded_as_t0sz_25() {
        let tcr = tcr_el1(39, 0b010);
        assert_eq!(tcr & 0x3F, 25);
        // 4 KiB granule.
        assert_eq!((tcr >> 14) & 0b11, 0b00);
        // TTBR1 disabled: there is no second table yet, and translating against a missing one
        // faults with nothing to handle it.
        assert_ne!(tcr & (1 << 23), 0);
        // Page-table walks cacheable; uncached walks make every TLB miss a memory round trip.
        assert_eq!((tcr >> 8) & 0b11, 0b01);
        assert_eq!((tcr >> 10) & 0b11, 0b01);
    }

    #[test]
    fn table_descriptors_point_at_the_next_level() {
        let t = table(0x4020_1000);
        assert!(is_valid(t));
        assert_eq!(output_address(t), 0x4020_1000);
    }

    #[test]
    fn an_invalid_descriptor_is_zero() {
        assert!(!is_valid(0));
    }
}

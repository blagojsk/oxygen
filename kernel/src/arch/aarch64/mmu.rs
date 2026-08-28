//! Turning on the MMU, and enforcing W^X.
//!
//! This is the most dangerous single step in the kernel. The instruction after `SCTLR_EL1.M` is
//! set is fetched through the translation tables, so if the code we are executing is not mapped —
//! correctly, executable, with the access flag set — the machine stops there. No fault, no output,
//! nothing to debug with.
//!
//! The defence is to make the change as close to a no-op as possible: an *identity* map, where
//! every virtual address equals the physical address it already had. Execution continues at the
//! same addresses and only the attributes change.
//!
//! Granularity is chosen per region rather than uniformly, because both extremes are wrong. Paging
//! all of RAM at 4 KiB would need 262,144 descriptors and two megabytes of tables — most of what a
//! 64 MiB machine came for. Mapping it all with one gigabyte block makes the kernel's code and its
//! stack share permissions, which is how the first version of this file ended up mapping RAM
//! read-write-execute. So: 4 KiB pages across the kernel image, where protection matters, and
//! large blocks everywhere else, where it does not.

use core::sync::atomic::{Ordering, compiler_fence};

use oxygen_aarch64::paging::{
    self, Access, ENTRIES, L1_BLOCK_SIZE, L2_BLOCK_SIZE, L3_PAGE_SIZE, MAIR_VALUE, MemoryKind,
};

use crate::println;

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

fn sym(s: &u8) -> u64 {
    s as *const u8 as u64
}

/// A translation table: 512 descriptors, 4 KiB, aligned as TTBR and table descriptors require.
#[repr(C, align(4096))]
struct Table([u64; ENTRIES]);

impl Table {
    const fn new() -> Self {
        Table([0; ENTRIES])
    }
}

/// Static rather than allocated: the frame allocator needs memory described before it can hand any
/// out, so the first tables have to exist before it does.
static mut L1: Table = Table::new();
static mut L2_RAM: Table = Table::new();
static mut L3_KERNEL: Table = Table::new();

/// RAM base on this board. The frame allocator will learn the real extent from the device tree;
/// the mapping only needs to know where the kernel lives.
const RAM_BASE: u64 = L1_BLOCK_SIZE;

/// How much RAM the identity map covers. One L1 entry's worth, which is more than the machines
/// this OS targets are likely to have.
const MAPPED_RAM: u64 = L1_BLOCK_SIZE;

/// Builds the identity map and enables translation.
///
/// # Safety
/// Runs once, on the boot core, with the MMU off.
pub unsafe fn init() {
    let l1 = &raw mut L1;
    let l2 = &raw mut L2_RAM;
    let l3 = &raw mut L3_KERNEL;

    let kernel_start = sym(unsafe { &__kernel_start });
    let kernel_end = sym(unsafe { &__kernel_end });

    // The kernel image is mapped by a single L3 table, which spans 2 MiB. Growing past that is not
    // a subtle failure — the tail of the image would simply be unmapped — so it is refused here,
    // while there is still a console to say so on.
    assert!(
        kernel_end - kernel_start <= L2_BLOCK_SIZE,
        "kernel image exceeds the 2 MiB covered by one L3 table"
    );

    // SAFETY: single-threaded boot path, sole writer of these tables, all indices in bounds.
    unsafe {
        // Everything below RAM is memory-mapped I/O on this board — the UART and the GIC both
        // live there. Device attributes, never executable.
        (*l1).0[0] = paging::block(0, MemoryKind::Device, Access::KernelData);

        // RAM is described by an L2 table so the first 2 MiB can be refined further.
        (*l1).0[1] = paging::table(l2 as u64);

        // The 2 MiB containing the kernel image goes to an L3 table; the rest of RAM is 2 MiB
        // blocks of ordinary read/write data. Not executable: nothing outside the kernel image
        // has any business being run, and saying so is most of what W^X buys.
        (*l2).0[0] = paging::table(l3 as u64);
        for i in 1..ENTRIES {
            let pa = RAM_BASE + (i as u64) * L2_BLOCK_SIZE;
            if pa >= RAM_BASE + MAPPED_RAM {
                break;
            }
            (*l2).0[i] = paging::block(pa, MemoryKind::Normal, Access::KernelData);
        }

        // Now the kernel image itself, one 4 KiB page at a time, each with the narrowest
        // permissions its section can tolerate.
        let text = (sym(&__text_start), sym(&__text_end), Access::KernelCode);
        let rodata = (
            sym(&__rodata_start),
            sym(&__rodata_end),
            Access::KernelReadOnly,
        );
        let data = (sym(&__data_start), kernel_end, Access::KernelData);

        for (start, end, access) in [text, rodata, data] {
            let mut pa = start;
            while pa < end {
                let index = paging::index_for(pa, 3);
                (*l3).0[index] = paging::page(pa, MemoryKind::Normal, access);
                pa += L3_PAGE_SIZE;
            }
        }

        // The rest of the 2 MiB this L3 table covers is ordinary RAM and must be mapped too.
        // Leaving it blank is not harmless: the frame allocator hands out memory immediately above
        // the kernel image, so the very first heap frame lands here and faults on a page that was
        // never mapped. Read/write, never executable — the kernel image above is the only
        // executable region in the system.
        let mut pa = kernel_end;
        while pa < RAM_BASE + L2_BLOCK_SIZE {
            let index = paging::index_for(pa, 3);
            (*l3).0[index] = paging::page(pa, MemoryKind::Normal, Access::KernelData);
            pa += L3_PAGE_SIZE;
        }
    }

    let ttbr0 = l1 as u64;

    // SAFETY: the sequence below is order-critical and each step is required.
    unsafe {
        core::arch::asm!(
            // Attributes and layout must be in place before translation is switched on: the CPU
            // reads MAIR and TCR as part of the very first walk.
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr0}",
            // The tables were written with the MMU off, so those stores must be visible to the
            // page-table walker before it runs, and every stale translation discarded.
            "dsb ish",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            // M enables translation, C the data cache, I the instruction cache. Caches come on
            // with the MMU because their behaviour depends on attributes that only exist once
            // translation is live.
            "mrs {tmp}, sctlr_el1",
            "orr {tmp}, {tmp}, #(1 << 0)",
            "orr {tmp}, {tmp}, #(1 << 2)",
            "orr {tmp}, {tmp}, #(1 << 12)",
            "msr sctlr_el1, {tmp}",
            // Nothing after this point is fetched through the old regime.
            "isb",
            mair = in(reg) MAIR_VALUE,
            tcr = in(reg) paging::tcr_el1(39, 0b010),
            ttbr0 = in(reg) ttbr0,
            tmp = out(reg) _,
            options(nostack),
        );
    }
    compiler_fence(Ordering::SeqCst);

    println!(
        "  [mmu]  translation on — {} KiB kernel image at 4 KiB pages, W^X enforced",
        (kernel_end - kernel_start) / 1024
    );
}

/// End of the kernel image, page-aligned. Everything above this is free RAM as far as the frame
/// allocator is concerned; everything below is the kernel and must never be handed out.
pub fn kernel_end() -> u64 {
    sym(unsafe { &__kernel_end })
}

/// Address of a byte inside `.text`, for the selftest that proves code is not writable.
pub fn text_address() -> u64 {
    sym(unsafe { &__text_start })
}

/// Whether translation is currently enabled.
pub fn is_enabled() -> bool {
    let sctlr: u64;
    // SAFETY: SCTLR_EL1 is readable at EL1 and reading it has no side effects.
    unsafe { core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack)) };
    sctlr & 1 != 0
}

//! Turning on the MMU.
//!
//! This is the most dangerous single step in the kernel. The instruction after `SCTLR_EL1.M` is
//! set is fetched through the translation tables, so if the code we are executing is not mapped —
//! correctly, executable, with the access flag set — the machine stops there. No fault, no output,
//! nothing to debug with. Every other bug in this file looks identical from the outside.
//!
//! The defence is to make the change as close to a no-op as possible: build an *identity* map, so
//! every virtual address equals the physical address it already had. Execution continues at the
//! same addresses it was already using, and only the attributes change.
//!
//! QEMU's `virt` board makes that unusually tidy. Devices live below 1 GiB and RAM starts exactly
//! at the 1 GiB boundary, so two L1 block descriptors — one gigabyte each — cover everything we
//! need. Two descriptors, one 4 KiB table, no allocation.

use core::sync::atomic::{Ordering, compiler_fence};

use oxygen_aarch64::paging::{self, Access, ENTRIES, L1_BLOCK_SIZE, MAIR_VALUE, MemoryKind};

use crate::println;

/// One L1 table. 512 entries of 1 GiB each spans the whole 512 GiB address space this layout
/// allows, though only the first two entries are populated.
///
/// Static rather than allocated because the frame allocator needs memory described before it can
/// hand any out, and this must exist before that. 4 KiB alignment is architectural: TTBR0 holds a
/// table address whose low twelve bits must be zero.
#[repr(C, align(4096))]
struct Table([u64; ENTRIES]);

static mut L1: Table = Table([0; ENTRIES]);

/// Where the identity map stops. Two gigabytes covers QEMU `virt`'s devices and its first
/// gigabyte of RAM, which is more than the machines this OS targets are likely to have.
const MAPPED_LIMIT: u64 = 2 * L1_BLOCK_SIZE;

/// Builds the identity map and enables translation.
///
/// # Safety
/// Runs once, on the boot core, with the MMU off. The caller must not rely on any mapping other
/// than the identity map this installs.
pub unsafe fn init() {
    // SAFETY: single-threaded boot path, before any other core is running, and the only writer of
    // this table. Taking a raw pointer rather than a reference keeps us clear of aliasing a static
    // mut through a shared reference.
    let l1 = &raw mut L1;

    // Entry 0: everything below 1 GiB is memory-mapped I/O on this board — the UART and the GIC
    // both live here. Device attributes, and never executable.
    // Entry 1: RAM. Normal cacheable memory, and executable, because the kernel is running from it
    // and the next instruction fetch after enabling the MMU comes from here.
    //
    // The whole gigabyte of RAM is mapped read/write/execute, which is not a permission scheme so
    // much as the absence of one. It has to be: one descriptor covers both the kernel's code and
    // its stack, so it needs execute for the instruction fetched after translation comes on and
    // write for the first push. Mapping it read-only stops the machine instantly — the push
    // faults, the fault handler pushes, and the recursive fault is unrecoverable. Splitting the
    // image into read-execute text and read-write data needs linker symbols and a finer-grained
    // table; until then W^X is a known gap, not a decision.
    // SAFETY: writing two entries of a table we exclusively own, at indices within its bounds.
    unsafe {
        (*l1).0[0] = paging::block(0, MemoryKind::Device, Access::KernelData);
        (*l1).0[1] = paging::block(L1_BLOCK_SIZE, MemoryKind::Normal, Access::KernelRwx);
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
            // The table was written with the MMU off, so those stores must be visible to the page
            // table walker before it runs, and every earlier translation must be discarded.
            "dsb ish",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            // M enables translation, C enables the data cache, I the instruction cache. Caches are
            // turned on together with the MMU because their behaviour depends on the memory
            // attributes that only exist once translation is live.
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
        "  [mmu]  translation on — identity mapped to {} MiB",
        MAPPED_LIMIT >> 20
    );
}

/// Whether translation is currently enabled. Used by the selftest: reaching the next instruction
/// proves the map is sound, and reading the bit back proves it was actually switched on rather
/// than the write being silently dropped.
pub fn is_enabled() -> bool {
    let sctlr: u64;
    // SAFETY: SCTLR_EL1 is readable at EL1 and reading it has no side effects.
    unsafe { core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack)) };
    sctlr & 1 != 0
}

//! GICv2 — the ARM Generic Interrupt Controller.
//!
//! Chosen because it is what the cheap boards this OS targets actually have: QEMU's `virt` board
//! defaults to GICv2, and the GIC-400 found on Pi-class hardware is a GICv2 implementation. GICv3
//! exists on newer server parts and is a different programming model (system registers rather than
//! MMIO for the CPU interface); it belongs in a sibling module when we need it.
//!
//! Two blocks matter. The *distributor* decides which interrupts exist, who they go to and at what
//! priority; the *CPU interface* is how this core acknowledges and ends them.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::println;

/// Distributor and CPU interface base addresses on QEMU `virt`.
///
/// Hardcoded for now. Real boards put these elsewhere, so this is exactly the kind of constant the
/// device tree will supply once we parse it — the addresses are a property of the board, not of
/// the architecture.
const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;

// Distributor registers (offsets from GICD_BASE).
const GICD_CTLR: usize = 0x000;
const GICD_TYPER: usize = 0x004;
const GICD_ISENABLER: usize = 0x100;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_ITARGETSR: usize = 0x800;

// CPU interface registers (offsets from GICC_BASE).
const GICC_CTLR: usize = 0x00;
const GICC_PMR: usize = 0x04;
const GICC_IAR: usize = 0x0C;
const GICC_EOIR: usize = 0x10;

/// Interrupt IDs 1020 and above are special values, not real interrupts. 1023 in particular means
/// "spurious": the interrupt went away before we acknowledged it, which is normal and must not be
/// treated as a device needing service.
const SPURIOUS: u32 = 1023;

/// Counted so a test can assert interrupts are actually being delivered rather than inferring it
/// from the absence of a hang.
static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

fn gicd_read(off: usize) -> u32 {
    // SAFETY: fixed MMIO in the distributor block; volatile so it is never elided or reordered
    // against the other device accesses around it.
    unsafe { core::ptr::read_volatile((GICD_BASE + off) as *const u32) }
}

fn gicd_write(off: usize, value: u32) {
    // SAFETY: as above, for a writable distributor register.
    unsafe { core::ptr::write_volatile((GICD_BASE + off) as *mut u32, value) }
}

fn gicc_read(off: usize) -> u32 {
    // SAFETY: fixed MMIO in this core's CPU-interface block.
    unsafe { core::ptr::read_volatile((GICC_BASE + off) as *const u32) }
}

fn gicc_write(off: usize, value: u32) {
    // SAFETY: as above, for a writable CPU-interface register.
    unsafe { core::ptr::write_volatile((GICC_BASE + off) as *mut u32, value) }
}

/// Brings up the distributor and this core's CPU interface.
///
/// # Safety
/// Touches board MMIO and must run once, on the boot core, before interrupts are unmasked.
pub unsafe fn init() {
    // GICD_TYPER's low five bits encode the number of interrupt lines in blocks of 32.
    let lines = ((gicd_read(GICD_TYPER) & 0x1F) + 1) * 32;

    gicd_write(GICD_CTLR, 0);

    // Everything starts at the lowest priority and targeted at core 0. SGIs and PPIs (IDs 0..31)
    // are banked per core and have no targets register, so both loops start at 32.
    for i in (32..lines as usize).step_by(4) {
        gicd_write(GICD_IPRIORITYR + i, 0xA0A0_A0A0);
        gicd_write(GICD_ITARGETSR + i, 0x0101_0101);
    }

    gicd_write(GICD_CTLR, 1);

    // Priority mask must be *numerically higher* than an interrupt's priority for it to be
    // delivered — lower numbers are more urgent on the GIC. 0xFF admits everything.
    gicc_write(GICC_PMR, 0xFF);
    gicc_write(GICC_CTLR, 1);

    println!("  [gic] gicv2 up, {lines} interrupt lines");
}

/// Enables one interrupt ID for this core.
///
/// # Safety
/// Touches distributor MMIO; the caller must have run [`init`].
pub unsafe fn enable(intid: u32) {
    let reg = (intid / 32) as usize * 4;
    let bit = 1u32 << (intid % 32);
    gicd_write(GICD_ISENABLER + reg, bit);
}

/// Called from the IRQ vector.
///
/// The acknowledge/end pairing is not optional: reading IAR takes ownership of the interrupt and
/// raises the running priority, and until the matching EOIR write the GIC will deliver nothing
/// else at that priority. A missed EOIR looks exactly like a dead interrupt controller.
pub fn handle_irq() {
    let iar = gicc_read(GICC_IAR);
    let intid = iar & 0x3FF;

    if intid >= SPURIOUS {
        return;
    }

    match intid {
        super::timer::TIMER_INTID => {
            TICKS.fetch_add(1, Ordering::Relaxed);
            super::timer::rearm();
            // Mark, do not switch. Switching here would leave the interrupt unacknowledged for as
            // long as the next thread runs, and the GIC delivers nothing else at this priority
            // until it sees the EOI below.
            crate::sched::request_reschedule();
        }
        super::uart::UART_INTID => super::uart::handle_irq(),
        other => println!("  [gic] unexpected interrupt {other}"),
    }

    gicc_write(GICC_EOIR, iar);
}

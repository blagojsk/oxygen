//! The ARM generic timer.
//!
//! Architected rather than board-specific, which is why it is the right clock to build on: every
//! ARMv8 core has one, so the same code ticks on QEMU and on a Pi without a per-board driver.
//!
//! The physical timer counts down from `CNTP_TVAL_EL0` and fires when it crosses zero. It is a
//! one-shot, so a periodic tick means re-arming inside the handler.

use crate::println;

/// PPI 30: the non-secure physical timer for EL1. Private to each core, and fixed by the
/// architecture rather than discovered.
pub const TIMER_INTID: u32 = 30;

/// How often to tick. 100 Hz is a deliberate choice for weak hardware: fine enough for responsive
/// scheduling, coarse enough that interrupt overhead stays negligible on a slow core.
const TICK_HZ: u64 = 100;

fn frequency() -> u64 {
    let hz: u64;
    // SAFETY: CNTFRQ_EL0 is a read-only frequency report, readable at EL1.
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) hz, options(nomem, nostack)) };
    hz
}

fn interval() -> u64 {
    // Guard against firmware leaving CNTFRQ at zero, which would otherwise arm a timer that fires
    // continuously and livelocks the core in its own handler.
    let hz = frequency();
    if hz == 0 { 0 } else { hz / TICK_HZ }
}

/// Starts the periodic tick.
///
/// # Safety
/// Writes timer system registers and enables a GIC line; requires the vector table and GIC to be
/// initialised first.
pub unsafe fn init() {
    let hz = frequency();
    if hz == 0 {
        println!("  [timer] CNTFRQ_EL0 is zero — refusing to arm");
        return;
    }
    // SAFETY: enabling this core's own physical timer, per the contract above.
    unsafe {
        super::gic::enable(TIMER_INTID);
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) interval(), options(nomem, nostack));
        // CNTP_CTL_EL0: bit 0 enables, bit 1 masks. Enable and leave unmasked.
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
    }
    println!("  [timer] {hz} Hz counter, ticking at {TICK_HZ} Hz");
}

/// Re-arms after a tick. Called from the IRQ handler, where the timer has already fired.
pub fn rearm() {
    // SAFETY: writing this core's timer value register; the timer is already enabled.
    unsafe {
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) interval(), options(nomem, nostack));
    }
}

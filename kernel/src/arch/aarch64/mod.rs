//! AArch64 support: boot, exceptions, interrupt controller, timer, console.

pub mod boot;
pub mod context;
pub mod exceptions;
pub mod gic;
pub mod mmu;
pub mod semihosting;
pub mod shell;
pub mod timer;
pub mod uart;
pub mod user;

/// Which exception level the firmware left us at. QEMU's `virt` board and most board firmware
/// hand control over at EL1; knowing this matters because system register names and behaviour
/// differ per level, and code written for one silently misbehaves at another.
pub fn current_el() -> u64 {
    let el: u64;
    // SAFETY: CurrentEL is readable at every exception level and has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
    }
    (el >> 2) & 0b11
}

/// Unmasks IRQs. Nothing is delivered before this, so it is called once the vector table and the
/// interrupt controller are both ready — never earlier, or the first interrupt lands on a vector
/// base that still points at whatever the firmware left behind.
///
/// # Safety
/// The caller must have installed the vector table and initialised the GIC.
pub unsafe fn enable_irqs() {
    // SAFETY: delegated to the caller, per the contract above.
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack)) };
}

/// Masks IRQs and returns the previous interrupt state, for [`restore_irqs`] to put back.
///
/// This is what makes a lock safe to take in a thread and in the interrupt that would otherwise
/// preempt it. On one core, masking is enough: an interrupt that cannot arrive cannot try to take
/// a lock the interrupted code is already holding.
pub fn save_and_mask_irqs() -> u64 {
    let daif: u64;
    // SAFETY: reads DAIF and sets the I bit. Both are ordinary PSTATE manipulations at EL1, and
    // the read happens before the mask so the caller gets the state to restore.
    unsafe {
        core::arch::asm!(
            "mrs {out}, daif",
            "msr daifset, #2",
            out = out(reg) daif,
            options(nomem, nostack),
        )
    };
    daif
}

/// Puts back what [`save_and_mask_irqs`] returned.
///
/// # Safety
/// `state` must be a value that call returned, and the caller must be finished with whatever the
/// masking was protecting.
pub unsafe fn restore_irqs(state: u64) {
    // SAFETY: delegated to the caller, per the contract above.
    unsafe { core::arch::asm!("msr daif, {}", in(reg) state, options(nomem, nostack)) };
}

pub fn wait_for_interrupt() -> ! {
    loop {
        wait_for_interrupt_once();
    }
}

/// Parks the core until the next interrupt, then returns.
///
/// The single-shot form exists for a thread that has nothing to do but is not finished — it must
/// come back and re-check the condition it is waiting on. Spinning instead would be correct and
/// would also burn a whole core, which on a single-core board is the entire machine.
pub fn wait_for_interrupt_once() {
    // SAFETY: WFI parks the core until an interrupt arrives; it has no other effect.
    unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
}

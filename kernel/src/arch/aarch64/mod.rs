//! AArch64 support: boot, exceptions, interrupt controller, timer, console.

pub mod boot;
pub mod exceptions;
pub mod gic;
pub mod semihosting;
pub mod timer;
pub mod uart;

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

pub fn wait_for_interrupt() -> ! {
    loop {
        // SAFETY: WFI parks the core until an interrupt arrives; it has no other effect.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

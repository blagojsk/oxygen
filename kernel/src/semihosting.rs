//! Asking QEMU to shut down, with a status code.
//!
//! This is what makes a boot testable rather than merely observable: a test harness can run
//! the kernel and read an exit code instead of scraping the serial log. Semihosting is a
//! debug channel provided by the emulator — on real hardware these calls trap, so anything
//! built on this is development scaffolding, not a kernel facility.

use core::arch::asm;

const SYS_EXIT: u64 = 0x18;
/// ADP_Stopped_ApplicationExit — the "clean shutdown" reason code.
const APPLICATION_EXIT: u64 = 0x2_0026;

/// Terminates the machine. `code` 0 reports success to the host.
pub fn exit(code: u32) -> ! {
    let block = [APPLICATION_EXIT, code as u64];
    // SAFETY: the semihosting call convention for AArch64 — operation in w0, a pointer to
    // the parameter block in x1. If the host does not implement it the instruction traps,
    // and the loop below keeps us from running off into undefined memory either way.
    unsafe {
        asm!(
            "hlt #0xF000",
            in("x0") SYS_EXIT,
            in("x1") block.as_ptr(),
            options(nostack),
        );
    }
    loop {
        core::hint::spin_loop();
    }
}

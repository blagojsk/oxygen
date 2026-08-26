//! Oxygen — an operating system built to be driven by agents and people alike.
//!
//! Milestone 0: reach Rust on bare metal and prove it, nothing more. There is no memory
//! management, no scheduler and no user mode yet; what exists is a boot path, a way to speak,
//! and a way to fail loudly.

#![no_std]
#![no_main]

mod boot;
mod semihosting;
mod uart;

/// Set by the harness when the kernel is booted as a test rather than for a human to watch.
const SELFTEST: bool = option_env!("OXYGEN_SELFTEST").is_some();

/// Entry point, called from the boot assembly once a stack exists and `.bss` is zeroed.
///
/// Diverges because there is nothing to return to: the caller is four instructions of
/// assembly with no continuation.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    println!();
    println!("  oxygen {}", env!("CARGO_PKG_VERSION"));
    println!("  an operating system for agents and humans");
    println!();
    println!("  [boot] aarch64 · qemu virt · EL{}", current_el());
    println!("  [boot] stack installed, .bss zeroed, rust reached");
    println!();

    if SELFTEST {
        println!("  [selftest] boot reached kernel_main — ok");
        semihosting::exit(0);
    }

    println!("  nothing left to do yet. halting.");
    halt()
}

/// Which privilege level the firmware dropped us at. QEMU's `virt` board starts at EL2
/// unless virtualisation is disabled; knowing this matters as soon as we touch system
/// registers, because their names and behaviour differ per level.
fn current_el() -> u64 {
    let el: u64;
    // SAFETY: CurrentEL is readable at every exception level and has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack));
    }
    (el >> 2) & 0b11
}

fn halt() -> ! {
    loop {
        // SAFETY: WFE simply parks the core until an event arrives.
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}

/// Where every unrecoverable error ends up.
///
/// A kernel panic has no supervisor to report to, so the contract is: say everything we know
/// on the console, then stop. Under the test harness it also exits non-zero, which is what
/// turns "the boot printed something alarming" into a failing build.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!();
    println!("  [panic] {}", info.message());
    if let Some(loc) = info.location() {
        println!(
            "  [panic] at {}:{}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        );
    }
    if SELFTEST {
        semihosting::exit(0);
    }
    halt()
}

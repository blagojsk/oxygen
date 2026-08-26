//! Oxygen — the Swiss army knife for getting things done in an agentic era.
//!
//! An operating system for agents and humans in equal measure, built to make weak and old hardware
//! useful again.
//!
//! Milestone 2 in progress: the machine can be interrupted, and translation is on. Virtual memory
//! is the prerequisite for isolating anything from anything else, and therefore for processes,
//! containers and agents that cannot reach past their own boundaries.

#![no_std]
#![no_main]

mod arch;

use arch::target::{self, exceptions, gic, mmu, semihosting, timer};

/// Set by the harness when the kernel is booted as a test rather than for a human to watch.
const SELFTEST: bool = option_env!("OXYGEN_SELFTEST").is_some();

/// Entry point, called from the boot assembly once a stack exists and `.bss` is zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    println!();
    println!("  oxygen {}", env!("CARGO_PKG_VERSION"));
    println!("  the swiss army knife for getting things done in an agentic era");
    println!();
    println!("  [boot] aarch64 · qemu virt · EL{}", target::current_el());
    println!("  [boot] stack installed, .bss zeroed, rust reached");

    // Order matters and is not stylistic: vectors must be installed before the GIC can deliver
    // anything, the GIC must be up before the timer has a line to raise, and IRQs stay masked
    // until all three are ready.
    // SAFETY: called once, on the boot core, in the order each step documents as required.
    unsafe {
        exceptions::init();
        println!("  [trap] vector table installed");
        // Before the GIC, because its registers are reached through the device mapping this
        // installs — and because turning translation on is least dangerous while the system is
        // still quiet.
        mmu::init();
        gic::init();
        timer::init();
        target::enable_irqs();
    }
    println!("  [boot] interrupts enabled");
    println!();

    if SELFTEST {
        selftest()
    }

    println!("  idle. interrupts are live — the timer is ticking.");
    target::wait_for_interrupt()
}

/// Proves the timer is actually delivering interrupts.
///
/// Waiting for a tick count to *rise* is the assertion that matters: a kernel that merely fails to
/// crash proves nothing, and this is the difference between "interrupts are configured" and
/// "interrupts arrive". The bound is generous because it exists to fail a dead controller, not to
/// measure timing.
fn selftest() -> ! {
    const REQUIRED: u64 = 5;
    const PATIENCE: u64 = 200_000_000;

    if !mmu::is_enabled() {
        println!("  [selftest] MMU reported disabled after init — FAILED");
        semihosting::exit(1);
    }
    println!("  [selftest] translation is on and we are still executing — ok");

    let mut spins = 0u64;
    while gic::ticks() < REQUIRED {
        core::hint::spin_loop();
        spins += 1;
        if spins > PATIENCE {
            println!("  [selftest] no timer interrupts after {spins} spins — FAILED");
            semihosting::exit(1);
        }
    }
    println!(
        "  [selftest] {} timer interrupts delivered — ok",
        gic::ticks()
    );

    // Last, because it ends the run: prove W^X is enforced rather than merely configured.
    // Reading the descriptors back would only confirm we wrote what we meant to; the hardware
    // refusing the write is the actual guarantee. The fault handler recognises this one and exits
    // 0, so reaching the line after the write means protection is NOT in effect.
    exceptions::expect_write_fault();
    let text = mmu::text_address() as *mut u8;
    // SAFETY: deliberately illegal. The MMU is expected to refuse this, and the selftest fails
    // loudly below if it does not.
    unsafe { core::ptr::write_volatile(text, 0xFF) };

    println!("  [selftest] write to .text SUCCEEDED — W^X is NOT enforced — FAILED");
    semihosting::exit(1)
}

/// Where every unrecoverable error ends up.
///
/// A kernel panic has no supervisor to report to, so the contract is: say everything we know on the
/// console, then stop. Under the test harness it also exits non-zero, which is what turns "the boot
/// printed something alarming" into a failing build.
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
        semihosting::exit(1);
    }
    loop {
        core::hint::spin_loop();
    }
}

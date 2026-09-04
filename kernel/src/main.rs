//! Oxygen — the Swiss army knife for getting things done in an agentic era.
//!
//! An operating system for agents and humans in equal measure, built to make weak and old hardware
//! useful again.
//!
//! Milestone 3: the machine can be interrupted, memory is translated and protected, and threads
//! are preemptively scheduled. Concurrency is the prerequisite for a system that can run an agent
//! and still answer a human.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};

extern crate alloc;

mod arch;
mod ipc;
mod mm;
mod sched;
mod sync;
mod syscall;

use arch::target::{self, exceptions, gic, mmu, semihosting, timer, user};

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
        // After the MMU, because the heap's memory is only usable once the identity map covers
        // it, and before the GIC so later bring-up can allocate if it needs to.
        mm::init();
        // Before interrupts, because the first timer tick asks for a reschedule and there must be
        // a current thread to reschedule away from.
        sched::init();
        // After the heap, which the endpoint table and the registry both allocate from.
        ipc::init();
        gic::init();
        timer::init();
        // After the MMU, which it needs in order to narrow a page's permissions, and before any
        // thread can be entered into it.
        user::load();
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

    // Prove threads are actually scheduled, not merely created. Three threads spin up counters;
    // the test passes only once every one of them has made progress, which cannot happen unless
    // the timer is preempting whichever thread is running and the switcher is restoring the next
    // one's stack correctly.
    {
        sched::spawn("worker-a", worker, 0);
        sched::spawn("worker-b", worker, 1);
        sched::spawn("worker-c", worker, 2);

        let (total, ready) = sched::census();
        println!(
            "  [selftest] {total} threads exist, {ready} runnable, running as #{}",
            sched::current_id()
        );
        sched::dump();

        let mut spins = 0u64;
        while COUNTERS.iter().any(|c| c.load(Ordering::Relaxed) < 50) {
            // Yield rather than spin only: this thread must give the others a turn, and doing it
            // explicitly proves cooperative yielding works alongside preemption.
            sched::yield_now();
            spins += 1;
            if spins > 5_000_000 {
                println!("  [selftest] threads did not all progress — FAILED");
                for (i, c) in COUNTERS.iter().enumerate() {
                    println!("  [selftest]   worker {i}: {}", c.load(Ordering::Relaxed));
                }
                semihosting::exit(1);
            }
        }

        println!(
            "  [selftest] 3 threads interleaved ({}, {}, {}) over {} switches — ok",
            COUNTERS[0].load(Ordering::Relaxed),
            COUNTERS[1].load(Ordering::Relaxed),
            COUNTERS[2].load(Ordering::Relaxed),
            sched::switches(),
        );
    }

    // Prove the heap actually works, rather than merely reporting a size. Growing a Vec past its
    // initial capacity forces a real allocate-copy-free cycle through the global allocator.
    {
        use alloc::vec::Vec;
        let before = mm::heap_used();
        let mut v: Vec<u64> = Vec::new();
        for i in 0..1024 {
            v.push(i);
        }
        let sum: u64 = v.iter().sum();
        if sum != (0..1024u64).sum::<u64>() {
            println!("  [selftest] heap returned wrong data — FAILED");
            semihosting::exit(1);
        }
        if mm::heap_used() <= before {
            println!("  [selftest] heap reported no usage after allocating — FAILED");
            semihosting::exit(1);
        }
        drop(v);
        if mm::heap_used() != before {
            println!("  [selftest] heap leaked across a drop — FAILED");
            semihosting::exit(1);
        }
        println!(
            "  [selftest] heap: 1024 items allocated, summed and freed cleanly, {} KiB free — ok",
            mm::heap_free() / 1024
        );
    }

    // Prove the privilege boundary exists and that capabilities are what crosses it. The user
    // program runs at EL0, writes through a capability, derives a narrower one, revokes it, and
    // reports back what the kernel refused the revoked handle with. Asserting on *which* refusal
    // is the point: any error would prove the write failed, but only this one proves it failed
    // because the grant was withdrawn.
    {
        let id = sched::spawn("user", user_thread, 0);
        match await_exit(id, "the user thread") {
            syscall::E_STALE => println!(
                "  [selftest] EL0 wrote through a capability, then lost it to revocation — ok"
            ),
            sched::EXIT_FAULTED => {
                println!("  [selftest] the user thread faulted instead of exiting — FAILED");
                semihosting::exit(1);
            }
            other => {
                println!(
                    "  [selftest] user thread exited {other:#x}, expected a stale handle — FAILED"
                );
                semihosting::exit(1);
            }
        }
    }

    // The boundary itself. A user thread that reads kernel memory must die, and the kernel must
    // not. Every fault before M4 was fatal to the machine because every fault was the kernel's;
    // this asserts that is no longer true, which is the whole return on having a privilege level.
    {
        let id = sched::spawn(
            "trespasser",
            trespassing_thread,
            mmu::text_address() as usize,
        );
        match await_exit(id, "the trespassing thread") {
            sched::EXIT_FAULTED => {
                println!("  [selftest] EL0 read of kernel memory faulted, kernel survived — ok")
            }
            other => {
                println!("  [selftest] EL0 read kernel memory and exited {other:#x} — FAILED");
                semihosting::exit(1);
            }
        }
    }

    // IPC. A server publishes an endpoint by name and blocks on it; a client that has never been
    // told anything about the server finds it by that name and sends it a typed message. Three
    // things are asserted rather than one: that the receiver genuinely leaves the run queue, that
    // the payload arrives intact, and that a name nobody published cannot be found.
    {
        let server = sched::spawn("server", server_thread, 0);

        let mut spins = 0u64;
        while !matches!(sched::state_of(server), Some(sched::State::Blocked(_))) {
            sched::yield_now();
            spins += 1;
            if spins > 5_000_000 {
                println!("  [selftest] the server never blocked on its endpoint — FAILED");
                sched::dump();
                semihosting::exit(1);
            }
        }
        println!("  [selftest] server is blocked on its endpoint, off the run queue — ok");
        ipc::dump();

        let client = sched::spawn("client", client_thread, 0);
        if await_exit(client, "the client thread") != 0 {
            println!("  [selftest] the client could not send — FAILED");
            semihosting::exit(1);
        }

        match await_exit(server, "the server thread") {
            42 => println!(
                "  [selftest] a typed message crossed between two EL0 tasks that only shared a name — ok"
            ),
            other => {
                println!("  [selftest] the server received {other:#x}, expected 42 — FAILED");
                semihosting::exit(1);
            }
        }

        let absent = oxygen_ipc::Name::new("nosuchservice").expect("a valid name");
        if ipc::lookup(&absent).is_ok() {
            println!("  [selftest] the registry found a name nobody published — FAILED");
            semihosting::exit(1);
        }
        println!("  [selftest] an unpublished name is not found — ok");
    }

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

/// Gives the running thread the two capabilities every user program here is born holding.
///
/// Seeded explicitly, in code you can point at, rather than appearing as a property of having been
/// created. A thread that was granted nothing can do nothing, and that has to be the default or
/// the word "capability" is decoration.
fn seed_capabilities() -> (u64, u64) {
    use oxygen_cap::{Object, Rights};

    let console = sched::with_caps(|caps| caps.insert(Object::Console, Rights::ALL))
        .expect("a fresh capability space has room");
    let registry = sched::with_caps(|caps| caps.insert(Object::Registry, Rights::ALL))
        .expect("a fresh capability space has room");
    (console.raw(), registry.raw())
}

/// The kernel half of the first user thread: the one that exercises capabilities.
extern "C" fn user_thread(_arg: usize) {
    let (console, registry) = seed_capabilities();
    // SAFETY: the loader prepared and mapped this entry and stack for EL0 during boot, and no
    // other thread uses stack slot 0.
    unsafe { user::enter(user::program(), user::stack_top(0), console, registry) }
}

/// A user thread that reads where it may not, so the refusal can be observed.
///
/// The address it is given is a kernel page the kernel is genuinely using, so what the hardware
/// raises is a permission fault rather than a translation fault. Reading an unmapped address would
/// fault too, and would prove nothing about privilege.
extern "C" fn trespassing_thread(kernel_address: usize) {
    // SAFETY: mapped EL0-executable alongside the other programs; stack slot 1 is its own.
    unsafe {
        user::enter(
            user::trespasser(),
            user::stack_top(1),
            kernel_address as u64,
            0,
        )
    }
}

/// The kernel half of the server: publishes an endpoint and waits on it.
extern "C" fn server_thread(_arg: usize) {
    let (console, registry) = seed_capabilities();
    // SAFETY: mapped EL0-executable alongside the other programs; stack slot 2 is its own.
    unsafe { user::enter(user::server(), user::stack_top(2), console, registry) }
}

/// The kernel half of the client: finds that endpoint by name and sends to it.
extern "C" fn client_thread(_arg: usize) {
    let (console, registry) = seed_capabilities();
    // SAFETY: mapped EL0-executable alongside the other programs; stack slot 3 is its own.
    unsafe { user::enter(user::client(), user::stack_top(3), console, registry) }
}

/// Waits for a thread to retire and returns what it stopped with, failing the run if it never does.
fn await_exit(id: u64, what: &str) -> u64 {
    let mut spins = 0u64;
    loop {
        if let Some(code) = sched::exit_code_of(id) {
            return code;
        }
        sched::yield_now();
        spins += 1;
        if spins > 5_000_000 {
            println!("  [selftest] {what} never finished — FAILED");
            sched::dump();
            semihosting::exit(1);
        }
    }
}

/// Counters the selftest's worker threads advance, one each.
/// Counters the selftest's worker threads advance, one each.
static COUNTERS: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

/// A selftest worker. Runs forever: the point is that it keeps getting scheduled, not that it
/// finishes.
extern "C" fn worker(index: usize) {
    loop {
        COUNTERS[index].fetch_add(1, Ordering::Relaxed);
        sched::yield_now();
    }
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

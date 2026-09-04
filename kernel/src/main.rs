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
mod audit;
mod ipc;
mod mm;
mod sched;
mod schema;
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
        // Needs nothing of its own — a `SchemaTable` is a fixed-size array — but is ordered here
        // for clarity: it describes the same object kinds IPC's endpoint and registry types
        // name, so building the description after what it describes exists keeps the boot order
        // legible even though nothing here actually reads IPC state.
        schema::init();
        gic::init();
        timer::init();
        // After the GIC, which owns the line it enables.
        arch::target::uart::init_rx();
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

    // A service to find, and something to find it with. The boot thread has nothing left to do
    // but exist: it parks, and the timer preempts it into the shell.
    sched::spawn("server", server_thread, 0);
    sched::spawn("shell", shell_thread, 0);
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
    //
    // Its id is kept past this block's end: the M7 checks below assert that the delegate and the
    // revoke it just performed both landed in the audit journal attributed to this same thread.
    let user_thread_id;
    {
        let id = sched::spawn("user", user_thread, 0);
        user_thread_id = id;
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
    //
    // The client's id is kept past this block's end: the M7 checks below assert that its refused
    // attempt to re-register the looked-up endpoint landed in the audit journal, attributed to it.
    let client_thread_id;
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
        client_thread_id = client;
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

    // M7: the agent surface. Every grant, delegation, lookup, registration and refusal above was
    // journaled as it happened; this is where that record is actually read back and checked,
    // rather than merely trusted to have worked. Three things: the sequence is what it claims to
    // be, a specific delegate-then-revoke landed against the thread that did it, and a specific
    // refusal — not just any refusal — is on the record too. Then the schema: that a capability's
    // own methods can be listed, and that the listing survives the same wire encoding
    // `SYS_DESCRIBE` hands to a caller.
    {
        audit::dump();
        println!(
            "  [selftest] journal: {} events dropped over its lifetime so far",
            audit::dropped()
        );

        let len = audit::len();
        let mut previous_seq = 0u64;
        for i in 0..len {
            let Some(event) = audit::event_at(i) else {
                println!("  [selftest] journal: event {i} missing out of {len} retained — FAILED");
                semihosting::exit(1);
            };
            if event.seq <= previous_seq {
                println!(
                    "  [selftest] journal: seq {} did not increase past {previous_seq} — FAILED",
                    event.seq
                );
                semihosting::exit(1);
            }
            previous_seq = event.seq;
        }
        println!(
            "  [selftest] journal: {len} events retained, sequence numbers strictly increasing — ok"
        );

        // The user thread delegated a capability and then had it revoked — both must be on the
        // record, attributed to it, delegate before revoke.
        let mut last_delegate_seq: Option<u64> = None;
        let mut delegate_then_revoke = false;
        for i in 0..len {
            let event = audit::event_at(i).expect("index bounded by len, checked above");
            if event.actor != user_thread_id {
                continue;
            }
            match event.action {
                oxygen_audit::Action::Delegate => last_delegate_seq = Some(event.seq),
                oxygen_audit::Action::Revoke
                    if last_delegate_seq.is_some_and(|seq| seq < event.seq) =>
                {
                    delegate_then_revoke = true;
                }
                _ => {}
            }
        }
        if !delegate_then_revoke {
            println!(
                "  [selftest] journal: no delegate-then-revoke recorded for the user thread — FAILED"
            );
            semihosting::exit(1);
        }
        println!(
            "  [selftest] journal: user thread delegated then revoked, both recorded with the right actor — ok"
        );

        // The client looked "echo" up — READ|WRITE, no GRANT — and then tried to register that
        // same handle under a new name. The refusal has to be on the record, attributed to it.
        let denied = (0..len)
            .map(|i| audit::event_at(i).expect("index bounded by len, checked above"))
            .any(|event| {
                matches!(event.action, oxygen_audit::Action::Denied)
                    && event.detail == syscall::SYS_REGISTER
                    && event.actor == client_thread_id
            });
        if !denied {
            println!(
                "  [selftest] journal: no denied SYS_REGISTER recorded for the client thread — FAILED"
            );
            semihosting::exit(1);
        }
        println!(
            "  [selftest] journal: a lookup-obtained handle could not re-register the name, and the refusal was recorded — ok"
        );

        // The schema: the console's first method is `write`, and its encoding round-trips.
        let Some(method) = schema::method_at(1, 0) else {
            println!("  [selftest] schema: the console has no method at index 0 — FAILED");
            semihosting::exit(1);
        };
        if method.name().as_str() != "write" {
            println!(
                "  [selftest] schema: console method 0 is {:?}, expected write — FAILED",
                method.name().as_str()
            );
            semihosting::exit(1);
        }
        let mut buf = [0u8; oxygen_schema::ENCODED_METHOD_BYTES];
        method
            .encode(&mut buf)
            .expect("buf is exactly ENCODED_METHOD_BYTES wide");
        match oxygen_schema::Method::decode(&buf) {
            Ok(decoded) if decoded == method => {}
            _ => {
                println!("  [selftest] schema: encode/decode did not round-trip — FAILED");
                semihosting::exit(1);
            }
        }
        println!(
            "  [selftest] schema: the console describes itself, and the description survives the wire — ok"
        );
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

/// Gives the running thread the three capabilities every user program here is born holding.
///
/// Seeded explicitly, in code you can point at, rather than appearing as a property of having been
/// created. A thread that was granted nothing can do nothing, and that has to be the default or
/// the word "capability" is decoration.
///
/// The journal capability carries `Rights::READ` only. Nothing but the kernel itself ever calls
/// `audit::record` — no syscall hands a user thread a way to write an event directly — so no
/// holder needs, or should be able to claim, `WRITE` on it; `GRANT`/`REVOKE` are withheld the same
/// way every other seeded capability withholds them; a thread wanting to share read access to the
/// record narrows this down further with `SYS_DELEGATE`, never widens it.
fn seed_capabilities() -> (u64, u64, u64) {
    use oxygen_cap::{Object, Rights};

    let console = sched::with_caps(|caps| caps.insert(Object::Console, Rights::ALL))
        .expect("a fresh capability space has room");
    let registry = sched::with_caps(|caps| caps.insert(Object::Registry, Rights::ALL))
        .expect("a fresh capability space has room");
    let journal = sched::with_caps(|caps| caps.insert(Object::Journal, Rights::READ))
        .expect("a fresh capability space has room");
    (console.raw(), registry.raw(), journal.raw())
}

/// The kernel half of the first user thread: the one that exercises capabilities.
extern "C" fn user_thread(_arg: usize) {
    let (console, registry, journal) = seed_capabilities();
    // SAFETY: the loader prepared and mapped this entry and stack for EL0 during boot, and no
    // other thread uses stack slot 0.
    unsafe {
        user::enter(
            user::program(),
            user::stack_top(0),
            console,
            registry,
            journal,
        )
    }
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
            0,
        )
    }
}

/// The kernel half of the server: publishes an endpoint and waits on it.
extern "C" fn server_thread(_arg: usize) {
    let (console, registry, journal) = seed_capabilities();
    // SAFETY: mapped EL0-executable alongside the other programs; stack slot 2 is its own.
    unsafe {
        user::enter(
            user::server(),
            user::stack_top(2),
            console,
            registry,
            journal,
        )
    }
}

/// The kernel half of the client: finds that endpoint by name and sends to it.
extern "C" fn client_thread(_arg: usize) {
    let (console, registry, journal) = seed_capabilities();
    // SAFETY: mapped EL0-executable alongside the other programs; stack slot 3 is its own.
    unsafe {
        user::enter(
            user::client(),
            user::stack_top(3),
            console,
            registry,
            journal,
        )
    }
}

/// The kernel half of the shell.
extern "C" fn shell_thread(_arg: usize) {
    let (console, registry, journal) = seed_capabilities();
    let entry = arch::target::shell::main as *const () as u64;
    // SAFETY: `shell::main` is linked into `.user_text`, which `mmu::init` mapped EL0-executable,
    // and stack slot 0 belongs to this thread — the selftest programs that use it do not run here.
    unsafe { user::enter(entry, user::stack_top(0), console, registry, journal) }
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

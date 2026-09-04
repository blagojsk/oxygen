The record of what Oxygen is. CLAUDE.md holds the rules; nothing describing the system belongs there.

**When a FUTURE item ships, the same commit moves it to PRESENT.**

# PRESENT

## What Oxygen is

An operating system written from scratch in Rust, on the premise that its users are **agents and people in
equal measure**. Not a Linux distribution, not a wrapper around an existing kernel, not an assistant bolted
onto a conventional OS.

It is aimed at everyday office work, education and software engineering — a general-purpose system rather
than a demonstration. Its positioning line is *the Swiss army knife for getting things done in an agentic
era*.

Two constraints shape every decision and pull against each other on purpose:

- **It must run well on the weakest hardware imaginable**, to make old and cheap machines competitive while
  hardware stays expensive. Memory is the scarce resource, then storage, then CPU.
- **It is agent-native from the first commit.** Agents are first-class users, able to run, be interacted
  with, and eventually be built in.

Agents are memory-hungry and the target machines are not. Inference is therefore expected to be
remote-first with a local path where hardware allows, and the agent interface matters more than any bundled
model.

Two capabilities follow from the same design rather than being added to it:

- **Agents and people can both spin off containers.** On a monolithic kernel a container is a process with a
  retrofitted restricted view; here a process only ever holds the authority it was handed, so a container is
  a process given a curated capability set.
- **A task factory is a system service, not an app.** Work — a refactor, a spreadsheet, a lesson — can be
  given to an agent and carried to completion under supervision. A human approval **is** a capability grant,
  and the audit journal **is** what project tracking reads.

## The design thesis

Conventional systems expose their capabilities twice and badly: as text streams shaped for humans, which
agents must scrape and guess at, and as rigid C ABIs, which nothing can discover at runtime. Oxygen treats a
capability as one typed, self-describing thing and renders it for whoever is asking.

Five invariants follow:

| Invariant | Meaning |
| --- | --- |
| **Discoverable** | The system can enumerate what it can do, with schemas. Nothing requires reading prose to call correctly |
| **Structured** | Calls and results are typed values, not byte streams awaiting a parser |
| **Capability-secured** | Authority travels as unforgeable handles, never as ambient permission |
| **Auditable** | Effectful operations are journaled and attributable |
| **One surface, two audiences** | Human and agent interfaces are renderings of one contract, never two implementations |

## Repository layout

A Cargo workspace. Everything freestanding.

| Path | Holds |
| --- | --- |
| `kernel/` | the kernel binary: boot, exceptions, memory, scheduling |
| `kernel/src/arch/aarch64/` | everything that knows about a specific CPU or interrupt controller |
| `kernel/linker.ld` | memory layout; `build.rs` passes it to the linker |
| `kernel/src/syscall.rs` | the system call surface — everything EL0 can ask for |
| `kernel/src/ipc.rs` | endpoints, blocking, and the name registry |
| `kernel/src/arch/aarch64/shell.rs` | the shell — Rust compiled into the EL0 section |
| `kernel/src/arch/aarch64/user.rs` | entering EL0, and the test programs that run there |
| `crates/oxygen-mem/` | portable memory logic: addresses, frame allocator, heap |
| `crates/oxygen-aarch64/` | AArch64 encodings that are pure arithmetic |
| `crates/oxygen-cap/` | portable capability logic: handles, rights, derivation, revocation |
| `crates/oxygen-ipc/` | portable message, queue and registry logic |
| `scripts/` | developer and CI entry points |
| `scripts/check-userspace.sh` | asserts no EL0 code branches into the kernel |
| `.cargo/config.toml` | default target and the QEMU runner |
| `rust-toolchain.toml` | pinned compiler, components and target |

## Target and toolchain

*Kernel-shaped substitute for the "Profiles and environment" parity section: a kernel has a target and a
board where an application has profiles.*

| | |
| --- | --- |
| Architecture | AArch64, primary. Chosen because cheap ARM boards are the first hardware target |
| Target triple | `aarch64-unknown-none-softfloat` — floating point off, so the kernel cannot touch FP/SIMD registers before it saves them on a context switch |
| Reference machine | QEMU `virt`, `-cpu cortex-a72`, 1 core, 256 MiB |
| Entry exception level | EL1 |
| Compiler | Rust nightly, pinned in `rust-toolchain.toml`, with `rust-src` and `llvm-tools-preview` |
| Install | rustup from rust-lang.org; shims at `~/.cargo/bin`, on PATH via `~/.cargo/env` |
| Emulator | QEMU, `qemu-system-aarch64` |

Environment variables read by the build or the kernel, verified against the source:

| Name | Read by | Effect |
| --- | --- | --- |
| `OXYGEN_SELFTEST` | `kernel/src/main.rs` | Kernel runs its assertions and exits via semihosting instead of idling |
| `CARGO_PKG_VERSION` | `kernel/src/main.rs` | Version printed in the boot banner |
| `PROFILE` | `scripts/boot-test.sh` | `debug` or `release`; selects which build the boot test runs |

## Boot sequence

*Kernel-shaped substitute for the "Environments" parity section: a kernel has one environment, entered in a
fixed order, and the order is the load-bearing fact.*

| Step | Does | Lives in |
| --- | --- | --- |
| 1 | Parks secondary cores, installs the stack, zeroes `.bss` | `arch/aarch64/boot.rs` |
| 2 | Installs the exception vector table in `VBAR_EL1` | `arch/aarch64/exceptions.rs` |
| 3 | Builds the identity map and enables the MMU, data cache and instruction cache | `arch/aarch64/mmu.rs` |
| 4 | Describes physical memory, reserves the kernel image, carves the heap | `mm.rs` |
| 5 | Adopts the running code as thread 0 | `sched.rs` |
| 6 | Brings up the GIC distributor and CPU interface | `arch/aarch64/gic.rs` |
| 7 | Starts the generic timer | `arch/aarch64/timer.rs` |
| 8 | Creates the endpoint table and the name registry | `ipc.rs` |
| 9 | Enables UART receive interrupts | `arch/aarch64/uart.rs` |
| 10 | Gives the user stacks their EL0 permissions | `arch/aarch64/user.rs` |
| 11 | Unmasks IRQs | `arch/aarch64/mod.rs` |

Order is load-bearing: vectors before any interrupt can be delivered, the MMU before the GIC whose registers
are reached through the device mapping, the scheduler before the first timer tick asks for a reschedule.

## Memory map

*Kernel-shaped substitute for the "changelogs in apply order" parity section: an ordered, authoritative list
of how address space is laid out. `kernel/linker.ld` and `kernel/src/arch/aarch64/mmu.rs` are the authority.*

Translation: 4 KiB granule, 39-bit virtual address space, three levels, TTBR1 disabled.

| Range | Mapping | Attributes |
| --- | --- | --- |
| `0x0000_0000`–`0x4000_0000` | one L1 block, 1 GiB | Device-nGnRnE, read/write, never executable |
| `0x0900_0000` | PL011 UART, within the device block | |
| `0x0800_0000` / `0x0801_0000` | GICv2 distributor / CPU interface, within the device block | |
| `0x4000_0000`–`0x4020_0000` | L3 table, 4 KiB pages | per section, below |
| `0x4020_0000`–`0x8000_0000` | L2 blocks, 2 MiB each | Normal, read/write, never executable |

Within the kernel image, one page at a time:

| Section | Attributes |
| --- | --- |
| `.text` | read-only, executable at EL1 |
| `.rodata` | read-only, never executable |
| `.data`, `.bss`, page tables, boot stack | read/write, never executable |
| user text page | read-only at EL0 and EL1, executable at EL0 only |
| user stack page | read/write at EL0 and EL1, never executable |
| remainder of the first 2 MiB | read/write, never executable |

Fixed sizes: boot stack 64 KiB, per-thread kernel stack 16 KiB, kernel heap 2 MiB (512 frames), frame
allocator bitmap sized for the assumed RAM extent at one bit per 4 KiB frame.

## Memory management

Two allocators, stacked. The **frame allocator** owns physical memory in 4 KiB units and is a bitmap: one bit
per frame is 32 KiB of metadata per GiB, so a 64 MiB machine spends two kilobytes tracking its memory. Frames
start reserved, so memory must be declared usable before it is handed out. Exhaustion and fragmentation are
distinct errors.

The **kernel heap** sits on frames handed to it and is a free list whose blocks are sorted by address. Block
headers live inside free blocks, so a fully-allocated heap has zero overhead; the address ordering is what
permits coalescing.

RAM extent is assumed at 128 MiB rather than discovered, and deliberately assumed below the 256 MiB the run
scripts provide.

## Capabilities and user mode

A thread below EL1 holds no ambient authority. It cannot reach the console, or anything else, except by
presenting a handle the kernel checks — which is what makes "what is this agent allowed to do?" a question
with a readable answer rather than an audit of every code path it might take.

| | |
| --- | --- |
| Handle | `(index, generation)` packed into a `u64`. The generation is what makes a withdrawn handle fail rather than silently naming whatever reused its slot |
| Rights | `READ`, `WRITE`, `GRANT`, `REVOKE`. Never widen on delegation — a derived capability gets the intersection of what was asked for and what the parent held |
| Objects | `Console`, `Task`, `Memory`, and `Null` for a free slot |
| Space | Fixed at 16 slots per thread. Growable spaces let one task consume the kernel's memory |
| Derivation | Each slot carries `parent`, `first_child`, `next_sibling` as `u32` indices — 16 bytes, and the table stays relocatable |

**Revocation is supported, and that decision is why the derivation tree exists.** An approval *is* a
capability grant, so a capability that cannot be withdrawn is an approval that cannot be withdrawn, which is
not an approval. Generation counters alone would have been cheaper but revoke for every holder at once; the
case that matters is withdrawing what was given to one agent, which is per-edge and needs the tree. The cost
is 16 bytes per delegated capability — 4,096 live delegations is 64 KiB, against a target machine measured
in tens of megabytes.

`revoke` frees every descendant of a slot and leaves the slot itself intact: you withdraw what you gave, not
what you hold. `delete` frees the slot and its subtree together, so no descendant outlives the authority it
came from. Both walk the tree iteratively — recursion on a 16 KiB kernel stack is a depth limit nobody
declared.

### System calls

Number in `x8`, arguments in `x0`–`x5`, result in `x0`, following the AArch64 Linux convention. Errors come
back as small negative values, distinct per cause: the difference between "you never had this" and "you had
it and it was withdrawn" is what a caller needs in order to react.

| Call | Signature |
| --- | --- |
| `SYS_WRITE` | `(handle, ptr, len) -> bytes written` — requires a `Console` and `WRITE` |
| `SYS_IDENTIFY` | `(handle) -> kind in the low byte, rights above it` |
| `SYS_DELEGATE` | `(handle, rights) -> handle` — requires `GRANT` |
| `SYS_REVOKE` | `(handle) -> how many were withdrawn` — requires `REVOKE` |
| `SYS_EXIT` | `(code)`, does not return |
| `SYS_ENDPOINT` | `() -> handle` — a new endpoint, with every right, because the caller made it |
| `SYS_SEND` | `(handle, interface, method, ptr, len) -> 0` — requires `WRITE` |
| `SYS_RECV` | `(handle, ptr, cap) -> bytes written` — requires `READ`; blocks |
| `SYS_LOOKUP` | `(registry, name_ptr, name_len) -> handle` — requires `READ` on the registry |
| `SYS_REGISTER` | `(registry, name_ptr, name_len, endpoint) -> 0` — requires `WRITE` on the registry and `GRANT` on the endpoint |
| `SYS_READ` | `(handle, ptr, cap) -> bytes read` — requires `READ` on a console; blocks |
| `SYS_SERVICES` | `(registry, index, ptr, cap) -> len` — the name published at an index |
| `SYS_UPTIME` | `() -> timer ticks since boot` |

Every pointer argument is checked against the calling thread's own pages before the kernel dereferences it.
Without that check a handle with entirely legitimate rights becomes a way to print kernel memory.

## IPC and the registry

Two tasks that cannot name the same thing cannot talk. A capability cannot yet cross a task
boundary, so the only thing two tasks can both name is a *name* — which is what the registry is
for, and why it arrived in the same milestone as messaging rather than later.

| | |
| --- | --- |
| Message | `interface: u32`, `method: u32`, `len: u32`, then up to 64 bytes inline |
| Typed | Interface `0` is reserved and rejected, so "you forgot to say what this is" is a caught error rather than a message that silently means nothing |
| Queue | 8 messages per endpoint, a real ring buffer. Shallow on purpose: a deep queue turns a receiver that stopped keeping up into memory the kernel holds on its behalf, and tells the sender far too late |
| Endpoint ids | Start at 1, so a zeroed field never names one |
| Registry | 16 names, each 1–16 bytes of printable ASCII, enumerable |

A payload is fixed and inline rather than a pointer to be copied later: a message that never
allocates and never outlives the call that sent it is the only shape that stays affordable on a
machine measured in tens of megabytes.

Names are restricted to printable ASCII because this surface is read by both audiences. A name that
can hold a control byte or a space is one that cannot be printed back to a human unambiguously.

**Blocking.** A receiver with an empty queue leaves the run queue entirely; a sender wakes everyone
parked on that endpoint and lets them race for it. Waking one — picking a winner — would be a
scheduling policy hidden inside a message queue; the loser finds the queue empty and parks again,
which is correct with nothing coordinating it. `send` releases the endpoint lock before taking the
scheduler's, so no path holds both.

Discovery is itself an authority: looking a name up requires a capability on the registry. A
capability handed back by `SYS_LOOKUP` carries `READ | WRITE` but never `GRANT` — finding a service
lets you talk to it, not advertise it as yours.

## Userspace and the shell

Programs run at EL0 and are linked into `.user`, a section of the kernel image that is the only one
EL0 may fetch from — read-only at both levels, executable at EL0, never executable at EL1. They run
at the address they were linked at, because the whole space is identity-mapped.

Being in the kernel's image does not make them part of the kernel, and the boundary is enforced two
ways. At run time the MMU refuses EL0 any access to kernel pages. At build time
`scripts/check-userspace.sh` disassembles `.user` and fails if any branch leaves it.

That build-time check is not belt-and-braces, it is load-bearing. Ordinary Rust reaches into `core`
constantly and a debug build reaches further: `Range::contains` is a call, `slice::get_unchecked` is
a call, `let buf = [0u8; 128]` can be a call to `memset`, and every `+=` carries an overflow check
that calls `core::panicking`. All of those live in kernel `.text`, and from EL0 each one is an
instruction abort on whichever line happens to run first. So EL0 code here uses raw pointer reads,
`MaybeUninit` instead of zeroed arrays, and `wrapping_*` arithmetic — and the check enforces it
rather than review.

**Console input** is interrupt-driven: the PL011 raises SPI 1 (INTID 33) on receive or receive
timeout, the handler drains the FIFO into a 256-byte ring and wakes anyone parked on the console
wait channel. The timeout interrupt matters as much as the receive one — without it a few
characters that never fill the FIFO would sit there until somebody typed enough more. A full ring
says so once and then drops quietly; input that vanishes without a word is a keyboard that looks
broken.

Echo is the shell's job, not the kernel's. The kernel has no idea whether whatever is on the other
end wants to see what it typed, and an agent driving this over a pipe does not.

| Command | Does |
| --- | --- |
| `help` | lists these |
| `services` | enumerates the registry — what is published, discovered rather than known in advance |
| `uptime` | timer ticks since boot |
| `echo ...` | hands the rest of the line back |

## Scheduling

Round-robin, preemptive. The timer marks the running thread for replacement; the switch happens on the way
out of the IRQ vector, after the interrupt is acknowledged and ended.

The context switch saves only the callee-saved registers — the procedure call standard permits a function to
destroy `x0`–`x18`, and a preempted thread's full register set is in the trap frame on its own kernel stack.
`yield_now` releases the scheduler lock before switching.

`SpinLock` spins rather than blocking, which holds only while the kernel is single-core and critical sections
are a handful of instructions.

## Dependencies

*Kernel-shaped substitute for the "Libraries" parity section.*

**No external crates.** All three manifests have empty dependency lists, which is the point of a freestanding
kernel: every line that runs is in this repository or in `core`.

| Crate | Consumed by | Holds |
| --- | --- | --- |
| `oxygen-mem` | `oxygen-kernel` | Address types, frame allocator, heap |
| `oxygen-aarch64` | `oxygen-kernel` | Translation-table descriptors, MAIR and TCR encoding |

Both are host-testable by construction: they contain arithmetic only, so their failure modes are red tests
rather than a dead machine.

## Verification

*Kernel-shaped substitute for the "CI, versioning, deploys" parity section: there is no deploy target yet.*

| Script | Proves |
| --- | --- |
| `scripts/run.sh` | Builds and boots; `--accel` runs on the host CPU via Apple's hypervisor |
| `scripts/check.sh` | The full gate: fmt, clippy on both targets, host tests, boot |
| `scripts/test.sh` | Host unit tests for the portable crates |
| `scripts/boot-test.sh` | The kernel boots and reaches its assertions; watchdog fails a hang |
| `scripts/_toolchain.sh` | Sourced by the others; locates cargo and QEMU |

No CI, no versioning scheme and no deploy target exist. Version is `0.0.1` in the workspace manifest.

## Test tiers

| Tier | Where | Runs on | Count |
| --- | --- | --- | --- |
| Host unit | `crates/*/src`, under `#[cfg(test)]` | host triple | 39 |
| Boot assertions | `kernel/src/main.rs`, gated by `OXYGEN_SELFTEST` | QEMU, TCG | 5 |

Boot assertions each assert an observable behaviour rather than the absence of a crash: translation is on and
execution continues, the timer tick count rises, three threads interleave, the heap allocates and returns to
its prior usage, and a write to `.text` is refused by the hardware.

## Milestones

| | Milestone | State |
| --- | --- | --- |
| M0 | Boots, reaches Rust, serial output, panic handling | done |
| M1 | Exception vectors, GICv2, timer, physical frame allocator | done |
| M2 | MMU, per-section mapping with W^X, kernel heap | done |
| M3 | Threads, context switch, preemptive scheduler | done |
| M4 | User mode, syscalls, capability handles | done |
| M5 | Typed IPC and the capability registry | done |
| M6 | Userspace services — console, shell | done |
| M7 | Agent surface: schema discovery, audit journal | next |

M0–M3 was conventional kernel work. M4–M6 are where the thesis starts paying: authority is a handle the
kernel checks rather than a permission the caller happens to have, it can be taken back, and what those
handles name is discoverable at run time rather than known in advance. M7 turns that surface into one an
agent can read as a schema and a human can read as help.

# FUTURE

**CUT** — do not re-propose; the reason is recorded. **PARKED** — may return once its reopening condition
holds.

**1. Device-tree parsing for the physical memory map — PARKED**

- **What.** Read RAM base and extent, and device base addresses, from the flattened device tree QEMU passes
  in `x0` at entry, replacing the assumed 128 MiB and the hardcoded UART and GIC addresses.
- **Why it waits.** The assumed extent is deliberately below the machine's real size, so nothing is currently
  incorrect — only inflexible.
- **Prerequisites (owner).** Reopens the moment a second board is targeted or more than 128 MiB must be used.

**2. x86_64 port — PARKED**

- **What.** A sibling of `arch/aarch64` covering APIC, x86 paging and the different boot protocol.
- **Why it waits.** Cheap ARM boards are the chosen first target. Architecture-specific code is confined to
  named modules so the port is additive.
- **Prerequisites (owner).** Reopens if reviving old x86 laptops and desktops becomes the priority.

**3. Multi-core support — PARKED**

- **What.** Wake the secondary cores parked at boot, and per-core scheduling.
- **Why it waits.** `SpinLock` and the scheduler both assume one core.
- **Prerequisites (owner).** Reopens with M3's successor; requires replacing the spin lock with something a
  thread and the interrupt preempting it can both take.

**4. Capabilities that cross a task boundary — PARKED**

- **What.** Delegating a capability into *another* task's space. Today `delegate` derives within one space,
  which is enough for a thread to narrow its own authority and hand it to itself, and not enough for one
  task to grant another.
- **Why it waits.** It needs a second task to grant to, and the derivation links are `u32` indices into one
  table — crossing spaces means saying what a parent index means when the parent lives elsewhere.
- **Prerequisites (owner).** M5 shipped without it: the registry lets two tasks find the same endpoint
  by name, which is enough to talk and not enough to hand over authority. Reopens at M6, where a service
  must give a caller a capability to something it created.

**5. Request and reply as one operation — PARKED**

- **What.** A `call` that sends and waits for the answer on a reply channel, instead of the sender and the
  receiver each having to own an endpoint and find each other twice.
- **Why it waits.** A reply channel is a capability handed to the receiver for one use, which is the
  cross-task delegation above. Send-only is enough to prove the path works.
- **Prerequisites (owner).** Blocked on entry 4.

**6. Destroying an endpoint — PARKED**

- **What.** Endpoints are created and never freed, so the table only grows.
- **Why it waits.** Nothing creates them in a loop yet. Freeing one means deciding what happens to threads
  blocked on it and to capabilities still naming it — the same question revocation answers for capabilities,
  and it should get the same answer rather than a second mechanism.
- **Prerequisites (owner).** Reopens when a task can exit and take its endpoints with it.

**7. Loading a program from storage — PARKED**

- **What.** Reading a user program from a filesystem rather than assembling it into the kernel image.
- **Why it waits.** There is no storage driver and no filesystem. The loader itself already does the part
  that matters — copy while writable, publish to the instruction stream, then narrow to read-execute — so
  what is missing is where the bytes come from, not what is done with them.
- **Prerequisites (owner).** Reopens at M6, with the first userspace service that has to be started.

**8. Userspace built as its own binary — PARKED**

- **What.** Compiling userspace as a separate crate against a syscall shim, linked as a blob, rather than as
  modules of the kernel crate placed in a section by attribute.
- **Why it waits.** The section trick works and is checked mechanically, so what is missing is ergonomics
  rather than correctness — EL0 code currently cannot use ordinary Rust, because `core` is not reachable
  from it. That is a real cost and it grows with every program.
- **Prerequisites (owner).** Reopens when a program needs more than the shell does, or when the second
  architecture makes the per-architecture placement untenable.

**9. Interrupt masking is not enough on more than one core — PARKED**

- **What.** `SpinLock` masks interrupts while held, which makes it safe against the interrupt that would
  preempt it. Another core can still hold the lock while this one masks.
- **Why it waits.** There is one core.
- **Prerequisites (owner).** Part of multi-core support; recorded separately because it is a property of the
  lock rather than of the scheduler.

**10. Kernel heap growth on demand — PARKED**

- **What.** Extend the heap from free frames when it is exhausted, rather than fixing it at 2 MiB.
- **Why it waits.** 2 MiB is sufficient for the current kernel, and a kernel that reserves tens of megabytes
  for itself has spent what the user came for.
- **Prerequisites (owner).** Reopens when an allocation fails; needs a fault handler to trigger growth.

**11. On-target test framework — PARKED**

- **What.** `custom_test_frameworks` so tests run inside the kernel, for behaviour that needs real hardware:
  MMU semantics, exception delivery, context switching.
- **Why it waits.** The boot assertions cover the current surface.
- **Prerequisites (owner).** Reopens when a test needs hardware semantics that host arithmetic cannot model.

**12. Hardware-accelerated test harness — CUT**

- **What.** Running the automated boot test under HVF rather than TCG.
- **Why.** HVF does not intercept the semihosting trap, so the exit code the harness depends on is lost. The
  kernel boots correctly under HVF, so `scripts/run.sh --accel` uses it for running; testing stays on TCG.

**13. Buddy or size-class frame allocator — CUT**

- **What.** Replacing the bitmap with an allocator that serves large contiguous requests faster.
- **Why.** Both keep per-block structures resident whether or not anything is allocated. The bitmap's flat
  cost of one bit per frame is the correct trade for the target hardware.

**14. Saving the full register set on context switch — CUT**

- **What.** Preserving `x0`–`x18` in `Context` alongside the callee-saved registers.
- **Why.** The procedure call standard already permits a function to destroy them, so the caller has
  preserved anything it needs. A preempted thread's full state is in its trap frame regardless.

**15. Docker as a way to run Oxygen — CUT**

- **What.** Shipping a container image that boots the OS.
- **Why.** A container shares the host kernel, so it cannot boot one of its own. Running an OS needs a
  machine; QEMU is that machine.

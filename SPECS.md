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
| `crates/oxygen-mem/` | portable memory logic: addresses, frame allocator, heap |
| `crates/oxygen-aarch64/` | AArch64 encodings that are pure arithmetic |
| `scripts/` | developer and CI entry points |
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
| 8 | Unmasks IRQs | `arch/aarch64/mod.rs` |

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
| M4 | User mode, syscalls, capability handles | not started |
| M5 | Typed IPC and the capability registry | not started |
| M6 | Userspace services — console, shell | not started |
| M7 | Agent surface: schema discovery, audit journal | not started |

M0–M3 is conventional kernel work. The thesis begins paying off at M4–M5, where capabilities become
unforgeable handles and the IPC surface becomes typed and introspectable.

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

**4. Capability revocation model — PARKED**

- **What.** Whether a granted capability can be withdrawn, which decides if the kernel carries a derivation
  tree recording who granted what to whom.
- **Why it waits.** It changes the shape of every capability and is painful to retrofit.
- **Prerequisites (owner).** Must be decided before M4 begins. A human approval that cannot be withdrawn is
  not an approval, which argues for revocation despite the memory it costs.

**5. Kernel heap growth on demand — PARKED**

- **What.** Extend the heap from free frames when it is exhausted, rather than fixing it at 2 MiB.
- **Why it waits.** 2 MiB is sufficient for the current kernel, and a kernel that reserves tens of megabytes
  for itself has spent what the user came for.
- **Prerequisites (owner).** Reopens when an allocation fails; needs a fault handler to trigger growth.

**6. On-target test framework — PARKED**

- **What.** `custom_test_frameworks` so tests run inside the kernel, for behaviour that needs real hardware:
  MMU semantics, exception delivery, context switching.
- **Why it waits.** The boot assertions cover the current surface.
- **Prerequisites (owner).** Reopens when a test needs hardware semantics that host arithmetic cannot model.

**7. Hardware-accelerated test harness — CUT**

- **What.** Running the automated boot test under HVF rather than TCG.
- **Why.** HVF does not intercept the semihosting trap, so the exit code the harness depends on is lost. The
  kernel boots correctly under HVF, so `scripts/run.sh --accel` uses it for running; testing stays on TCG.

**8. Buddy or size-class frame allocator — CUT**

- **What.** Replacing the bitmap with an allocator that serves large contiguous requests faster.
- **Why.** Both keep per-block structures resident whether or not anything is allocated. The bitmap's flat
  cost of one bit per frame is the correct trade for the target hardware.

**9. Saving the full register set on context switch — CUT**

- **What.** Preserving `x0`–`x18` in `Context` alongside the callee-saved registers.
- **Why.** The procedure call standard already permits a function to destroy them, so the caller has
  preserved anything it needs. A preempted thread's full state is in its trap frame regardless.

**10. Docker as a way to run Oxygen — CUT**

- **What.** Shipping a container image that boots the OS.
- **Why.** A container shares the host kernel, so it cannot boot one of its own. Running an OS needs a
  machine; QEMU is that machine.

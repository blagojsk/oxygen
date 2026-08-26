# Oxygen

**The Swiss army knife for getting things done in an agentic era.**

An operating system written from scratch in Rust, for agents and humans in equal measure —
built to make weak and old hardware genuinely useful again.

Conventional systems expose what they can do twice, and badly: as text shaped for people, which
agents have to scrape and guess at, and as rigid ABIs that nothing can discover at runtime. Oxygen
starts from the opposite premise — a capability is one typed, self-describing thing, rendered for
whoever is asking.

**Status: milestone 1.** It boots on bare metal, routes its own exceptions, and takes timer
interrupts. It cannot yet manage memory, run a thread, or hold a conversation.

## Starting it

```bash
./scripts/run.sh
```

That is the whole thing. It builds the kernel and boots it — press **Ctrl-A** then **X** to quit.

The script finds the toolchain itself, so nothing needs to be on your `PATH` first. If Rust or
QEMU are missing it says which and how to install them:

```bash
brew install rustup qemu
rustup default nightly
```

Want it fast? `./scripts/run.sh --accel` runs the guest on your Mac's own CPU through Apple's
hypervisor instead of emulating it.

### There is no Docker here

A container shares the host's kernel — that is what makes it a container — so it cannot boot a
kernel of its own. Running an OS needs a *machine*, and QEMU is that machine. On Apple Silicon,
Docker would additionally be running its own Linux VM, so you would be emulating inside a virtual
machine to do what `./scripts/run.sh` already does directly.

### Other commands

```bash
./scripts/check.sh       # everything that must pass before a commit
./scripts/test.sh        # host unit tests for the portable crates
./scripts/boot-test.sh   # assert the kernel boots cleanly, for CI
```

## Where it runs

AArch64 on QEMU's `virt` board — a legacy-free machine, and the same architecture as the
development host. x86_64 is intended, and architecture-specific assumptions are kept in named
modules so that port is additive rather than a rewrite.

## Roadmap

| | Milestone | State |
| --- | --- | --- |
| M0 | Boots, reaches Rust, serial output, panic handling | **done** |
| M1 | Exception vectors, GIC, timer interrupt, physical frame allocator | **done** |
| M2 | MMU, page tables, kernel heap | next |
| M3 | Threads and a scheduler | |
| M4 | User mode, syscalls, capability handles | |
| M5 | Typed IPC and the capability registry | |
| M6 | Userspace services — console, storage | |
| M7 | Agent surface: schema discovery, audit journal | |

M0–M3 is conventional kernel work that any OS needs. The thesis starts paying off at M4–M5, where
capabilities become unforgeable handles and the IPC surface becomes typed and introspectable.

## Licence

MIT OR Apache-2.0.

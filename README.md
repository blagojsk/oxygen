# Oxygen

**The Swiss army knife for getting things done in an agentic era.**

An operating system written from scratch in Rust, for agents and humans in equal measure —
built to make weak and old hardware genuinely useful again.

Conventional systems expose what they can do twice, and badly: as text shaped for people, which
agents have to scrape and guess at, and as rigid ABIs that nothing can discover at runtime. Oxygen
starts from the opposite premise — a capability is one typed, self-describing thing, rendered for
whoever is asking.

**Status: milestone 0.** It boots on bare metal and can speak. That is all it does. Everything
below M0 in the roadmap is unwritten.

## Requirements

- Rust nightly (pinned by `rust-toolchain.toml`)
- QEMU with `qemu-system-aarch64`

```bash
brew install rustup qemu
export PATH="$(brew --prefix rustup)/bin:$PATH"
```

## Running it

```bash
cargo run                # boot under QEMU — Ctrl-A then X to quit
./scripts/boot-test.sh   # build and assert a clean boot
```

Expected output:

```
  oxygen 0.0.1
  an operating system for agents and humans

  [boot] aarch64 · qemu virt · EL1
  [boot] stack installed, .bss zeroed, rust reached
```

## Where it runs

AArch64 on QEMU's `virt` board — a legacy-free machine, and the same architecture as the
development host. x86_64 is intended, and architecture-specific assumptions are kept in named
modules so that port is additive rather than a rewrite.

## Roadmap

| | Milestone | State |
| --- | --- | --- |
| M0 | Boots, reaches Rust, serial output, panic handling | **done** |
| M1 | Exception vectors, timer interrupt, physical frame allocator | next |
| M2 | MMU, page tables, kernel heap | |
| M3 | Threads and a scheduler | |
| M4 | User mode, syscalls, capability handles | |
| M5 | Typed IPC and the capability registry | |
| M6 | Userspace services — console, storage | |
| M7 | Agent surface: schema discovery, audit journal | |

M0–M3 is conventional kernel work that any OS needs. The thesis starts paying off at M4–M5, where
capabilities become unforgeable handles and the IPC surface becomes typed and introspectable.

## Licence

MIT OR Apache-2.0.

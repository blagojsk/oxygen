# Oxygen

**The Swiss army knife for getting things done in an agentic era.**

An operating system written from scratch in Rust, for agents and humans in equal measure —
built to make weak and old hardware genuinely useful again.

Conventional systems expose what they can do twice, and badly: as text shaped for people, which
agents have to scrape and guess at, and as rigid ABIs that nothing can discover at runtime. Oxygen
starts from the opposite premise — a capability is one typed, self-describing thing, rendered for
whoever is asking.

**Status: milestone 6 — it has a prompt.** Boot it and type at it:

```
oxygen$ services
  echo
oxygen$ echo it works
it works
```

Underneath: it routes its own exceptions, maps itself with page tables that refuse a write to its
own code, allocates from a kernel heap, and preempts between threads. Unprivileged programs reach
the kernel only by presenting a capability — one that can be narrowed, handed on, and taken back
again — and find each other by publishing endpoints under names, then exchanging typed messages
over them. The shell is one of those programs, with no more authority than it was handed.

## Starting it

```bash
./scripts/run.sh
```

That is the whole thing. It builds the kernel and boots it — press **Ctrl-A** then **X** to quit.

The script finds the toolchain itself, so nothing needs to be on your `PATH` first. If Rust or
QEMU are missing it says which and how to install them:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust, per rust-lang.org
brew install qemu                                                # the machine to boot on
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
| M2 | MMU, per-section mapping with W^X, kernel heap | **done** |
| M3 | Threads, context switch, preemptive scheduler | **done** |
| M4 | User mode, syscalls, capability handles | **done** |
| M5 | Typed IPC and the capability registry | **done** |
| M6 | Userspace services — console, shell | **done** |
| M7 | Agent surface: schema discovery, audit journal | next |

M0–M3 was conventional kernel work that any OS needs. M4–M6 are where the thesis starts paying: a
program holds no ambient authority, only handles the kernel checks — a grant can be withdrawn,
because an approval nobody can take back is not an approval — and what those handles name is a
typed, enumerable surface rather than a set of numbers you had to be told. `services` is that
surface, seen by a human. M7 is the same surface seen by an agent.

## Licence

[MIT](LICENSE-MIT). Contributions are accepted under the same licence.

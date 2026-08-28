# Project

Oxygen is an operating system written from scratch in Rust, built on the premise that its users
are **agents and people in equal measure**. It is not a Linux distribution, not a wrapper around an
existing kernel, and not an assistant bolted onto a conventional OS.

## What it is for

Everyday office work, education and software engineering — a general-purpose system, not a
demo. Two constraints shape every decision, and they pull against each other on purpose:

- **It must run well on the weakest hardware imaginable.** The goal is to make old and cheap
  machines competitive again while hardware stays expensive. Memory is the scarce resource, then
  storage, then CPU. A feature that costs megabytes of resident memory needs to justify itself
  against the machine it prices out.
- **Agents and people can both spin off containers.** Isolated execution is a system primitive,
  not an add-on. This falls out of the capability model rather than fighting it: on a monolithic
  kernel a container is a process with a retrofitted restricted view, whereas here a process only
  ever holds the authority it was handed, so a container is simply a process given a curated set of
  capabilities. Expect isolation to be cheap and the interesting questions to be about images,
  compatibility and resource accounting instead.
- **It is agent-native from the first commit.** Agents are first-class users of the system, able
  to run, be interacted with, and eventually be built in — not an application layered on top.
- **A task factory is a system service, not an app.** Work — a refactor, a spreadsheet, a lesson —
  is something an agent can be given and carry to completion under supervision, with project
  tracking and a human in the loop. Two parts of the design already carry most of this weight: an
  approval by a human *is* a capability grant, and the audit journal *is* what tracking reads. Do
  not design a second mechanism for either.

The tension is the interesting part: agents are usually memory-hungry and the target machines are
not. That is a design constraint, not a contradiction to wave away — expect inference to be
remote-first with a local path where the hardware allows, and expect the agent interface to matter
more than any bundled model.

## The design thesis

Conventional systems expose their capabilities twice and badly: as text streams shaped for humans,
which agents must scrape and guess at, and as rigid C ABIs, which nothing can discover at runtime.
Oxygen treats a capability as one typed, self-describing thing and renders it for whoever is
asking.

The invariants that follow from that, and which changes are expected to respect:

- **Discoverable.** The system can enumerate what it is able to do, with schemas. Nothing should
  require reading a man page or parsing prose to call correctly.
- **Structured.** Calls and results are typed values, not byte streams awaiting a parser.
- **Capability-secured.** Authority travels as unforgeable handles, never as ambient permission. An
  agent acting on its own must be unable to exceed what it was granted — this is a safety property,
  not a feature.
- **Auditable.** Effectful operations are journaled and attributable, so what an autonomous actor
  did can be reconstructed after the fact.
- **One surface, two audiences.** A human interface and an agent interface are renderings of the
  same contract, never two implementations that drift.

# Layout

A Cargo workspace. Everything freestanding — the host toolchain must never leak in.

| Path | Holds |
| --- | --- |
| `kernel/` | the microkernel: boot, memory, scheduling, IPC |
| `kernel/linker.ld` | memory layout; `build.rs` passes it to the linker |
| `scripts/` | developer and CI entry points |
| `.cargo/config.toml` | default target and the QEMU `cargo run` runner |
| `rust-toolchain.toml` | pinned compiler, components and target |

# Toolchain

- **Rust nightly**, pinned in `rust-toolchain.toml`. Freestanding work needs unstable features;
  pinning means an upstream nightly change breaks the build deliberately rather than by surprise.
- **Target:** `aarch64-unknown-none-softfloat`. Floating point is off because the kernel must not
  use FP/SIMD registers before it saves them on context switch.
- **QEMU** `virt` board is the reference machine. x86_64 is expected later; keep architecture
  assumptions inside clearly-named modules so that port is additive.
- Installed via rustup from rust-lang.org, so the shims are at `~/.cargo/bin` and the standard
  `~/.cargo/env` puts them on PATH. The scripts source it themselves when a shell has not.

# Building and running

```
./scripts/run.sh         # build and boot it (Ctrl-A X to quit); --accel for host-CPU speed
./scripts/check.sh       # the full pre-commit gate: fmt, clippy, host tests, boot
./scripts/test.sh        # host unit tests for the portable crates
./scripts/boot-test.sh   # assert a clean boot; this is what CI runs
```

The scripts locate the toolchain themselves via `scripts/_toolchain.sh`. Never assume `cargo` is
directly invocable: a non-interactive shell or a CI runner can have a perfectly good install and
still not have it on PATH.

`OXYGEN_SELFTEST=1` makes the kernel exit via semihosting instead of halting, which is what turns
a boot into a pass/fail result. Semihosting is an emulator debug channel: it is scaffolding for
development and must never become load-bearing for kernel behaviour.

# Coding guidelines

- `#![no_std]` everywhere. No `std`, no allocator until one is written, no unwinding — panics
  abort, set in the workspace profiles.
- **Every `unsafe` block carries a `SAFETY:` comment** stating the invariant that makes it sound.
  An `unsafe` block without one is treated as a defect. In a kernel this is the only review
  mechanism there is.
- Volatile access for MMIO, always. A normal read or write to a device register is a bug the
  optimiser is entitled to delete.
- Hardware constants (base addresses, bit positions) are named, never inlined as magic numbers, and
  carry a comment naming the register they come from.
- Prefer iterators and slices to hand-rolled pointer loops wherever the generated code allows it.
- Public items get doc comments explaining *why*, not restatements of the signature.
- Formatting is `cargo fmt`; lints are `cargo clippy` and are expected to be clean.

# Testing

- Add tests only when explicitly asked.
- `scripts/boot-test.sh` is the current suite: it proves the kernel boots and reaches Rust. It fails
  on panic (non-zero exit) and on hang (watchdog).
- Unit tests that need no hardware belong in-crate under `#[cfg(test)]` and run on the host.

# Commit messages

Conventional Commits: `type(scope): summary` (`feat(mm):`, `fix(uart):`, `docs:`, `chore:`).
Imperative, under ~72 characters, with the why and any caveats in the body.

# Branching

- Never do feature or fix work directly on the default branch. Branch, commit there, and merge only
  when explicitly asked.
- CI configuration under `.github/workflows/` may be changed on the default branch directly.
- This applies per request: a merge approved once does not authorise the next.

# Model use and delegation

Match the model to the job, and spend the expensive ones on thinking rather than typing. Whichever
model is running as the primary delegates work *downward*, never sideways or up:

| Primary | Delegates to | Does itself |
| --- | --- | --- |
| **Fable** | Opus or Sonnet | Planning and orchestration only — no implementation |
| **Opus** | Sonnet or Haiku | Plans, then implements only what genuinely needs its judgement |
| **Sonnet** | Haiku | Implementation |

**Pick the cheapest worker that will do the job well — never default to the most capable one.**
Choosing the strongest model available for every task wastes the budget that makes delegation
worth doing, and it is the primary's job to make that call, task by task, on how much judgement
the work actually needs.

As a guide: unfamiliar design, subtle concurrency, `unsafe` whose soundness argument is not
obvious, and anything where being wrong is expensive and quiet — kernel bring-up, page tables,
capability logic — go to the stronger worker. Mechanical refactors, moving code between modules,
writing tests against a settled contract, boilerplate, and repetitive edits with a clear
specification go to the cheaper one. When a cheap worker returns something wrong, escalate that
task rather than lowering the bar for the next one.

# Before making a commit or providing a suggestion

- `cargo build` cleanly.
- `cargo clippy` with no warnings, `cargo fmt --check` clean.
- `./scripts/boot-test.sh` passes.
- If any of these could not be run, say so explicitly and name what was skipped. Never report work
  as verified on the strength of a subset.

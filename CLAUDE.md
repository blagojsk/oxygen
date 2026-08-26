# Project

Oxygen is an operating system written from scratch in Rust, built on the premise that its users
are **agents and people in equal measure**. It is not a Linux distribution, not a wrapper around an
existing kernel, and not an assistant bolted onto a conventional OS.

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
- Installed through Homebrew, so the shims are **not** on the default PATH:
  ```
  export PATH="$(brew --prefix rustup)/bin:$PATH"
  ```

# Building and running

```
cargo build              # build the kernel
cargo run                # build and boot it under QEMU (Ctrl-A X to quit)
./scripts/boot-test.sh   # build and assert it boots cleanly — this is the test suite
```

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

# Before making a commit or providing a suggestion

- `cargo build` cleanly.
- `cargo clippy` with no warnings, `cargo fmt --check` clean.
- `./scripts/boot-test.sh` passes.
- If any of these could not be run, say so explicitly and name what was skipped. Never report work
  as verified on the strength of a subset.

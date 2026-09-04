**CLAUDE.md for rules, SPECS.md for the record, nothing else.** What the system *is* — layout, memory map,
boot order, milestones, deferred decisions — lives in SPECS.md. Read it before changing anything structural.

**When a FUTURE item in SPECS.md ships, the same commit moves it to PRESENT.**

# Design rules

- Every change must respect the five invariants recorded in SPECS.md: discoverable, structured,
  capability-secured, auditable, one surface for two audiences.
- **Justify any feature that costs megabytes of resident memory** against the machine it prices out — see
  the target hardware constraints in SPECS.md.
- Never design a second mechanism for human approval or for task tracking. An approval **is** a capability
  grant; the audit journal **is** what tracking reads.
- Keep architecture-specific assumptions inside clearly-named modules under `arch/`, so a second architecture
  is additive rather than a rewrite.
- Never let the host toolchain leak into the kernel. Everything is freestanding.

# Coding guidelines

- `#![no_std]` everywhere. No `std`, no unwinding — panics abort, set in the workspace profiles.
- **Every `unsafe` block carries a `SAFETY:` comment** stating the invariant that makes it sound. An `unsafe`
  block without one is a defect; in a kernel this is the only review mechanism there is.
- Use volatile access for MMIO, always. A normal read or write to a device register is a bug the optimiser is
  entitled to delete.
- Name hardware constants — base addresses, bit positions — never inline them, and comment the register they
  come from.
- Prefer iterators and slices to hand-rolled pointer loops wherever the generated code allows.
- Give public items doc comments explaining *why*, never restatements of the signature.
- Format with `cargo fmt`; keep `cargo clippy` clean.

## Vocabulary

- One name per concept. Per-tenant or per-organization toggles are **features** (feature flags), never
  "modules".

# Testing

- Add tests only when explicitly asked.
- Assert observable behaviour, never the absence of a crash. A kernel that fails to crash proves nothing.
- Put logic that needs no hardware in a portable crate under `#[cfg(test)]` so it runs on the host, and keep
  hardware access behind a trait so the logic can be tested against a fake.
- Never let `OXYGEN_SELFTEST` or semihosting become load-bearing for kernel behaviour. Semihosting is an
  emulator debug channel and is development scaffolding only.

# Building and verifying

```
./scripts/run.sh         # build and boot it (Ctrl-A X to quit); --accel for host-CPU speed
./scripts/check.sh       # the full pre-commit gate: fmt, clippy, host tests, boot
./scripts/test.sh        # host unit tests for the portable crates
./scripts/boot-test.sh   # assert a clean boot
```

- Never assume `cargo` is directly invocable: a non-interactive shell or a CI runner can have a working
  install and no `cargo` on PATH. The scripts resolve it via `scripts/_toolchain.sh`.
- Never modify `rust-toolchain.toml`, `Cargo.toml` or `.cargo/config.toml` unless explicitly asked.

## False greens to watch for

- A pipeline's exit status is its **last** command's. `set -e` will not catch a failure piped into `tail`.
- Lints must run per target: the kernel on bare metal, portable crates on the host. `--all-targets` on a
  freestanding target fails to link a `test` crate that no such target has.
- A boot that prints nothing and exits 0 is not a pass. Check the assertion lines, not just the status.

# Branching

- Never do feature or fix work directly on the default branch. Branch, commit there, and merge only when
  explicitly asked.
- CI configuration under `.github/workflows/` may be changed on the default branch directly.
- This applies per request: a merge approved once does not authorise the next.
- **A merge is not finished until the branch is gone and the issue is closed.** Delete the merged branch,
  local and remote, in the same step as the merge, and close every issue the merge resolved with a comment
  naming the pull request or commit. Merged branches only — a parked branch stays until its work lands. A
  branch that outlives its merge gets built on by mistake; an issue that outlives its fix gets planned twice.

# Commit messages

Conventional Commits: `type(scope): summary` (`feat(mm):`, `fix(uart):`, `docs:`, `chore:`). Imperative, under
~72 characters, with the why and any caveats in the body. Never narrate history in SPECS.md — that is what
commit messages are for.

# Model use and delegation

**Delegation.** Spend the expensive models on thinking, not typing. **The primary model decides what it does itself
and what it hands down — it never reaches for a model above it.** And for each task it hands down it chooses the
**cheapest worker that will do the job well**: the stronger worker is a choice the task has to earn — real ambiguity,
a design call, an unfamiliar subsystem — never the default it falls back to.

- **Fable** plans and orchestrates, and does **not** implement. Per task it picks between its two workers: **Sonnet**
  for anything the plan fully specifies, **Opus** only where the task still needs judgement mid-flight.
- **Opus** plans, then implements the parts that need its own judgement; per task it picks between **Sonnet** for
  specified work and **Haiku** for the mechanical passes — a rename across a package, a sweep for one pattern, a
  fixture updated in ten files.
- **Sonnet** implements.

A delegated task carries what the delegate cannot infer: the file, the rule it must not break, and how the result is
verified. Delegation is not a way to avoid reading the code — whoever plans still has to know what they are asking for.

# Before making a commit or providing a suggestion

- `./scripts/check.sh` passes.
- If any part could not be run, say so explicitly and name what was skipped. Never report work as verified on
  the strength of a subset.

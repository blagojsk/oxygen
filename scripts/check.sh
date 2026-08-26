#!/usr/bin/env bash
# Everything that must be green before a commit.
#
# Lints run twice on purpose: the kernel is linted for the bare-metal target it actually runs on,
# and the portable crates for the host, where their tests exist. Linting portable crates with
# --all-targets on bare metal fails to link a `test` crate that no freestanding target has.
set -euo pipefail
cd "$(dirname "$0")/.."
HOST=$(rustc -vV | awk '/^host:/ {print $2}')

echo "==> fmt"
cargo fmt --all --check

echo "==> clippy (kernel, bare metal)"
cargo clippy -p oxygen-kernel -- -D warnings

echo "==> clippy (portable crates, host)"
cargo clippy --workspace --exclude oxygen-kernel --all-targets --target "$HOST" -- -D warnings

echo "==> host tests"
./scripts/test.sh

echo "==> boot"
./scripts/boot-test.sh

echo "==> all green"

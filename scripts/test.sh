#!/usr/bin/env bash
# Host-side unit tests for the portable crates.
#
# .cargo/config.toml pins the bare-metal target, so the host triple is passed explicitly —
# otherwise cargo tries to link a test harness for a target that has no `test` crate.
set -euo pipefail
cd "$(dirname "$0")/.."
HOST=$(rustc -vV | awk '/^host:/ {print $2}')
echo "==> host tests on $HOST"
cargo test --workspace --exclude oxygen-kernel --target "$HOST" "$@"

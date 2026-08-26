#!/usr/bin/env bash
# Starts Oxygen. This is the one command you need.
#
#   ./scripts/run.sh            boot it (Ctrl-A then X to quit)
#   ./scripts/run.sh --accel    boot on the host CPU via Apple's hypervisor, at native speed
#
# There is no Docker and no VM to install: QEMU *is* the machine. A container could not run this
# even in principle, because a container shares the host's kernel and this replaces it.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/_toolchain.sh

MACHINE="virt"
if [ "${1:-}" = "--accel" ]; then
  # HVF runs the guest on the Mac's own CPU instead of emulating. Semihosting is not intercepted
  # under it, so the selftest harness deliberately stays on emulation; this path is for using the
  # OS, not testing it.
  MACHINE="virt,accel=hvf"
  CPU=(-cpu host)
  echo "==> hardware-accelerated (hvf)"
else
  CPU=(-cpu cortex-a72)
fi

echo "==> building"
cargo build

KERNEL=target/aarch64-unknown-none-softfloat/debug/oxygen-kernel
echo "==> booting — press Ctrl-A then X to quit"
echo
exec qemu-system-aarch64 \
  -machine "$MACHINE" "${CPU[@]}" \
  -smp 1 -m 256M -nographic \
  -semihosting-config enable=on,target=native \
  -kernel "$KERNEL"

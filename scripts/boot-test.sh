#!/usr/bin/env bash
# Boots the kernel under QEMU and fails if it does not reach kernel_main cleanly.
#
# The kernel asks QEMU to exit via semihosting, so this is a real pass/fail signal rather
# than grepping the serial log: a panic exits non-zero, a hang trips the watchdog below.
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=aarch64-unknown-none-softfloat
PROFILE=${PROFILE:-debug}
KERNEL="target/$TARGET/$PROFILE/oxygen-kernel"

echo "==> building (selftest)"
if [ "$PROFILE" = "release" ]; then
  OXYGEN_SELFTEST=1 cargo build --release
else
  OXYGEN_SELFTEST=1 cargo build
fi

echo "==> booting $KERNEL"
# macOS has no coreutils timeout, so the watchdog is a background sleep that kills a
# kernel which never asks to exit — otherwise a boot hang would block CI forever.
qemu-system-aarch64 -machine virt -cpu cortex-a72 -smp 1 -m 256M \
  -nographic -semihosting-config enable=on,target=native -kernel "$KERNEL" &
QEMU_PID=$!
( sleep 15; kill -9 "$QEMU_PID" 2>/dev/null || true ) &
WATCHDOG=$!

set +e
wait "$QEMU_PID"
STATUS=$?
set -e
kill "$WATCHDOG" 2>/dev/null || true

if [ "$STATUS" -eq 0 ]; then
  echo "==> boot ok"
else
  echo "==> boot FAILED (exit $STATUS)" >&2
fi
exit "$STATUS"

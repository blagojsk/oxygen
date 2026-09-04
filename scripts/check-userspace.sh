#!/usr/bin/env bash
#
# Asserts that nothing in the EL0 section branches out of it.
#
# Userspace here is compiled into the kernel image but runs at EL0, where kernel .text is not
# executable. A call out of `.user` — an inserted overflow check, a bounds check, a `core` helper
# the compiler chose not to inline — is an instruction abort at run time, and it only shows up when
# that particular line happens to run. This makes it a build failure instead.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=_toolchain.sh
source "$here/_toolchain.sh"

profile="${PROFILE:-debug}"
binary="$here/../target/aarch64-unknown-none-softfloat/$profile/oxygen-kernel"
[ -f "$binary" ] || { echo "no kernel at $binary — build it first"; exit 1; }

sysroot="$(rustc --print sysroot)"
objdump="$(find "$sysroot" -name llvm-objdump -type f 2>/dev/null | head -1)"
nm_tool="$(find "$sysroot" -name llvm-nm -type f 2>/dev/null | head -1)"
if [ -z "$objdump" ] || [ -z "$nm_tool" ]; then
  echo "llvm-objdump/llvm-nm not found — add the llvm-tools component"
  exit 1
fi

"$nm_tool" "$binary" > "$here/../target/.user-symbols"
"$objdump" -d --section=.user "$binary" > "$here/../target/.user-disasm"

python3 "$here/check_userspace.py" \
  "$here/../target/.user-symbols" \
  "$here/../target/.user-disasm"

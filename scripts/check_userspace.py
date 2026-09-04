"""Fails if any branch in the EL0 section targets an address outside it.

Kept in Python rather than awk because the comparison is on hexadecimal addresses, and macOS awk
has no strtonum — comparing them as strings would quietly pass everything, which is exactly the
kind of check that is worse than no check at all.
"""

import re
import sys

symbols_path, disasm_path = sys.argv[1], sys.argv[2]

symbols = []
bounds = {}
for line in open(symbols_path):
    parts = line.split()
    if len(parts) < 3:
        continue
    address, name = parts[0], parts[-1]
    try:
        value = int(address, 16)
    except ValueError:
        continue
    symbols.append((value, name))
    if name in ("__user_start", "__user_end"):
        bounds[name] = value

if len(bounds) != 2:
    sys.exit("no __user_start/__user_end symbols in the image")
low, high = bounds["__user_start"], bounds["__user_end"]
symbols.sort()


def owner(address):
    """The symbol an address falls inside, for a message that names the culprit."""
    found = "?"
    for value, name in symbols:
        if value <= address:
            found = name
        else:
            break
    return found


branch = re.compile(r"\bbl?\s+(0x[0-9a-f]+)")
escapes = set()
for line in open(disasm_path):
    match = branch.search(line)
    if match:
        target = int(match.group(1), 16)
        if not low <= target < high:
            escapes.add(target)

if escapes:
    print(f"==> userspace FAILED: branches leave .user ({low:#x}..{high:#x})")
    for target in sorted(escapes):
        print(f"    {target:#x}  {owner(target)}")
    print("    EL0 cannot fetch from kernel .text — every one of these is an instruction abort.")
    sys.exit(1)

print("==> userspace ok (no branch leaves .user)")

# Locates the Rust toolchain, so the scripts work in a shell that has not sourced it.
#
# rustup installs to ~/.cargo/bin and its installer offers to add that to PATH, but a
# non-interactive shell, a fresh terminal or a CI runner may not have it. Finding it here means the
# scripts never depend on the caller's environment being set up.
if ! command -v cargo >/dev/null 2>&1; then
  if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  elif [ -x "$HOME/.cargo/bin/cargo" ]; then
    PATH="$HOME/.cargo/bin:$PATH"
    export PATH
  fi
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found." >&2
  echo "       install it the way the Rust docs recommend:" >&2
  echo "       curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
fi

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "error: qemu-system-aarch64 not found." >&2
  echo "       install it with:  brew install qemu" >&2
  exit 1
fi

# Locates the Rust toolchain, so the scripts work in a plain shell with nothing configured.
#
# Homebrew's rustup keeps its shims in its own prefix rather than ~/.cargo/bin, and that prefix is
# not on the default PATH. Rather than require every user to edit a shell profile before the OS
# will build, the scripts find it themselves.
if ! command -v cargo >/dev/null 2>&1; then
  for candidate in \
    "$HOME/.cargo/bin" \
    "$(brew --prefix rustup 2>/dev/null)/bin" \
    "/opt/homebrew/opt/rustup/bin" \
    "/usr/local/opt/rustup/bin"
  do
    if [ -x "$candidate/cargo" ]; then
      PATH="$candidate:$PATH"
      export PATH
      break
    fi
  done
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found." >&2
  echo "       install it with:  brew install rustup && rustup default nightly" >&2
  exit 1
fi

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "error: qemu-system-aarch64 not found." >&2
  echo "       install it with:  brew install qemu" >&2
  exit 1
fi

#!/usr/bin/env bash
# Install db-studio on Linux (and other Unix) to ~/.local/bin
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
mkdir -p "$BIN_DIR"

echo "→ building release…"
cargo build -p ide-app --release

SRC="$ROOT/target/release/db-studio"
install -m 755 "$SRC" "$BIN_DIR/db-studio"

echo "→ installed: $BIN_DIR/db-studio"
if ! command -v db-studio >/dev/null 2>&1; then
  echo "  note: add to PATH if needed:"
  echo "    export PATH=\"$BIN_DIR:\$PATH\""
fi
"$BIN_DIR/db-studio" --version 2>/dev/null || ls -lh "$BIN_DIR/db-studio"
echo "done."

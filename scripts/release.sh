#!/usr/bin/env bash
# Build release binary for db-studio.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p ide-app --release
bin="target/release/db-studio"
echo "built: $bin"
"$bin" --help 2>/dev/null || true
ls -lh "$bin"

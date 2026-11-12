#!/usr/bin/env bash
# Update db-studio from git and reinstall to ~/.local/bin
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -d .git ]]; then
  echo "→ git pull…"
  git pull --ff-only || git pull
fi

exec "$ROOT/scripts/install.sh"

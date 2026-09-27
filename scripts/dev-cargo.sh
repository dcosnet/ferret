#!/usr/bin/env bash
# Convenience wrapper: source the rootless libmpv prefix env and run cargo.
# Usage: bash scripts/dev-cargo.sh check|build|test|clippy [args...]
set -euo pipefail
ROOT="/home/z/my-project/ferret"
source "$ROOT/mpv-prefix/env.sh"
source "$HOME/.cargo/env"
cd "$ROOT"
exec cargo "$@"

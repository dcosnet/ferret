#!/usr/bin/env bash
# scripts/build.sh
#
# One-shot build: ensures libmpv is set up, then cargo builds the release binary.
# Optionally runs lint checks and tests.
#
# Usage:
#   ./scripts/build.sh                # build release
#   ./scripts/build.sh --debug        # build debug
#   ./scripts/build.sh --clean        # clean + rebuild
#   ./scripts/build.sh --test         # run cargo test after build
#   ./scripts/build.sh --lint         # run clippy + bracket audit after build
#   ./scripts/build.sh --ci           # lint + test + build (full CI pass)

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$HERE/.." && pwd)"
PREFIX="$PROJECT_ROOT/mpv-prefix"

cd "$PROJECT_ROOT"

# Step 1: ensure libmpv prefix exists.
if [ ! -f "$PREFIX/env.sh" ]; then
    echo "==> mpv-prefix not found; running setup-libmpv.sh first..."
    "$HERE/setup-libmpv.sh"
fi

# Step 2: source env.
# shellcheck disable=SC1091
source "$PREFIX/env.sh"

# Step 3: ensure rust toolchain.
if ! command -v cargo >/dev/null 2>&1; then
    echo "==> cargo not found; installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
fi

# Step 4: parse flags.
PROFILE="release"
RUN_TEST=0
RUN_LINT=0
for arg in "$@"; do
    case "$arg" in
        --debug)  PROFILE="dev" ;;
        --clean)  cargo clean ;;
        --test)   RUN_TEST=1 ;;
        --lint)   RUN_LINT=1 ;;
        --ci)     RUN_LINT=1; RUN_TEST=1 ;;
        *) ;;
    esac
done

# Step 5: cargo build.
echo "==> cargo build ($PROFILE)..."
if [ "$PROFILE" = "release" ]; then
    cargo build --release
    echo
    echo "==> Built: $PROJECT_ROOT/target/release/ferret"
else
    cargo build
    echo
    echo "==> Built: $PROJECT_ROOT/target/debug/ferret"
fi

# Step 6: lint (optional).
if [ "$RUN_LINT" -eq 1 ]; then
    echo
    echo "==> cargo clippy..."
    cargo clippy --release -- -D warnings

    echo
    echo "==> Bracket-balance audit..."
    python3 "$HERE/audit_brackets.py" "$PROJECT_ROOT"

    echo
    echo "==> &T deref audit (heuristic)..."
    python3 "$HERE/audit_deref.py" "$PROJECT_ROOT"
fi

# Step 7: test (optional).
if [ "$RUN_TEST" -eq 1 ]; then
    echo
    echo "==> cargo test..."
    cargo test --release
fi

echo
echo "==> Done."

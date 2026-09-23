#!/usr/bin/env bash
# Linux build + verification for YeLee' PacketSage (engine/CLI + contract suites).
#
# It reproduces the `check` / `test` / `python` jobs of .github/workflows/ci.yml on a
# plain Linux box or WSL, so a Linux build needs no CI at all:
#
#   bash scripts/build_linux.sh              # fmt + clippy + tests + release binary
#   bash scripts/build_linux.sh --quick      # release binary only
#   SKIP_PYTHON=1 bash scripts/build_linux.sh
#
# Prerequisites:
#   * rustup (stable) — the workspace pins `channel = "stable"` (rust-toolchain.toml);
#   * a C toolchain + pkg-config (`apt-get install -y build-essential pkg-config`) —
#     the bundled sqlite code is compiled by `cc`;
#   * python3 with `httpx` + `pytest` for the contract suites
#     (`python3 -m venv .venv && .venv/bin/pip install httpx pytest`).
#
# On a slow link to crates.io, point both rustup and cargo at a mirror first, e.g.
#   export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
#   export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
#   printf '[source.crates-io]\nreplace-with = "tuna"\n\n[source.tuna]\nregistry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"\n' >> ~/.cargo/config.toml
#
# Artifact: $CARGO_TARGET_DIR/release/packetsage (default `target/release/packetsage`).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

QUICK=0
for arg in "$@"; do
    case "$arg" in
        --quick) QUICK=1 ;;
        *) echo "usage: bash scripts/build_linux.sh [--quick]" >&2; exit 2 ;;
    esac
done

CARGO="${CARGO:-}"
if [[ -z "$CARGO" ]]; then
    # rustup installed with --no-modify-path keeps its proxies out of PATH.
    if command -v cargo >/dev/null 2>&1; then
        CARGO="$(command -v cargo)"
    elif [[ -x "$HOME/.cargo/bin/cargo" ]]; then
        CARGO="$HOME/.cargo/bin/cargo"
    else
        echo "cargo not found — install rustup (https://rustup.rs) or set \$CARGO" >&2
        exit 3
    fi
fi
PYTHON="${PYTHON:-python3}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/release/packetsage"

if [[ "$QUICK" != 1 ]]; then
    echo "==> cargo fmt --all --check"
    "$CARGO" fmt --all --check
    echo "==> cargo clippy --workspace --all-targets -- -D warnings"
    "$CARGO" clippy --workspace --all-targets -- -D warnings
    echo "==> cargo test --workspace"
    "$CARGO" test --workspace
fi

echo "==> cargo build --release"
"$CARGO" build --release --workspace
echo "engine: $BIN"
"$BIN" --version

if [[ "${SKIP_PYTHON:-0}" == 1 ]]; then
    echo "-- python contract suites skipped (SKIP_PYTHON=1)"
elif ! "$PYTHON" -c 'import httpx, pytest' 2>/dev/null; then
    echo "-- python contract suites skipped (no httpx/pytest for $PYTHON)"
else
    if [[ ! -f samples/synth-mixed.pcap ]]; then
        echo "==> samples/synth-mixed.pcap is missing — generating it"
        "$PYTHON" scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 --profile mixed
    fi
    echo "==> tests/cli/gui_contract.py (S40-S45)"
    PYTHONPATH=agent "$PYTHON" tests/cli/gui_contract.py --binary "$BIN"
    echo "==> tests/sidecar/protocol_cases.py (S57-S70)"
    PYTHONPATH=agent "$PYTHON" tests/sidecar/protocol_cases.py --binary "$BIN"
fi

echo "done."

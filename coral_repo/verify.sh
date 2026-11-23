#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

./scripts/setup-rootfs.sh

echo "[1/4] Building Coral workspace..."
cargo build --workspace

echo "[2/4] Running unit & E2E tests (OCI + CRI v1 gRPC)..."
cargo test --workspace

echo "[3/4] Running strict Clippy lint checks..."
cargo clippy --workspace --all-targets -- -D warnings

echo "[4/4] Verifying live container execution via coral CLI..."
OUTPUT="$(./target/debug/coral run --bundle . verify-live-box)"
echo "${OUTPUT}"

if [[ "${OUTPUT}" == *"hostname=coral-container"* && "${OUTPUT}" == *"pid=1"* ]]; then
    echo "All Coral v2.8.0 verifications PASSED!"
else
    echo "Verification failed: unexpected container output"
    exit 1
fi

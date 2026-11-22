#!/usr/bin/env bash
set -euo pipefail

# Coral vs Docker (and other OCI runtimes) container startup benchmark.
#
# Usage:
#   ./scripts/benchmark.sh                          # 30 runs x 3 rounds, cold + warm
#   RUNS=50 ROUNDS=5 ./scripts/benchmark.sh          # custom run / round counts
#   CPUS=0-7 ./scripts/benchmark.sh                  # pin the harness to a CPU set
#   MODES=warm ./scripts/benchmark.sh                # measure only the warm cache mode
#   MAX_LOAD=1.0 ./scripts/benchmark.sh              # wait until loadavg1 < 1.0 per round
#   PULL_EXTRA=1 ./scripts/benchmark.sh              # also benchmark busybox + debian
#   CORAL_BIN=/usr/bin/coral CORAL_DAEMON_BIN=/usr/bin/coral-daemon ./scripts/benchmark.sh
#                                                   # benchmark the installed runtime
#   JSON_OUT=/tmp/bench.json ./scripts/benchmark.sh  # also write a machine-readable summary
#
# `CORAL_BIN` / `CORAL_DAEMON_BIN` point the benchmark at an installed build (e.g. the
# system-wide `coral-runtime` .deb). When both are set, no release build is triggered.
#
# Cache and load control live in scripts/benchmark.py:
#   - cold mode evicts the runtime binaries and the container image files from the
#     page cache before every measured run (posix_fadvise, no root needed)
#   - warm mode measures the same workload again with everything cached
#   - the harness and every child are pinned to a CPU set; a running dockerd is
#     pinned to the same set when possible
#   - each workload runs over several rounds; the headline number is the median
#     of the per-round medians

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${RUNS:-30}"
WARMUP="${WARMUP:-3}"
ROUNDS="${ROUNDS:-3}"
MODES="${MODES:-cold,warm}"
CPUS="${CPUS:-}"
MAX_LOAD="${MAX_LOAD:-0}"
PULL_EXTRA="${PULL_EXTRA:-0}"
ONLY="${ONLY:-}"
JSON_OUT="${JSON_OUT:-}"
CORAL_BIN="${CORAL_BIN:-${ROOT_DIR}/target/release/coral}"
CORAL_DAEMON_BIN="${CORAL_DAEMON_BIN:-${ROOT_DIR}/target/release/coral-daemon}"

cd "${ROOT_DIR}"
./scripts/setup-rootfs.sh >/dev/null

if [[ ! -x "${CORAL_BIN}" || ! -x "${CORAL_DAEMON_BIN}" ]]; then
    echo "[bench] Building release binaries (cargo build --release --workspace)..."
    cargo build --release --workspace
fi

ARGS=(
    --runs "${RUNS}"
    --warmup "${WARMUP}"
    --rounds "${ROUNDS}"
    --modes "${MODES}"
    --max-load "${MAX_LOAD}"
    --coral-bin "${CORAL_BIN}"
    --daemon-bin "${CORAL_DAEMON_BIN}"
    --rootfs "${ROOT_DIR}/rootfs"
)
[[ -n "${CPUS}" ]] && ARGS+=(--cpus "${CPUS}")
[[ -n "${ONLY}" ]] && ARGS+=(--only "${ONLY}")
[[ -n "${JSON_OUT}" ]] && ARGS+=(--json "${JSON_OUT}")
[[ "${PULL_EXTRA}" == "1" ]] && ARGS+=(--pull-extra)

exec python3 "${ROOT_DIR}/scripts/benchmark.py" "${ARGS[@]}"

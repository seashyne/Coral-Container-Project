#!/usr/bin/env bash
set -euo pipefail

# Coral vs Docker (and other OCI runtimes) container startup benchmark.
# Usage:
#   ./scripts/benchmark.sh              # default: 30 runs (release build)
#   RUNS=50 ./scripts/benchmark.sh      # custom run count
#   PULL_EXTRA=1 ./scripts/benchmark.sh # also pull & benchmark busybox:musl + debian:bookworm-slim
#   CORAL_BIN=/usr/bin/coral CORAL_DAEMON_BIN=/usr/bin/coral-daemon ./scripts/benchmark.sh
#                                           # benchmark the installed runtime instead of the repo build
#
# `CORAL_BIN` / `CORAL_DAEMON_BIN` point the benchmark at an installed build (e.g. the
# system-wide `coral-runtime` .deb). When both are set, no release build is triggered.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${RUNS:-30}"
WARMUP="${WARMUP:-3}"
PULL_EXTRA="${PULL_EXTRA:-0}"
CORAL_BIN="${CORAL_BIN:-${ROOT_DIR}/target/release/coral}"
CORAL_DAEMON_BIN="${CORAL_DAEMON_BIN:-${ROOT_DIR}/target/release/coral-daemon}"

cd "${ROOT_DIR}"
./scripts/setup-rootfs.sh >/dev/null

if [[ ! -x "${CORAL_BIN}" || ! -x "${CORAL_DAEMON_BIN}" ]]; then
    echo "[bench] Building release binaries (cargo build --release --workspace)..."
    cargo build --release --workspace
fi

python3 - "${ROOT_DIR}" "${RUNS}" "${WARMUP}" "${PULL_EXTRA}" "${CORAL_BIN}" "${CORAL_DAEMON_BIN}" <<'PY'
import json
import os
import platform
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time

root_dir = sys.argv[1]
runs = int(sys.argv[2])
warmup = int(sys.argv[3])
pull_extra = sys.argv[4] == "1"
coral_bin = sys.argv[5]
daemon_bin = sys.argv[6]
alpine_rootfs = os.path.join(root_dir, "rootfs")

def percentile(sorted_vals, pct):
    if not sorted_vals:
        return 0.0
    k = (len(sorted_vals) - 1) * (pct / 100.0)
    f = int(k)
    c = min(f + 1, len(sorted_vals) - 1)
    if f == c:
        return sorted_vals[f]
    return sorted_vals[f] + (sorted_vals[c] - sorted_vals[f]) * (k - f)

def bench(label, make_cmd, n_runs, n_warmup, expected="BENCH_OK"):
    for i in range(n_warmup):
        cmd = make_cmd(f"w{i}")
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if res.returncode != 0 or expected not in res.stdout:
            raise RuntimeError(f"{label} warmup failed (rc={res.returncode}): {res.stdout} {res.stderr}")

    samples_ms = []
    for i in range(n_runs):
        cmd = make_cmd(f"r{i}")
        t0 = time.perf_counter_ns()
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        t1 = time.perf_counter_ns()
        if res.returncode != 0 or expected not in res.stdout:
            raise RuntimeError(f"{label} run {i} failed (rc={res.returncode}): {res.stdout} {res.stderr}")
        samples_ms.append((t1 - t0) / 1e6)

    samples_ms.sort()
    mean_ms = statistics.mean(samples_ms)
    return {
        "label": label,
        "runs": n_runs,
        "min": samples_ms[0],
        "p50": percentile(samples_ms, 50),
        "mean": mean_ms,
        "p95": percentile(samples_ms, 95),
        "max": samples_ms[-1],
        "cps": 1000.0 / mean_ms if mean_ms > 0 else 0.0,
    }

def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

def read_rss_mb(pid):
    try:
        with open(f"/proc/{pid}/status", "r") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    kb = int(line.split()[1])
                    return kb / 1024.0
    except OSError:
        pass
    return None

def file_mb(path):
    return os.path.getsize(path) / (1024.0 * 1024.0)

with tempfile.TemporaryDirectory(prefix="coral-bench-") as tmp:
    state_root = os.path.join(tmp, "state")
    images_dir = os.path.join(tmp, "images")
    bundle_dir = os.path.join(tmp, "bundle")
    daemon_state = os.path.join(tmp, "daemon-state")
    os.makedirs(bundle_dir, exist_ok=True)

    # Create minimal OCI bundle config.json for Alpine
    subprocess.run(
        [coral_bin, "spec", "--rootless", "--rootfs", alpine_rootfs, "--output", os.path.join(bundle_dir, "config.json")],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    cfg_path = os.path.join(bundle_dir, "config.json")
    with open(cfg_path, "r") as f:
        spec = json.load(f)
    spec["process"]["args"] = ["/bin/sh", "-c", "echo BENCH_OK"]
    with open(cfg_path, "w") as f:
        json.dump(spec, f, indent=2)

    # Register local Alpine rootfs into benchmark images_dir
    subprocess.run(
        [coral_bin, "--images-dir", images_dir, "pull", "alpine:3.20", "--local", "--rootfs", alpine_rootfs],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    if pull_extra:
        for extra_img in ["busybox:musl", "debian:bookworm-slim"]:
            print(f"[bench] Pulling {extra_img}...", file=sys.stderr)
            subprocess.run(
                [coral_bin, "--images-dir", images_dir, "pull", extra_img, "--local"],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

    results = []

    # 1. Coral OCI direct (`coral run --bundle`)
    results.append(
        bench(
            "Coral OCI direct (--bundle, alpine:3.20)",
            lambda tag: [coral_bin, "--root", state_root, "run", "--bundle", bundle_dir, f"oci-{tag}"],
            runs,
            warmup,
        )
    )

    # 2. Coral CLI image mode (`coral run --image alpine:3.20`)
    results.append(
        bench(
            "Coral CLI (--image alpine:3.20)",
            lambda tag: [
                coral_bin, "--root", state_root, "--images-dir", images_dir,
                "run", "--image", "alpine:3.20", "--name", f"img-{tag}",
                "--", "/bin/sh", "-c", "echo BENCH_OK",
            ],
            runs,
            warmup,
        )
    )

    if pull_extra:
        for img_ref in ["busybox:musl", "debian:bookworm-slim"]:
            results.append(
                bench(
                    f"Coral CLI (--image {img_ref})",
                    lambda tag, r=img_ref: [
                        coral_bin, "--root", state_root, "--images-dir", images_dir,
                        "run", "--image", r, "--name", f"x-{tag}",
                        "--", "/bin/sh", "-c", "echo BENCH_OK",
                    ],
                    runs,
                    warmup,
                )
            )

    # 3. Coral CRI v1 Daemon (`coral-daemon` + `coral run --daemon`)
    port = free_port()
    addr = f"127.0.0.1:{port}"
    daemon_proc = subprocess.Popen(
        [
            daemon_bin,
            "--listen", addr,
            "--state-dir", daemon_state,
            "--images-dir", images_dir,
            "--coral-bin", coral_bin,
            "--default-rootfs", alpine_rootfs,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    daemon_rss_idle = None
    daemon_rss_after = None
    try:
        # Wait for daemon readiness
        for _ in range(50):
            chk = subprocess.run(
                [coral_bin, "--addr", f"http://{addr}", "daemon-version"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            if chk.returncode == 0:
                break
            time.sleep(0.05)
        daemon_rss_idle = read_rss_mb(daemon_proc.pid)

        results.append(
            bench(
                "Coral CRI Daemon (gRPC + alpine:3.20)",
                lambda tag: [
                    coral_bin, "--addr", f"http://{addr}",
                    "run", "--daemon", "--image", "alpine:3.20", "--name", f"cri-{tag}",
                    "--", "/bin/sh", "-c", "echo BENCH_OK",
                ],
                min(runs, 20),
                min(warmup, 2),
            )
        )
        daemon_rss_after = read_rss_mb(daemon_proc.pid)
    finally:
        daemon_proc.terminate()
        try:
            daemon_proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            daemon_proc.kill()

    # 4. runc (Docker's low-level OCI runtime, if installed or RUNC_BIN is set)
    runc_bin = os.environ.get("RUNC_BIN") or shutil.which("runc")
    if runc_bin and os.access(runc_bin, os.X_OK):
        runc_bundle = os.path.join(tmp, "runc-bundle")
        runc_state = os.path.join(tmp, "runc-state")
        os.makedirs(runc_bundle, exist_ok=True)
        if subprocess.run([runc_bin, "spec", "--rootless"], cwd=runc_bundle, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            rcfg = os.path.join(runc_bundle, "config.json")
            with open(rcfg, "r") as f:
                rspec = json.load(f)
            rspec["root"]["path"] = alpine_rootfs
            rspec["process"]["terminal"] = False
            rspec["process"]["args"] = ["/bin/sh", "-c", "echo BENCH_OK"]
            with open(rcfg, "w") as f:
                json.dump(rspec, f, indent=2)
            results.append(
                bench(
                    "runc (Docker OCI runtime, rootless)",
                    lambda tag: [runc_bin, "--root", runc_state, "run", "--bundle", runc_bundle, f"runc-{tag}"],
                    runs,
                    warmup,
                )
            )

    # 5. Docker (if installed and daemon reachable)
    docker_bin = shutil.which("docker")
    docker_available = False
    if docker_bin:
        info = subprocess.run([docker_bin, "info"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if info.returncode == 0:
            docker_available = True
            subprocess.run([docker_bin, "pull", "-q", "alpine:3.20"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            results.append(
                bench(
                    "Docker (--rm --network=none alpine:3.20)",
                    lambda _tag: [docker_bin, "run", "--rm", "--network=none", "alpine:3.20", "/bin/sh", "-c", "echo BENCH_OK"],
                    min(runs, 20),
                    min(warmup, 2),
                )
            )
            results.append(
                bench(
                    "Docker (--rm default bridge alpine:3.20)",
                    lambda _tag: [docker_bin, "run", "--rm", "alpine:3.20", "/bin/sh", "-c", "echo BENCH_OK"],
                    min(runs, 20),
                    min(warmup, 2),
                )
            )

    print()
    print(f"Host:   {platform.system()} {platform.release()} ({platform.machine()}, {os.cpu_count()} CPUs)")
    print(f"Binary: coral={file_mb(coral_bin):.2f} MB | coral-daemon={file_mb(daemon_bin):.2f} MB")
    if daemon_rss_idle is not None and daemon_rss_after is not None:
        print(f"Daemon: idle RSS={daemon_rss_idle:.1f} MB | post-bench RSS={daemon_rss_after:.1f} MB")
    print(f"Docker: {'detected & benchmarked' if docker_available else 'not installed on this host (skipped)'}")
    print()
    print(f"{'WORKLOAD':<44} {'RUNS':>4} {'MIN':>8} {'P50':>8} {'MEAN':>8} {'P95':>8} {'MAX':>8} {'CTR/S':>8}")
    print("-" * 104)
    for r in results:
        print(
            f"{r['label']:<44} {r['runs']:>4} "
            f"{r['min']:>6.2f}ms {r['p50']:>6.2f}ms {r['mean']:>6.2f}ms "
            f"{r['p95']:>6.2f}ms {r['max']:>6.2f}ms {r['cps']:>7.1f}"
        )
PY

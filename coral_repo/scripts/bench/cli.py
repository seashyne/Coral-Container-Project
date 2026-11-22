"""Command-line entry point of the Coral vs Docker startup benchmark."""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time

from .hostenv import apply_affinity, parse_cpu_list, pin_docker_daemon
from .report import print_report, write_json
from .stats import free_port, loadavg, read_rss_mb
from .suite import (
    build_runc_bundle,
    coral_workloads,
    docker_workloads,
    merge_results,
    prepare_oci_bundle,
    register_images,
    run_rounds,
)
from .workload import Workload


def parse_args(argv=None):
    """Parse the command-line arguments of the benchmark harness."""
    ap = argparse.ArgumentParser(
        description="Coral vs Docker container-startup benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("--runs", type=int, default=30, help="measured runs per workload per round")
    ap.add_argument("--warmup", type=int, default=3, help="unmeasured warmup runs per workload")
    ap.add_argument("--rounds", type=int, default=3, help="repeats of the whole suite")
    ap.add_argument("--modes", default="cold,warm", help="cache modes to measure (cold, warm)")
    ap.add_argument("--cpus", default="", help="CPU set to pin to (e.g. 0-7); default: all")
    ap.add_argument("--max-load", type=float, default=0.0, help="max 1-min load before a round starts")
    ap.add_argument("--pull-extra", action="store_true", help="also benchmark busybox + debian images")
    ap.add_argument("--only", default="", help="comma-separated workload keys to measure")
    ap.add_argument("--coral-bin", required=True)
    ap.add_argument("--daemon-bin", required=True)
    ap.add_argument("--rootfs", required=True, help="Alpine rootfs directory")
    ap.add_argument("--json", dest="json_out", default="", help="write the summary as JSON")
    return ap.parse_args(argv)


def docker_cache_paths(docker_bin):
    """Files whose residency decides how warm a Docker run is."""
    cache = []
    if not docker_bin:
        return cache
    cache.append(docker_bin)
    docker_root = os.path.expanduser("~/.local/share/docker")
    snapshots = os.path.join(
        docker_root, "containerd", "daemon", "io.containerd.snapshotter.v1.overlayfs", "snapshots"
    )
    if os.path.isdir(snapshots):
        cache.append(snapshots)
    runtime_dir = os.path.expanduser("~/.local/opt/docker-rootless/usr/bin")
    if os.path.isdir(runtime_dir):
        cache.append(runtime_dir)
    return cache


def wait_for_daemon(coral_bin, addr, attempts=50):
    """Block until the freshly spawned daemon answers `daemon-version`."""
    for _ in range(attempts):
        chk = subprocess.run([coral_bin, "--addr", f"http://{addr}", "daemon-version"],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if chk.returncode == 0:
            return True
        time.sleep(0.05)
    return False


def main(argv=None):
    """Run the benchmark and print the results."""
    args = parse_args(argv)

    modes = [m.strip() for m in args.modes.split(",") if m.strip()]
    for mode in modes:
        if mode not in ("cold", "warm"):
            raise SystemExit(f"unknown cache mode: {mode}")

    available = sorted(os.sched_getaffinity(0))
    cpus = parse_cpu_list(args.cpus, available) if args.cpus else available
    if not cpus:
        raise SystemExit(f"no usable CPUs in {args.cpus!r} (available: {available})")
    mask = apply_affinity(cpus)
    pinned_docker = pin_docker_daemon(cpus)

    coral_bin = os.path.abspath(args.coral_bin)
    daemon_bin = os.path.abspath(args.daemon_bin)
    alpine_rootfs = os.path.abspath(args.rootfs)

    # Files whose residency decides how warm a run is.
    coral_cache = [coral_bin, daemon_bin, alpine_rootfs]
    docker_bin = shutil.which("docker")
    runc_bin = os.environ.get("RUNC_BIN") or shutil.which("runc")
    docker_cache = docker_cache_paths(docker_bin)

    summary = {
        "host": {
            "kernel": platform.release(),
            "machine": platform.machine(),
            "cpus": len(available),
            "affinity": sorted(mask),
            "pinned_docker": pinned_docker,
            "load_before": loadavg(),
        },
        "binaries": {"coral": coral_bin, "daemon": daemon_bin},
        "runs": args.runs,
        "rounds": args.rounds,
        "modes": modes,
        "results": [],
    }

    with tempfile.TemporaryDirectory(prefix="coral-bench-") as tmp:
        state_root = os.path.join(tmp, "state")
        images_dir = os.path.join(tmp, "images")
        bundle_dir = os.path.join(tmp, "bundle")
        daemon_state = os.path.join(tmp, "daemon-state")
        os.makedirs(bundle_dir, exist_ok=True)

        prepare_oci_bundle(coral_bin, bundle_dir, alpine_rootfs)
        register_images(coral_bin, images_dir, alpine_rootfs, args.pull_extra)

        workloads = coral_workloads(
            coral_bin, state_root, images_dir, bundle_dir, coral_cache, args.pull_extra
        )

        # runc: Docker's own OCI runtime, measured on the same rootfs.
        runc_bundle = build_runc_bundle(runc_bin, tmp, alpine_rootfs)
        if runc_bundle:
            runc_state = os.path.join(tmp, "runc-state")
            workloads.append(
                _runc_workload(runc_bin, runc_state, runc_bundle, alpine_rootfs)
            )

        workloads.extend(docker_workloads(docker_bin, docker_cache))

        # CRI daemon: the daemon process is started by this harness, so it
        # inherits the CPU affinity. Its own text pages stay mapped, so the
        # "cold" mode can only make the container image cold for this path.
        port = free_port()
        addr = f"127.0.0.1:{port}"
        daemon_log = os.environ.get("DAEMON_LOG", "")
        daemon_out = open(daemon_log, "ab") if daemon_log else subprocess.DEVNULL
        daemon_proc = subprocess.Popen(
            [daemon_bin, "--listen", addr, "--state-dir", daemon_state, "--images-dir", images_dir,
             "--coral-bin", coral_bin, "--default-rootfs", alpine_rootfs],
            stdout=daemon_out, stderr=subprocess.DEVNULL if not daemon_log else daemon_out,
        )
        if daemon_log:
            daemon_out.close()
        try:
            wait_for_daemon(coral_bin, addr)
            summary["daemon_rss_idle"] = read_rss_mb(daemon_proc.pid)
            workloads.append(_cri_workload(coral_bin, addr, coral_cache))

            only = {k.strip() for k in args.only.split(",") if k.strip()}
            if only:
                workloads = [w for w in workloads if w.key in only]

            round_data = run_rounds(
                workloads, modes, args.runs, args.warmup, args.rounds, args.max_load
            )
            summary["daemon_rss_after"] = read_rss_mb(daemon_proc.pid)
        finally:
            daemon_proc.terminate()
            try:
                daemon_proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                daemon_proc.kill()

        summary["results"] = merge_results(workloads, modes, round_data)
        summary["rounds_detail"] = round_data
        summary["load_after"] = loadavg()

    print_report(summary, coral_bin, daemon_bin, args)
    if args.json_out:
        write_json(summary, args.json_out)


def _runc_workload(runc_bin, runc_state, runc_bundle, alpine_rootfs):
    """Build the runc workload entry."""
    return Workload(
        "runc",
        "runc (Docker OCI runtime, rootless)",
        lambda tag: [runc_bin, "--root", runc_state, "run", "--bundle", runc_bundle, f"runc-{tag}"],
        [runc_bin, alpine_rootfs],
    )


def _cri_workload(coral_bin, addr, coral_cache):
    """Build the CRI daemon workload entry."""
    return Workload(
        "coral-cri",
        "Coral CRI Daemon (gRPC + alpine:3.20)",
        lambda tag: [coral_bin, "--addr", f"http://{addr}", "run", "--daemon",
                     "--image", "alpine:3.20", "--name", f"cri-{tag}",
                     "--", "/bin/sh", "-c", "echo BENCH_OK"],
        coral_cache,
        note="daemon stays resident: only the image is cold",
    )


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)

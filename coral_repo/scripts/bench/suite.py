"""Construction of the measured workloads and execution of the rounds."""

from __future__ import annotations

import json
import os
import statistics
import subprocess
import sys

from .hostenv import wait_for_idle
from .stats import loadavg, percentiles
from .workload import Workload, measure


def prepare_oci_bundle(coral_bin, bundle_dir, alpine_rootfs):
    """Write a minimal OCI bundle that echoes the benchmark marker."""
    cfg_path = os.path.join(bundle_dir, "config.json")
    subprocess.run(
        [coral_bin, "spec", "--rootless", "--rootfs", alpine_rootfs, "--output", cfg_path],
        check=True, stdout=subprocess.DEVNULL,
    )
    with open(cfg_path) as f:
        spec = json.load(f)
    spec["process"]["args"] = ["/bin/sh", "-c", "echo BENCH_OK"]
    with open(cfg_path, "w") as f:
        json.dump(spec, f, indent=2)
    return cfg_path


def register_images(coral_bin, images_dir, alpine_rootfs, pull_extra):
    """Register the local Alpine rootfs (and optionally extra distros) locally."""
    subprocess.run(
        [coral_bin, "--images-dir", images_dir, "pull", "alpine:3.20", "--local",
         "--rootfs", alpine_rootfs],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    if pull_extra:
        for extra in ("busybox:musl", "debian:bookworm-slim"):
            subprocess.run(
                [coral_bin, "--images-dir", images_dir, "pull", extra, "--local"],
                check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )


def build_runc_bundle(runc_bin, tmp, alpine_rootfs):
    """Prepare a runc bundle on the same rootfs, or return None when unusable."""
    if not runc_bin or not os.access(runc_bin, os.X_OK):
        return None

    runc_bundle = os.path.join(tmp, "runc-bundle")
    os.makedirs(runc_bundle, exist_ok=True)
    if subprocess.run([runc_bin, "spec", "--rootless"], cwd=runc_bundle,
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0:
        return None

    rcfg = os.path.join(runc_bundle, "config.json")
    with open(rcfg) as f:
        rspec = json.load(f)
    rspec["root"]["path"] = alpine_rootfs
    rspec["process"]["terminal"] = False
    rspec["process"]["args"] = ["/bin/sh", "-c", "echo BENCH_OK"]
    with open(rcfg, "w") as f:
        json.dump(rspec, f, indent=2)
    return runc_bundle


def coral_workloads(coral_bin, state_root, images_dir, bundle_dir, coral_cache, pull_extra):
    """The Coral workloads: direct OCI run, image-mode CLI, extra distros."""
    workloads = [
        Workload(
            "coral-oci",
            "Coral OCI direct (--bundle, alpine:3.20)",
            lambda tag: [coral_bin, "--root", state_root, "run", "--bundle", bundle_dir, f"oci-{tag}"],
            coral_cache,
        ),
        Workload(
            "coral-cli",
            "Coral CLI (--image alpine:3.20)",
            lambda tag: [coral_bin, "--root", state_root, "--images-dir", images_dir,
                         "run", "--image", "alpine:3.20", "--name", f"img-{tag}",
                         "--", "/bin/sh", "-c", "echo BENCH_OK"],
            coral_cache,
        ),
    ]
    if pull_extra:
        for img_ref in ("busybox:musl", "debian:bookworm-slim"):
            workloads.append(
                Workload(
                    f"coral-{img_ref.split(':')[0]}",
                    f"Coral CLI (--image {img_ref})",
                    lambda tag, r=img_ref: [coral_bin, "--root", state_root, "--images-dir", images_dir,
                                            "run", "--image", r, "--name", f"x-{tag}",
                                            "--", "/bin/sh", "-c", "echo BENCH_OK"],
                    coral_cache,
                )
            )
    return workloads


def docker_available(docker_bin):
    """Return True when the Docker daemon behind `docker_bin` actually answers."""
    if not docker_bin:
        return False
    probe = subprocess.run([docker_bin, "info", "--format", "{{.ServerVersion}}"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return probe.returncode == 0


def docker_workloads(docker_bin, docker_cache):
    """The Docker Engine workloads: no-network and default bridge.

    Skipped entirely when the daemon is unreachable, so the harness can still
    measure Coral on a host where Docker is installed but not running.
    """
    if not docker_available(docker_bin):
        print("[bench] Docker daemon is not reachable — skipping the Docker workloads",
              file=sys.stderr)
        return []
    subprocess.run([docker_bin, "pull", "-q", "alpine:3.20"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return [
        Workload(
            "docker-none",
            "Docker Engine (--rm --network=none)",
            lambda tag: [docker_bin, "run", "--rm", "--network=none", "alpine:3.20",
                         "/bin/sh", "-c", "echo BENCH_OK"],
            docker_cache,
        ),
        Workload(
            "docker-bridge",
            "Docker Engine (--rm default bridge)",
            lambda tag: [docker_bin, "run", "--rm", "alpine:3.20",
                         "/bin/sh", "-c", "echo BENCH_OK"],
            docker_cache,
        ),
    ]


def run_rounds(workloads, modes, runs, warmup, rounds, max_load):
    """Measure every workload in every mode for `rounds` rounds."""
    round_data = []
    for round_index in range(rounds):
        if max_load > 0:
            wait_for_idle(max_load, timeout_s=60)
        load_start = loadavg()
        round_rows = []
        for mode in modes:
            cold = mode == "cold"
            for workload in workloads:
                stats = measure(workload, runs, warmup, cold)
                stats.update({"key": workload.key, "label": workload.label, "mode": mode,
                              "round": round_index + 1, "note": workload.note})
                round_rows.append(stats)
        round_data.append({"round": round_index + 1, "load_start": load_start,
                           "load_end": loadavg(), "rows": round_rows})
        print(f"[bench] round {round_index + 1}/{rounds} done "
              f"(loadavg {load_start:.2f} -> {loadavg():.2f})", file=sys.stderr)
    return round_data


def merge_results(workloads, modes, round_data):
    """Collapse the per-round rows into one median-of-medians row per workload."""
    merged = []
    for workload in workloads:
        for mode in modes:
            samples = [r for rd in round_data for r in rd["rows"]
                       if r["key"] == workload.key and r["mode"] == mode]
            if not samples:
                continue
            p50s = sorted(r["p50"] for r in samples)
            _, p50, _, _, _ = percentiles(p50s)
            failures = [f for r in samples for f in r.get("failures", [])]
            merged.append({
                "key": workload.key,
                "label": workload.label,
                "mode": mode,
                "rounds": len(samples),
                "p50_median": p50,
                "p50_min": p50s[0],
                "p50_max": p50s[-1],
                "p50_spread": p50s[-1] - p50s[0],
                "mean_median": statistics.median(r["mean"] for r in samples),
                "min_median": statistics.median(r["min"] for r in samples),
                "p95_median": statistics.median(r["p95"] for r in samples),
                "cps_median": 1000.0 / statistics.median(r["mean"] for r in samples)
                if statistics.median(r["mean"] for r in samples) > 0 else 0.0,
                "note": workload.note,
                "failures": failures,
            })
    return merged

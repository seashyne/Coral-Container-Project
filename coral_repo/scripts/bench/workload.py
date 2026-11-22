"""One measured workload and the loop that times it."""

from __future__ import annotations

import subprocess
import sys
import time

from .hostenv import evict_cache
from .stats import percentiles


class Workload:
    """One measured workload plus the files that must be evicted for it."""

    def __init__(self, key, label, make_cmd, evict_paths, note=""):
        self.key = key
        self.label = label
        self.make_cmd = make_cmd
        self.evict_paths = evict_paths
        self.note = note


def measure(workload, runs, warmup, cold, expected="BENCH_OK", retries=2):
    """Run one workload `runs` times, returning latency statistics in ms.

    A failed run is retried (the daemon occasionally loses a container's state
    file under heavy churn); every failure is counted so it can be reported
    instead of silently skewing the sample.
    """
    evicted = 0
    failures = []

    def run_once(tag):
        cmd = workload.make_cmd(tag)
        t0 = time.perf_counter_ns()
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        t1 = time.perf_counter_ns()
        return res, (t1 - t0) / 1e6

    for i in range(warmup):
        if cold:
            evicted += evict_cache(workload.evict_paths)
        res, _ = run_once(f"w{i}")
        if res.returncode != 0 or expected not in res.stdout:
            raise RuntimeError(
                f"{workload.label} warmup failed (rc={res.returncode}): {res.stdout} {res.stderr}"
            )

    samples = []
    for i in range(runs):
        attempt = 0
        while True:
            if cold:
                evicted += evict_cache(workload.evict_paths)
            res, elapsed = run_once(f"r{i}")
            if res.returncode == 0 and expected in res.stdout:
                samples.append(elapsed)
                break
            attempt += 1
            failures.append(f"{workload.label}: {res.stderr.strip()[:200]}")
            if attempt > retries:
                raise RuntimeError(
                    f"{workload.label} run {i} failed after {attempt} attempts "
                    f"(rc={res.returncode}): {res.stdout} {res.stderr}"
                )
            print(f"[bench] retry {attempt}/{retries} after failed {workload.key} run {i}",
                  file=sys.stderr)

    samples.sort()
    lo, p50, mean, p95, hi = percentiles(samples)
    return {
        "runs": runs,
        "min": lo,
        "p50": p50,
        "mean": mean,
        "p95": p95,
        "max": hi,
        "cps": 1000.0 / mean if mean > 0 else 0.0,
        "evicted_objects": evicted,
        "failures": failures,
    }

"""Human-readable and JSON rendering of a benchmark run."""

from __future__ import annotations

import json
import platform

from .stats import file_mb


def print_report(summary, coral_bin, daemon_bin, args):
    """Print the headline table of a finished benchmark run."""
    host = summary["host"]
    print()
    print(f"Host:    {platform.system()} {host['kernel']} ({host['machine']}, {host['cpus']} CPUs)")
    print(f"Binary:  coral={file_mb(coral_bin):.2f} MB | coral-daemon={file_mb(daemon_bin):.2f} MB")
    print(f"CPUs:    affinity={host['affinity']} | docker pinned: {host['pinned_docker'] or 'n/a'}")
    print(f"Load:    before={host['load_before']:.2f} after={summary['load_after']:.2f} "
          f"(max-load gate={args.max_load or 'off'})")
    print(f"Rounds:  {args.rounds} | runs/round={args.runs} | warmup={args.warmup} | modes={args.modes}")
    print("Cache:   cold = runtime binaries + image layers evicted via posix_fadvise (no root)")

    idle = summary.get("daemon_rss_idle")
    after = summary.get("daemon_rss_after")
    if idle is not None and after is not None:
        print(f"Daemon:  idle RSS={idle:.1f} MB | post-bench RSS={after:.1f} MB")

    print()
    print(f"{'WORKLOAD':<42} {'MODE':<6} {'P50':>9} {'ROUND P50 RANGE':>17} {'MEAN':>9} {'P95':>9} {'CTR/S':>8}")
    print("-" * 108)
    for row in summary["results"]:
        spread = f"{row['p50_min']:.2f}-{row['p50_max']:.2f}"
        print(f"{row['label']:<42} {row['mode']:<6} {row['p50_median']:>7.2f}ms {spread:>17} "
              f"{row['mean_median']:>8.2f}ms {row['p95_median']:>8.2f}ms {row['cps_median']:>7.1f}")

    failed = [(r["label"], r["mode"], r["failures"]) for r in summary["results"] if r.get("failures")]
    if failed:
        print()
        print("Retried failures (counted, not hidden):")
        for label, mode, errs in failed:
            print(f"  {label} [{mode}]: {len(errs)} failure(s)")
            for err in errs[:3]:
                print(f"    - {err}")


def write_json(summary, path):
    """Write the machine-readable summary to `path`."""
    with open(path, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\n[bench] JSON summary written to {path}")

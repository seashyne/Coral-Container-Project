"""Numeric helpers shared by the benchmark harness."""

from __future__ import annotations

import os
import socket
import statistics


def percentiles(sorted_vals):
    """Return (min, p50, mean, p95, max) for an already sorted sample list."""
    if not sorted_vals:
        return (0.0, 0.0, 0.0, 0.0, 0.0)

    def pct(p):
        k = (len(sorted_vals) - 1) * (p / 100.0)
        f = int(k)
        c = min(f + 1, len(sorted_vals) - 1)
        if f == c:
            return sorted_vals[f]
        return sorted_vals[f] + (sorted_vals[c] - sorted_vals[f]) * (k - f)

    return (sorted_vals[0], pct(50), statistics.mean(sorted_vals), pct(95), sorted_vals[-1])


def loadavg():
    """Return the 1-minute load average, or 0.0 when /proc is unavailable."""
    try:
        with open("/proc/loadavg", "r") as f:
            return float(f.read().split()[0])
    except OSError:
        return 0.0


def read_rss_mb(pid):
    """Return the resident set size of `pid` in MB, or None when unreadable."""
    try:
        with open(f"/proc/{pid}/status", "r") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) / 1024.0
    except OSError:
        pass
    return None


def file_mb(path):
    """Return the size of `path` in MB, or 0.0 when it cannot be read."""
    try:
        return os.path.getsize(path) / (1024.0 * 1024.0)
    except OSError:
        return 0.0


def free_port():
    """Return a currently unused TCP port on the loopback interface."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

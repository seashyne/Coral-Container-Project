#!/usr/bin/env python3
"""Coral vs Docker container-startup benchmark with controlled cache and load.

The harness measures startup latency for Coral (OCI direct, image mode, CRI
daemon), Docker's own `runc`, and Docker Engine, under two page-cache modes:

  cold  the runtime binaries and the container image files are evicted from
        the page cache before every measured run (posix_fadvise DONTNEED,
        no root required), so each run pays real disk I/O
  warm  the same workload measured again with everything already cached

Load is controlled by pinning the harness (and every child it spawns) to a
fixed CPU set, waiting for the machine to fall below a load threshold before
a round starts, and recording the load average across the round.

Every workload is measured over several rounds; the headline number is the
median of the per-round medians (median-of-medians) so that a single noisy
round cannot move the result.

The implementation lives in the `bench` package next to this file; this
script only exists so the harness can be started directly.

Usage:
  python3 scripts/benchmark.py --runs 30 --rounds 3 --cpus 0-15 \
      --coral-bin /usr/bin/coral --daemon-bin /usr/bin/coral-daemon
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from bench.cli import main  # noqa: E402

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)

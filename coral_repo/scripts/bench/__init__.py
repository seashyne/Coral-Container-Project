"""Benchmark harness for Coral vs Docker container-startup latency.

The package is split into small modules so that every file stays well below
the project's 400-line ceiling:

  stats     numeric helpers (percentiles, load average, RSS, file size)
  hostenv   CPU affinity, load gating and page-cache eviction
  workload  one measured workload and the measurement loop
  suite     workload construction, round execution and result merging
  report    human-readable and JSON output
  cli       argument parsing and entry point
"""

from __future__ import annotations

__all__ = ["cli", "hostenv", "report", "stats", "suite", "workload"]

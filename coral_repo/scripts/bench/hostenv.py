"""Host-level controls for a fair benchmark: CPU pinning, load gating, cache eviction.

These helpers make the measurement comparable across runs and across
runtimes. Everything here works as an unprivileged user.
"""

from __future__ import annotations

import os
import subprocess
import time

from .stats import loadavg


def parse_cpu_list(spec, available):
    """Turn "0-3,8" into a sorted list of CPU ids, clipped to `available`."""
    cpus = set()
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            lo, hi = (int(x) for x in part.split("-", 1))
            cpus.update(range(lo, hi + 1))
        else:
            cpus.add(int(part))
    return sorted(cpus & set(available))


def apply_affinity(cpus):
    """Pin this process (and its children) to `cpus`. Returns the mask."""
    mask = set(cpus)
    try:
        os.sched_setaffinity(0, mask)
    except OSError:
        pass
    return mask


def pin_docker_daemon(cpus):
    """Best-effort: pin an already running dockerd/containerd to the same CPUs.

    Docker's daemon is started outside this harness, so it does not inherit
    our affinity. Without this the comparison would let Docker use a
    different set of cores than Coral.
    """
    pinned = []
    for name in ("dockerd", "containerd"):
        try:
            out = subprocess.run(
                ["pgrep", "-x", name], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True
            ).stdout
        except OSError:
            continue
        for pid in out.split():
            try:
                os.sched_setaffinity(int(pid), set(cpus))
                pinned.append(f"{name}:{pid}")
            except (OSError, ValueError):
                continue
    return pinned


def wait_for_idle(max_load, timeout_s):
    """Block until the 1-minute load average drops below `max_load`."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        current = loadavg()
        if current <= max_load:
            return current
        time.sleep(2.0)
    return loadavg()


def evict_cache(paths):
    """Evict the given files/directories from the page cache.

    Uses posix_fadvise(POSIX_FADV_DONTNEED) which only needs read access, so
    no root privileges are required. Returns the number of objects touched.
    """
    targets = []
    for path in paths:
        if not path:
            continue
        if os.path.isdir(path):
            for root, _dirs, names in os.walk(path):
                targets.append(root)
                targets.extend(os.path.join(root, n) for n in names)
        elif os.path.exists(path):
            targets.append(path)

    evicted = 0
    for target in targets:
        try:
            fd = os.open(target, os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
            evicted += 1
        except OSError:
            pass
        finally:
            os.close(fd)
    return evicted

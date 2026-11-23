/**
 * Coral Container Project — English (en) dictionary, part 2
 * * * Platform integration & deep Docker-parity features through v2.8.1.
 * * * Loaded before i18n.js, which merges this into window.CORAL_I18N.
 */
window.CORAL_I18N_EN_FEATURES = {
  "nav.features": "Platform Features",
  "feat2.title": "Platform & Deep Docker-Parity Features (v2.8.1)",
  "feat2.lead":
    "Pure-Rust platform capabilities shipped through v2.8.1 — zero external daemon required and backed by automated unit & E2E suites.",
  "feat2.cache.title": "Zero-Copy Shared Layer Store & Cache",
  "feat2.cache.desc":
    "coral pull deduplicates compressed blobs in .layer-cache/sha256/ (4-stream parallel download with double SHA-256 verification) and extracts each layer once into .layers/sha256/, materializing image rootfs via zero-copy hardlinks and multi-lowerdir OverlayFS.",
  "feat2.dns.title": "Rootless L2/L3 Virtual Switch, NAT & DNS",
  "feat2.dns.desc":
    "Custom networks run a pure-Rust RootlessVSwitch over unprivileged /dev/net/tun (IFF_TAP) assigning real 10.89.x.Y private IPs across isolated netns without -p — complete with an embedded 10.89.x.1 gateway answering ARP, ICMP, multi-IP round-robin + upstream-forwarding RFC 1035 DNS (10.89.x.1:53), and userspace UDP/TCP outbound NAT.",
  "feat2.buildkit.title": "BuildKit/Buildx Pro & Parallel Multi-Stage DAG",
  "feat2.buildkit.desc":
    "Supports .dockerignore filtering, RUN <<EOF heredocs, RUN --mount=type=cache|secret|tmpfs|bind|ssh, parallel multi-stage topological waves with dead-stage elimination, --platform + QEMU binfmt checks, --cache-from, and multi-arch OCI Image Index publishing (--platform linux/amd64,linux/arm64 --push).",
  "feat2.dockersock.title": "Streaming docker.sock Engine API",
  "feat2.dockersock.desc":
    "Serves Docker Engine REST API (/v1.24–/v1.47) on Unix socket & TCP with HTTP 101 UPGRADED hijack for /exec/{id}/start, chunked live logs?follow=1 & broadcast /events, PUT/GET /containers/{id}/archive, POST /build, /networks, /volumes, and live /pause, /rename, /update, /top.",
  "feat2.gpu.title": "CVE Scanner, NVIDIA CDI/.so, Compose v2 & Observability",
  "feat2.gpu.desc":
    "Built-in coral scan (SPDX SBOM + OSV.dev batch API with --fail-on CI gates), coral top / wait / stats --all, CNCF CDI JSON + host NVIDIA .so/nvidia-smi mounts, Compose v2 (secrets, configs, include, --scale, watch), live pause/rename/update, and coral machine (Linux/Lima VirtioFS/WSL2).",
  "feat2.usageTitle": "How to use them",
  "feat2.usageLead": "Key CLI commands for each v2.8.1 capability",
  "feat2.caption": "Example commands for the v2.8.1 platform features",
  "feat2.colCapability": "Capability",
  "feat2.colUsage": "Usage",
  "feat2.rowCache": "Zero-copy shared layer store",
  "feat2.rowDns": "Rootless L2/L3 vswitch & DNS",
  "feat2.rowBuildkit": "Multi-arch Buildx & .dockerignore",
  "feat2.rowDocker": "Streaming Docker Engine API",
  "feat2.rowGpu": "NVIDIA CDI / .so & devices",
  "feat2.rowAlias": "CVE scan, Observability & Compose v2",
  "feat2.descCache": "shares inodes from .layers/sha256/ with automatic multi-lowerdir OverlayFS",
  "feat2.descDns": "assigns 10.89.x.Y IPs + 10.89.x.1:53 Gateway DNS (Round-Robin + Upstream) & NAT",
  "feat2.descBuildkit": "parallel DAG + .dockerignore + --mount=type=cache|secret|bind|ssh",
  "feat2.descDocker": "supports Upgrade: tcp hijack, chunked logs/events, /archive, /build & /top",
  "feat2.descGpu": "resolves CNCF CDI + mounts host libcuda.so* / nvidia-smi read-only",
  "feat2.descAlias": "OSV.dev CVE scan, coral top/wait/stats --all, pause/rename/update & compose --scale",
  "feat2.composeLead":
    "Use secrets, configs, include, deploy.replicas, gpus, devices, log_driver and aliases directly in coral-compose.yml or docker-compose.yml.",
};

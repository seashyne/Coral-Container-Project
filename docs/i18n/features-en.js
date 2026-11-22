/**
 * Coral Container Project — English (en) dictionary, part 2
 * * * Platform integration features introduced in v2.5.0.
 * * * Loaded before i18n.js, which merges this into window.CORAL_I18N.
 */
window.CORAL_I18N_EN_FEATURES = {
  "nav.features": "Platform Features",
  "feat2.title": "Platform Integration Features (v2.5.0)",
  "feat2.lead":
    "Five new capabilities ship in v2.5.0, all written in pure Rust with no extra external daemon, and all covered by the E2E suite.",
  "feat2.cache.title": "Content-Addressable Layer Cache",
  "feat2.cache.desc":
    "coral pull never re-downloads a layer it already has. Every blob is SHA-256 verified on download and again on every cache lookup — a blob that no longer matches is deleted automatically — and fresh layers are promoted atomically via rename. Up to 4 layers download concurrently, then are reassembled in manifest order.",
  "feat2.dns.title": "Embedded DNS Service Discovery",
  "feat2.dns.desc":
    "The daemon runs its own RFC 1035 DNS responder over UDP in-process. Container names, Compose service names and --network-alias values resolve both as plain names and as *.coral.local, while sibling names are also written to /etc/hosts and running containers are updated in place for bidirectional resolution.",
  "feat2.buildkit.title": "BuildKit Dockerfile Extensions",
  "feat2.buildkit.desc":
    "coral build understands heredocs (RUN <<EOF), RUN --mount=type=cache for a persistent host-side cache, type=secret mounted read-only from the build context, and type=tmpfs — plus the image metadata instructions EXPOSE, VOLUME, SHELL, HEALTHCHECK and ONBUILD, and multi-stage COPY --from=<stage|index|image>.",
  "feat2.dockersock.title": "docker.sock — Docker Engine API",
  "feat2.dockersock.desc":
    "coral-daemon serves the Docker Engine REST API (/v1.24 to /v1.47) over a Unix socket (default <state_dir>/docker.sock, mode 0660 — ~/.local/state/coral/daemon/docker.sock when run from the packaged systemd unit) and TCP, so Docker CLI, Testcontainers, Traefik and Portainer can drive Coral unmodified through DOCKER_HOST=unix://.",
  "feat2.gpu.title": "GPU & Device Passthrough",
  "feat2.gpu.desc":
    "Pass host hardware into a container with Docker-compatible --device, or let Coral discover GPUs automatically across NVIDIA, AMD ROCm (/dev/kfd) and DRM (/dev/dri) — together with --network-alias and --log-driver (raw, json-file, syslog, journald).",
  "feat2.usageTitle": "How to use them",
  "feat2.usageLead": "The key command for each capability",
  "feat2.caption": "Example commands for the v2.5.0 platform integration features",
  "feat2.colCapability": "Capability",
  "feat2.colUsage": "Usage",
  "feat2.rowCache": "Digest-addressed layer cache",
  "feat2.rowDns": "Service discovery on a network",
  "feat2.rowBuildkit": "BuildKit heredoc & mount",
  "feat2.rowDocker": "Docker Engine API",
  "feat2.rowGpu": "GPU and devices",
  "feat2.rowAlias": "Alias and log driver",
  "feat2.composeLead":
    "The gpus, devices, log_driver and aliases keys are available in Coral Compose files — both coral-compose.yml and docker-compose.yml.",
};

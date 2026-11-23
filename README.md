<p align="center">
  <img src="docs/assets/coral-icon.png" alt="Coral Container Project Logo" width="88" />
</p>

<h1 align="center">Coral Container Project (<code>v2.8.1</code>)</h1>

**Coral** is a next-generation, memory-safe container runtime written in **Rust (Edition 2024)** — uniting a rootless-first **OCI v1.0.2** Linux container engine (with Multi-UID `subuid`/`subgid`, `--device`, and `--gpus`), **Kubernetes CRI v1 gRPC** daemon + **Docker Engine REST API (`docker.sock`) compatibility shim**, **Embedded UDP DNS**, **BuildKit-style Dockerfile** (`RUN <<EOF`, `--mount=type=cache|secret`), **Shared Content-Addressable Layer Cache + Parallel Pull**, **WASI Serverless HTTP WebAssembly** runtime (`coral-wasm`), pure-Rust Linux **`/dev/kvm` Separate-Kernel MicroVM with VirtIO-Blk & VirtIO-Net** (`coral-vmm`, Zero QEMU/Firecracker), **Prometheus `/metrics`**, **Ed25519 Image Signing + SPDX 2.3 SBOM**, multi-arch **x86_64 & ARM64**, and the redesigned **Coral Desktop** GUI (Linux / Windows / macOS `.dmg`).

> **Online Docs & Visual Guides:** https://seashyne.github.io/Coral-Container-Project/
>
> **Evaluation & Docker Benchmark Report:** [REPORT_v2.3.md](coral_repo/docs/REPORT_v2.3.md) · [BENCHMARK.md](coral_repo/docs/BENCHMARK.md)
>
> **GitHub Releases (`.deb`, `.tar.gz`, Desktop installers):** https://github.com/seashyne/Coral-Container-Project/releases

![Coral Visual Quick Guide](docs/assets/coral-usage-guide.png)

---

## Highlights (`v2.8.1`)

- **Built-in CVE Vulnerability Scanner (`coral scan`)**: Extracts OS packages via SPDX SBOM (Alpine APK / Debian/Ubuntu DPKG) and queries **OSV.dev Batch API** with `--json`, `--offline`, and CI exit-gate `--fail-on critical|high|medium|low` — integrated into CLI, daemon, and Coral Desktop.
- **Rootless L2/L3 Virtual Switch (`10.89.x.0/24`) + Outbound NAT + Upstream & Round-Robin DNS (`10.89.x.1:53`)**: Pure-Rust `RootlessVSwitch` connects isolated `CLONE_NEWUSER | CLONE_NEWNET` containers on private `10.89.x.Y` IPs without root or `-p`, with a built-in `10.89.x.1` gateway answering ARP, ICMP Echo, userspace UDP/TCP outbound NAT, multi-IP round-robin DNS for `--scale` replicas, and upstream DNS forwarding.
- **Observability CLI & Native Coral Desktop `v2.8.1` Modals**: Adds `coral top <ctr>` (+ `GET /containers/{id}/top`), `coral wait <ctr>`, and `coral stats --all`, plus accessible native `<dialog>` modals (EN/TH) for Rename, Live Update Limits, Live Processes (`top`), and one-click CVE Scanning in Coral Desktop.
- **Streaming `docker.sock` Engine API (`Upgrade: tcp` Hijack, Chunked Logs/Events, `/archive`, `/build`)**: Full HTTP/1.1 `101 UPGRADED` hijack for `/exec/{id}/start`, chunked live `logs?follow=1` & broadcast `/events`, `PUT/GET /containers/{id}/archive` tar copy (Testcontainers), `POST /build` tar build API, plus `/pause`, `/unpause`, `/rename`, `/update`, and `/top`.
- **BuildKit & Buildx Pro (`.dockerignore`, Parallel DAG, Multi-Arch OCI Index Push, `bind`/`ssh` Mounts)**: Filters `COPY` & build cache hashes via `.dockerignore`, supports `RUN --mount=type=cache|secret|tmpfs|bind|ssh`, `--cache-from`, parallel multi-stage topological waves, and multi-arch `--platform linux/amd64,linux/arm64 --push` OCI Image Index publishing.
- **Docker Compose v2 Parity, Zero-Copy Layer Store & NVIDIA CDI/`.so` Injection**: Adds `secrets:`, `configs:`, `include:`, `deploy.replicas`/`--scale`, `coral compose watch`, `coral pause`/`unpause`/`rename`/`update`, `coral machine`, zero-copy `.layers/sha256/` hardlink + multi-lowerdir OverlayFS, and CNCF CDI + host NVIDIA `.so`/`nvidia-smi` mounts.
- **17.9×–26.8× Faster Startup than Docker on the Same Host**: `coral run --image alpine:3.20` starts rootlessly in **8.88 ms P50** vs Docker Engine **159.32–237.56 ms P50**, with `coral-daemon` idle RAM at **8.4 MB**, plus WASI WebAssembly (`329 µs`) and pure-Rust `/dev/kvm` MicroVMs (`963 µs`).

> Platform features introduced in `v2.4.0` and extended in `v2.5.0` are documented below.

![Coral v2.3.0 vs Docker Benchmark Chart](docs/assets/coral-vs-docker-benchmark.png)

---

## New in `v2.4.0` — Platform Integration Features

Five new capabilities ship on top of the OCI/CRI/Wasm/KVM core. Each one is implemented in
pure Rust with no external daemon and is covered by the E2E suite.

### 1. Content-Addressable Layer Cache

`coral pull` no longer re-downloads layers it already has. Verified blobs are stored
content-addressably by their OCI digest under `<images_dir>/.layer-cache/sha256/<hex>.blob`
and reused by every later image that references the same layer.

- **SHA-256 verified twice** — on download, and again on every cache lookup. A blob whose
  digest no longer matches is deleted automatically instead of being served.
- **Atomic promotion** — a freshly verified layer is `rename`d into the cache (with a `copy`
  fallback), so a partially written blob is never visible to a concurrent pull.
- **Parallel downloads** — up to **4 layers** download concurrently, then are reassembled in
  manifest order regardless of which finished first.

```bash
coral pull alpine:3.20          # first pull populates the cache
coral pull busybox:musl         # shared base layers are reused, not re-fetched
ls ~/.local/state/coral/images/.layer-cache/sha256/
```

### 2. Embedded DNS Service Discovery

The daemon runs its own **RFC 1035 UDP DNS responder** in-process (`coral-net`) on an
ephemeral loopback port. Container names, Compose service names and `--network-alias` values
are registered automatically, so containers on a user-defined network resolve each other by
name without any external DNS server.

- Every registered name also resolves as `<name>.coral.local`.
- `A` and `ANY` queries are answered; `AAAA` returns NODATA; unknown names return NXDOMAIN.
- Sibling names and aliases are additionally written into `/etc/hosts`, and **already-running
  siblings are updated in place** so resolution stays bidirectional.

```bash
coral network create app-net
coral run -d --network app-net --network-alias db --name postgres postgres:16
coral run --network app-net alpine:3.20 sh -c 'nslookup db'
```

### 3. BuildKit Dockerfile Extensions

`coral build` now understands the Dockerfile subset that modern Dockerfiles rely on.

- **Heredocs** — `RUN <<EOF`, `RUN <<-EOF` and `RUN --mount=... <<'EOF'`. The heredoc body is
  folded into a single command chain before the build runs.
- **`RUN --mount=type=cache`** — a persistent, host-side cache directory bind-mounted
  read-write into the build step, reused across builds (e.g. package manager caches).
- **`RUN --mount=type=secret`** — a file from the build context mounted **read-only** at
  `/run/secrets/<id>` (or an explicit `target=`). Secret sources are resolved inside the
  build context only, never from arbitrary host paths.
- **`RUN --mount=type=tmpfs`** — a `nosuid,nodev,mode=1777` tmpfs at the given target.
- **Image metadata instructions** — `EXPOSE`, `VOLUME`, `SHELL`, `HEALTHCHECK`
  (`--interval=`, `--retries=`, `CMD ...`, and `NONE` to clear) and `ONBUILD` are applied to
  the resulting OCI image config.
- **Multi-stage builds** — `COPY --from=<stage|index|image>` copies from an earlier build
  stage, a stage index, or another image, and referencing the stage currently being built is
  rejected.

```dockerfile
RUN --mount=type=cache,target=/root/.cache/pip \
    pip install --no-cache-dir -r requirements.txt
RUN --mount=type=secret,id=npmrc,target=/root/.npmrc \
    npm ci --production
```

### 4. `docker.sock` — Docker Engine API Compatibility

`coral-daemon` exposes a **Docker Engine REST API shim** (`/v1.24`..`/v1.47`) over a Unix
socket (default `<state_dir>/docker.sock`, mode `0660` — `~/.local/state/coral/daemon/docker.sock`
when run from the packaged systemd unit) and optionally over TCP. Existing Docker tooling can
drive Coral without modification.

- Supported endpoints: `/_ping`, `/version`, `/info`, `/images/json`, `/containers/json`,
  `/containers/create`, `/containers/{id}/json`, `/containers/{id}/logs`,
  `/containers/{id}/{start,stop,kill,restart,wait}`, `DELETE /containers/{id}`.
- `POST /containers/create` understands `Image`, `name`, `Hostname`, `Cmd`, `Env`,
  `Entrypoint`, `User`, `HostConfig.Binds`, `HostConfig.NetworkMode` and
  `HostConfig.Memory`.
- Point any Docker client at it with `DOCKER_HOST=unix://…/docker.sock` — Docker CLI,
  Testcontainers, Traefik and Portainer all work against Coral this way.

```bash
coral-daemon --docker-socket /run/coral/docker.sock
DOCKER_HOST=unix:///run/coral/docker.sock docker ps
```

### 5. GPU & Device Passthrough

Containers can be given direct access to host hardware.

- **`--device HOST_DEV[:CTR_DEV[:PERMS]]`** — Docker-compatible device mapping. Paths must be
  absolute and the host device must exist; `r` without `w` produces a read-only mount.
- **`--gpus all` / `--gpus <index>` / `--gpus device=<index>`** — automatic GPU discovery for
  **NVIDIA** (`nvidiactl`, `nvidia-uvm`, `nvidia-modeset`, `nvidia-caps`, `nvidia0..15`),
  **AMD ROCm** (`/dev/kfd`) and **DRM** (`/dev/dri`). The matching `NVIDIA_VISIBLE_DEVICES`,
  `NVIDIA_DRIVER_CAPABILITIES` and `ROCR_VISIBLE_DEVICES` variables are injected.
- **`--network-alias`** (alias `--alias`) — repeatable, network-scoped service alias.
- **`--log-driver`** — `raw` (default), `json-file`, `syslog` or `journald`.

```bash
coral run --gpus all --device /dev/video0:/dev/video0:ro nvcr.io/nvidia/cuda:12.4.0-base-ubuntu22.04 nvidia-smi
```

All of the above are also available in Compose files:

```yaml
services:
  trainer:
    image: nvcr.io/nvidia/cuda:12.4.0-base-ubuntu22.04
    gpus: all
    devices: ["/dev/video0:/dev/video0:ro"]
    log_driver: json-file
    aliases: [trainer-alias]
```

> Full command reference: [`coral_repo/docs/MANUAL.md`](coral_repo/docs/MANUAL.md) ·
> architecture notes: [`coral_repo/docs/ROADMAP.md`](coral_repo/docs/ROADMAP.md)

---

## Quick Install

> **Platform support:** the Coral engine (`coral`, `coral-daemon`) runs on **Linux only** — it is
> built directly on Linux syscalls, user namespaces and cgroups. **x86_64 and arm64 (ARM64) are
> both supported**, and CI builds and runs the full E2E suite on both architectures. The
> **Coral Desktop** GUI is a separate client and is available for Linux, Windows and macOS;
> it controls a Coral daemon and does not make the engine portable to those platforms.
> The hardware **`/dev/kvm` MicroVM and the AI Agent Sandbox are x86_64-only**; every other
> feature (OCI, CRI, CNI, images, signing, SBOM, WASM) works on arm64 as well.

### One-Liner (Linux x86_64 / arm64)

```bash
curl -fsSL https://seashyne.github.io/Coral-Container-Project/install.sh | bash
```

### Debian / Ubuntu `.deb`

Download the package matching your architecture — `coral-runtime_2.8.1_amd64.deb` (x86_64) or
`coral-runtime_2.8.1_arm64.deb` (arm64) — and the `Coral Desktop` installer from
[Releases](https://github.com/seashyne/Coral-Container-Project/releases/latest), then run:

```bash
sudo dpkg -i coral-runtime_2.8.1_amd64.deb    # or ..._arm64.deb
systemctl --user enable --now coral-daemon.service
coral version
```

### Build from Source (Linux)

```bash
git clone https://github.com/seashyne/Coral-Container-Project.git
cd Coral-Container-Project/coral_repo
./install.sh        # builds --release, installs to ~/.local/bin, enables coral-daemon.service
./verify.sh         # runs full 130+ test OCI + CRI + Wasm + KVM E2E suite + strict Clippy
```

---

## Try Coral v2.0 in 30 Seconds

```bash
# Run an isolated rootless Alpine container with resource limits
coral run --memory 64m --pids-limit 64 alpine:3.20 /bin/sh -c 'echo "PID=$$ host=$(hostname)"'

# Probe real hardware /dev/kvm 64-bit MicroVM boot latency
coral vm probe

# Run a WASI Preview 1 WebAssembly module with fuel & RAM caps
coral wasm run app.wasm --fuel 5000000 --memory-mb 32

# Execute untrusted AI-generated code in a zero-network read-only sandbox
coral sandbox run --image alpine:3.20 --timeout 5 -- /bin/sh -c 'echo "Safe sandbox PID=$$"'
```

---

## Monorepo Layout

```text
.
├── Makefile             # Unified workspace commands (`make build`, `make verify`, `make desktop-dev`)
├── coral_repo/          # 10-crate Rust workspace (OCI, CRI v1 gRPC, Wasm, KVM VMM)
│   ├── api/             # [coral-api] Kubernetes CRI v1 Protobuf & Tonic gRPC bindings
│   ├── pkg/             # oci, cgroups, networking, runtime, wasm, vmm
│   ├── cmd/             # coral-cli (`coral`) + coral-daemon
│   ├── tests/           # Unit, OCI, Docker-parity, CRI, Wasm & KVM E2E tests
│   ├── scripts/         # setup-rootfs.sh, package-deb.sh, benchmark.sh, push_coral.sh
│   └── docs/            # ARCHITECTURE.md, ROADMAP.md, MANUAL.md, BENCHMARK.md
├── desktop/             # Tauri v2 Coral Desktop GUI (Theme/Token CSS, EN/TH i18n, Linux/Win/macOS)
├── docs/                # GitHub Pages Docs website (TH/EN, install guides, sponsor cards)
├── tools/               # Shared build tooling (HTML partial assembler)
├── infra/               # Containerized build Dockerfile & compose
└── .github/workflows/   # CI/CD: release.yml, desktop.yml, pages.yml, benchmark.yml (nightly latency)
```

Both frontends (Desktop shell and Docs page) are assembled from small partials by a
dependency-free Node script (`tools/html-assembler.mjs`); the generated `index.html`
files are build artifacts and are not committed. `./check-line-limit.sh` keeps every
source file under 400 lines.

---

## Support the Project

If Coral saves you time or helps your team build safer, faster containers and AI sandboxes, consider supporting ongoing development via the [Sponsor / Donate section on the Docs site](https://seashyne.github.io/Coral-Container-Project/#support).

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

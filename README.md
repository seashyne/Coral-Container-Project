# Coral Container Project (`v2.2.0`)

**Coral** is a next-generation, memory-safe container runtime written in **Rust (Edition 2024)** — uniting a rootless-first **OCI v1.0.2** Linux container engine, **Kubernetes CRI v1 gRPC** daemon (TCP + `unix://` Unix Domain Socket + CNI plugin chain & streaming), **WASI Serverless HTTP WebAssembly** runtime (`coral-wasm`), pure-Rust Linux **`/dev/kvm` Separate-Kernel MicroVM with VirtIO-Blk & VirtIO-Net** (`coral-vmm`, Zero QEMU/Firecracker), **Prometheus `/metrics`**, **Ed25519 Image Signing + SPDX 2.3 SBOM**, multi-arch **x86_64 & ARM64**, and the cross-platform **Coral Desktop** GUI (Linux / Windows / macOS `.dmg`).

> **Documentation & Downloads:** https://seashyne.github.io/Coral-Container-Project/  
> **GitHub Releases (`.deb`, `.tar.gz`, Desktop installers):** https://github.com/seashyne/Coral-Container-Project/releases

---

## Highlights (`v2.2.0`)

- **Real-World OCI Compatibility & Ergonomics**: Inherits full OCI image config (`ENTRYPOINT`/`CMD`/`ENV`/`WORKDIR`/`USER`), remaps single-UID rootless service users (`redis`, `nginx`, `postgres`), forwards `SIGTERM`/`SIGINT`, binds `/dev/{fd,stdin,stdout,stderr,ptmx,shm}`, rotates logs (`--log-max-size-mb`), runs background Auto-GC, and provides `coral completion bash|zsh|fish`.
- **Kubernetes CRI v1 Full Lifecycle**: Dual TCP + Kubernetes **Unix Domain Socket (`unix:///.../coral.sock`)**, real CNI v1.0.0 plugin chain execution (`/opt/cni/bin/*`), plus streaming `Exec` / `Attach` / `PortForward`, `ContainerStats`, `ReopenContainerLog`, and `ImageFsInfo`.
- **WASI Serverless HTTP WebAssembly (`coral-wasm`)**: Run `.wasm`/`.wat` in **326 µs** OR host per-request isolated Serverless HTTP/1.1 microservices (`coral wasm serve`) in **89 µs/req** with deterministic CPU fuel metering & linear memory caps.
- **Pure-Rust `/dev/kvm` MicroVM + VirtIO-Blk & VirtIO-Net (`coral-vmm`, Zero QEMU)**: Boot Linux 6.6 + modules directly over `/dev/kvm` with persistent `/dev/vda` ext4/raw storage (`coral vm disk create`, `--disk`) and userspace VirtIO-Net TCP/UDP/DHCP/ICMP + `-p` port forwarding (`--net`) with zero root/TAP.
- **Observability, Security & Multi-Platform Releases**: Built-in Prometheus `/metrics` & `/healthz`, `cargo-fuzz` + deterministic fuzz regression tests, Ed25519 signing + SPDX 2.3 SBOM, **Linux `x86_64`/`aarch64`**, and **Coral Desktop (Linux, Windows, macOS Apple Silicon `.dmg`)**.

---

## Quick Install

### One-Liner (Linux x86_64)

```bash
curl -fsSL https://seashyne.github.io/Coral-Container-Project/install.sh | bash
```

### Debian / Ubuntu `.deb`

Download `coral-runtime_2.2.0_amd64.deb` from [Releases](https://github.com/seashyne/Coral-Container-Project/releases/latest) and run:

```bash
sudo dpkg -i coral-runtime_2.2.0_amd64.deb
systemctl --user enable --now coral-daemon.service
coral version
```

### Build from Source (Linux)

```bash
git clone https://github.com/seashyne/Coral-Container-Project.git
cd Coral-Container-Project/coral_repo
./install.sh        # builds --release, installs to ~/.local/bin, enables coral-daemon.service
./verify.sh         # runs full 60-test OCI + CRI + Wasm + KVM E2E suite + strict Clippy
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
├── coral_repo/          # 10-crate Rust workspace (OCI, CRI v1 gRPC, Wasm, KVM VMM)
│   ├── api/             # [coral-api] Kubernetes CRI v1 Protobuf & Tonic gRPC bindings
│   ├── pkg/             # oci, cgroups, networking, runtime, wasm, vmm
│   ├── cmd/             # coral-cli (`coral`) + coral-daemon
│   ├── tests/           # 60 unit, OCI, parity, CRI, Wasm & KVM E2E tests
│   ├── scripts/         # setup-rootfs.sh & package-deb.sh
│   └── docs/            # ARCHITECTURE.md, ROADMAP.md, MANUAL.md
├── desktop/             # Tauri v2 Coral Desktop GUI (Linux & Windows WSL2)
├── docs/                # GitHub Pages Docs website (TH/EN, install guides, sponsor cards)
├── infra/               # Containerized build Dockerfile & compose
└── .github/workflows/   # CI/CD: release.yml, desktop.yml, pages.yml
```

---

## Support the Project

If Coral saves you time or helps your team build safer, faster containers and AI sandboxes, consider supporting ongoing development via the [Sponsor / Donate section on the Docs site](https://seashyne.github.io/Coral-Container-Project/#support).

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

# Coral Container Project (`v2.0.0`)

**Coral** is a next-generation, memory-safe container runtime written in **Rust (Edition 2024)** — uniting a rootless-first **OCI v1.0.2** Linux container engine, **Kubernetes CRI v1 gRPC** daemon, **WASI Preview 1 WebAssembly** runtime (`coral-wasm`), pure-Rust Linux **`/dev/kvm` 64-bit MicroVM & AI Agent Sandbox** (`coral-vmm`), and the cross-platform **Coral Desktop** GUI (Tauri v2).

> **Documentation & Downloads:** https://seashyne.github.io/Coral-Container-Project/  
> **GitHub Releases (`.deb`, `.tar.gz`, Desktop installers):** https://github.com/seashyne/Coral-Container-Project/releases

---

## Highlights (`v2.0.0`)

- **Zero-GC Rust OCI Runtime (`coral`)**: Rootless by default (`CLONE_NEWUSER` + `pivot_root`), OverlayFS Copy-on-Write multi-lowerdir, Cgroups v2 limits & live stats, pure-Rust Seccomp-BPF (27 blocked syscalls), OCI Capability drop, interactive `-it` PTY, and real `setns()` `exec -it`.
- **Full Developer Workflow**: `coral build` (Dockerfile + Content-Addressable Step Cache), `coral compose up/ps/logs/down`, `coral login/push/pull` (OCI Distribution v2), `coral save/load` (`.tar`/`.tar.gz`/`.tar.zst`), `coral network create/ls/rm` (`pasta`/`slirp4netns` rootless egress + sibling `/etc/hosts`), `coral volume`, `coral cp`, `coral logs -f`, `coral stats --watch`, `coral system df/prune`, persistent state & Boot Auto-Resume, and live Healthchecks.
- **WebAssembly Runtime (`coral-wasm`)**: Run `.wasm` / `.wat` & OCI Wasm images directly via WASI Preview 1 (`wasmi 2.0`) with deterministic CPU fuel metering and linear memory ceilings — **326 µs** cold start on release builds without a Linux rootfs.
- **KVM 64-bit MicroVM & AI Agent Sandbox (`coral-vmm`)**: Pure-Rust native `/dev/kvm` x86-64 Long Mode hypervisor (**2.94 ms** boot + COM1 serial execution), Linux `newc` initramfs builder, and hardened zero-network AI Agent Sandbox (`coral sandbox run`) with full process-group timeout termination.
- **Coral Desktop (`desktop/`)**: Native Linux & Windows (WSL2) Tauri v2 GUI with full-duplex live streaming PTY terminal (`ExecStream` gRPC), SVG CPU/RAM sparklines, Volumes/Networks/Registry manager, and the **Next-Gen (Wasm & KVM)** playground.

---

## Quick Install

### One-Liner (Linux x86_64)

```bash
curl -fsSL https://seashyne.github.io/Coral-Container-Project/install.sh | bash
```

### Debian / Ubuntu `.deb`

Download `coral-runtime_2.0.0_amd64.deb` from [Releases](https://github.com/seashyne/Coral-Container-Project/releases/latest) and run:

```bash
sudo dpkg -i coral-runtime_2.0.0_amd64.deb
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

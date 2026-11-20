# Coral Container Runtime (`v2.2.0`)

**Coral** is a modular, memory-safe Linux container runtime, Kubernetes CRI v1 gRPC daemon, WebAssembly WASI runtime, and native `/dev/kvm` MicroVM / AI Sandbox engine built in **Rust (Edition 2024)**.

## Workspace Architecture (10 Crates)

```text
coral_repo/
├── api/                 # [coral-api] Kubernetes CRI v1 Protobuf & Tonic gRPC bindings (ExecStream PTY)
├── pkg/
│   ├── oci/             # [coral-oci] OCI Runtime Specification v1.0.2 & state.json
│   ├── cgroups/         # [coral-cgroups] Cgroups v2 resource limits & live observability
│   ├── networking/      # [coral-net] CNI v1.0.0 bridge, custom networks, pasta/slirp4netns egress
│   ├── runtime/         # [coral-runtime] Root/rootless lifecycle, OverlayFS CoW, Seccomp-BPF, PTY, setns
│   ├── wasm/            # [coral-wasm] Pure-Rust WASI Preview 1 WebAssembly runtime (wasmi 2.0)
│   └── vmm/             # [coral-vmm] Pure-Rust /dev/kvm 64-bit MicroVM hypervisor & AI Agent Sandbox
├── cmd/
│   ├── coral-cli/       # [coral] OCI/Docker-style CLI (run, build, compose, push, wasm, vm, sandbox)
│   └── coral-daemon/    # [coral-daemon] Async Tokio + Tonic CRI v1 gRPC daemon with persistent state
├── tests/               # [coral-e2e-tests] 60 unit, OCI, parity, CRI, Wasm & hardware KVM E2E tests
├── fuzz/                # [coral-fuzz] cargo-fuzz targets (oci_spec, tar_extract, wasm_http_cgi, virtio_ring)
├── scripts/             # setup-rootfs.sh & package-deb.sh
└── docs/                # ARCHITECTURE.md, ROADMAP.md, MANUAL.md
```

## Quick Start

```bash
# 1. Build & install release binaries (~/.local/bin/coral + coral-daemon.service)
./install.sh

# 2. Run full verification suite (60 tests + strict Clippy -D warnings + live container run)
./verify.sh

# 3. Build Debian/Ubuntu package (target/packages/coral-runtime_2.2.0_amd64.deb)
./scripts/package-deb.sh 2.2.0
```

## Documentation

- [Online Docs & Install Guide (TH/EN)](https://seashyne.github.io/Coral-Container-Project/)
- [System Architecture](docs/ARCHITECTURE.md)
- [Engineering Roadmap (Phases 1–3)](docs/ROADMAP.md)
- [User & Developer Manual (คู่มือการใช้งาน)](docs/MANUAL.md)

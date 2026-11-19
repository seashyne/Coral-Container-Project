# Coral Container Runtime — Engineering Roadmap

วิสัยทัศน์ของโปรเจกต์ **Coral**: สร้าง Container Runtime ยุคใหม่ด้วยภาษา Rust ที่มีความเร็วสูง (Zero-GC overhead), ปลอดภัยตั้งแต่ระดับสถาปัตยกรรม (Memory-safe + Rootless UserNS by default), และเข้ากันได้กับมาตรฐานอุตสาหกรรม (OCI Runtime Spec & Kubernetes CRI v1)

---

## Phase 1: Compatibility (OCI v1.0.2 & Kubernetes CRI v1) — Completed in v1.0
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **1.1 Standardized Workspace** | Workspace | แยกโครงสร้างเป็น `api/`, `pkg/{oci,cgroups,networking,runtime}`, `cmd/{coral-cli,coral-daemon}`, `tests/` | Done |
| **1.2 OCI Runtime Spec v1.0.2** | `coral-oci` | พาร์สและตรวจสอบ `config.json`, สร้าง `state.json`, ป้องกัน Path Traversal ใน Container ID และ Mounts | Done |
| **1.3 Root & Rootless Isolation** | `coral-runtime` | รองรับทั้ง `sudo` (Root) และ Rootless (`CLONE_NEWUSER` + `uid_map`/`gid_map`) พร้อม `pivot_root` + `MNT_DETACH` บน Alpine Linux rootfs | Done |
| **1.4 Split OCI Lifecycle** | `coral-runtime` | รองรับคำสั่งมาตรฐาน `spec`, `create` (`exec.fifo`), `start`, `run`, `state`, `list`, `kill`, `delete` | Done |
| **1.5 CRI v1 gRPC Daemon** | `coral-daemon` | รองรับ `RuntimeService` (`Version`, `RunPodSandbox`, `CreateContainer`, `StartContainer`, `StopContainer`, `RemoveContainer`, `ContainerStatus`, `ExecSync`) และ `ImageService` โดยไม่ต้องติดตั้ง `protoc` แยก | Done |

---

## Phase 2: Performance & Resource Optimization — Completed in v1.0 / Ongoing v1.x
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **2.1 Cgroups v2 Controller** | `coral-cgroups` | จำกัดและควบคุม `memory.max`, `memory.high`, `cpu.max` (quota/period), `cpu.weight` (แปลงจาก OCI shares), และ `pids.max` | Done |
| **2.2 Built-in Observability** | `coral-cgroups` | อ่านค่าใช้งานจริงจาก Kernel (`memory.current`, `pids.current`, `cpu.stat`) ผ่านคำสั่ง `coral stats <id>` | Done |
| **2.3 Zero-Lock Fork Pipeline** | `coral-cli` | แยก Binary `coral` ให้รันแบบ Single-threaded ก่อน `fork()` + `unshare()` เพื่อป้องกัน Deadlock และลด Startup latency | Done |
| **2.4 CNI v1.0.0 Networking** | `coral-net` | โมเดลคอนฟิก CNI Bridge (`coral0`), การตรวจสอบ CIDR, และการเปิดใช้งาน `lo` (Loopback) ใน Network Namespace | Done |
| **2.5 OverlayFS Snapshotter** | `coral-runtime` | จัดการ Image Layers ด้วย Kernel OverlayFS (`lowerdir`, `upperdir`, `workdir`, `userxattr` สำหรับ rootless) แยก writable layer ต่อคอนเทนเนอร์เป็นค่าเริ่มต้น | Done |

---

## Phase 3: The Killer Feature (Next-Gen Sandboxing & Security) — Ongoing
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **3.1 Seccomp-BPF Syscall Filter** | `coral-runtime` | Pure-Rust Classic BPF filter บล็อก syscalls อันตราย 27 ตัว (คืน `EPERM`) เป็นค่าเริ่มต้น | Done |
| **3.2 Linux Capabilities Drop** | `coral-runtime` | จำกัด Bounding/Permitted/Effective Capabilities เหลือเฉพาะ OCI default safe set (ตัด `CAP_SYS_ADMIN`, `CAP_SYS_PTRACE`, `CAP_BPF` ฯลฯ) | Done |
| **3.3 Interactive PTY, Volumes & Real `setns` Exec** | `coral-runtime` / `coral-cli` / `coral-daemon` | รองรับ `-it` PTY, Named Volumes (`coral volume`) + Host Bind Mounts (`-v`), `--network bridge` (Cross-NetNS TCP Port Forwarding ด้วย `SCM_RIGHTS`) + `--dns` + `--add-host`, และ `coral exec -it` เข้า namespace จริงด้วย `setns()` | Done |
| **3.4 Dockerfile Build, CoW Diff & Commit** | `coral-daemon` / `coral-cli` / `coral-runtime` | รองรับ `coral build -t <tag>` พร้อม Content-Addressable Step Cache (`[CACHED]` / `--no-cache`), `coral diff <id>`, `coral commit <id> <new_image>`, และ Multi-lowerdir OverlayFS | Done |
| **3.5 Multi-Container Compose & Image Archives** | `coral-daemon` / `coral-cli` / `desktop` | รองรับ `coral compose up / ps / logs / down` (`docker-compose.yml` / `coral-compose.yml` + `depends_on` topological sort), `coral tag`, `coral save / load` (`.tar`, `.tar.gz`, `.tar.zst`) และ `zstd` OCI layers | Done |
| **3.6 Persistent State, Boot Auto-Resume, Healthchecks, Live Logs & Web Terminal** | `coral-daemon` / `coral-cli` / `desktop` | บันทึกสถานะคอนเทนเนอร์/พอดลง `daemon-state.json` ข้ามการรีสตาร์ตพร้อม Boot Auto-Resume สำหรับ `--restart always/unless-stopped`, Healthcheck (`--health-cmd`), Live Streaming Logs (`coral logs -f --tail`), `coral stats --watch`, `coral inspect`, `coral cp`, `coral system df/prune`, และ Tabbed Drawer (Web Terminal + SVG Sparklines + Volumes & Storage UI) ใน Coral Desktop | Done |
| **3.7 Private Registry Push/Login, Custom Networks, Rootless Egress, True PTY Stream & Release CI** | `coral-api` / `coral-net` / `coral-daemon` / `coral-cli` / `desktop` | รองรับ `coral login`/`logout` (`auth.json` mode `0600`), `coral push` (OCI Distribution v2), `coral network create/ls/inspect/rm` พร้อม sibling `/etc/hosts` + `pasta`/`slirp4netns` rootless internet egress, Full-Duplex PTY Streaming (`ExecStream` gRPC) ใน Coral Desktop, สคริปต์แพ็ก `.deb`, และ GitHub Actions Release Pipeline | Done |
| **3.8 WebAssembly (Wasm) Workload** | `coral-wasm` | รัน `.wasm`/`.wat` และ OCI Wasm artifacts โดยตรงผ่าน WASI Preview 1 (`wasmi`) ระดับไมโครวินาที พร้อม CPU Fuel Metering + Memory Ceiling (`coral wasm run / pack`, `coral run --runtime wasm`) | Done |
| **3.9 MicroVM Isolation Backend & AI Agent Sandbox** | `coral-vmm` | Pure-Rust Linux `/dev/kvm` 64-bit Long Mode MicroVM Hypervisor (4-level paging, 16550 UART serial, VM-exit budget), Linux `newc` initramfs builder, และ AI Agent Untrusted Code Sandbox (`coral vm probe / run`, `coral sandbox run`) | Done |

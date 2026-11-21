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

---

## Phase 4: Deep-Tech Enterprise & Cloud-Native (v2.1) — Completed
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **4.1 Pure-Rust Linux Kernel Boot (Zero-QEMU)** | `coral-vmm` | บูต Linux x86-64 `bzImage` (`zero_page` + E820 + KVM IRQChip/PIT2 + 16550A UART + IRQ 4 + VirtIO-MMIO v2) โดยตรงผ่าน `/dev/kvm` พร้อม `coral vm setup-kernel` (โหลด Alpine `linux-virt` อัตโนมัติ) และ `coral vm boot` | Done |
| **4.2 WASI Serverless HTTP Runtime** | `coral-wasm` | รัน `.wasm`/`.wat` เป็น HTTP/1.1 Microservice (`coral wasm serve`) ด้วย Per-Request Micro-Isolated WASI Sandbox + `coral_http`/`wasi:http` host imports + telemetry headers (`X-Coral-Wasm-Duration-Us`, `X-Coral-Wasm-Fuel`) | Done |
| **4.3 Kubernetes CRI v1 Unix Socket & Status RPC** | `coral-api` / `coral-daemon` / `coral-cli` | รองรับ CRI v1 บน Unix Domain Socket (`--unix-socket` / `--endpoint unix:///...`) ควบคู่ TCP พร้อม `Status` (`RuntimeReady`/`NetworkReady`) และ `PodSandboxStatus` สำหรับ `kubelet` / `crictl` | Done |
| **4.4 Ed25519 Image Signing, SPDX 2.3 SBOM & ARM64** | `coral-daemon` / `coral-cli` / CI | `coral keygen` / `coral sign` / `coral verify` (Ed25519 + SHA-256 Rootfs Merkle Tree ตรวจจับการแก้ไขระดับไบต์), `coral sbom` (SPDX 2.3 JSON จาก APK/DPKG/Wasm), และ ARM64 (`aarch64`) Release CI | Done |

---

## Phase 5: Production & Deep-Tech Enterprise (v2.2) — Completed
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **5.1 Pure-Rust VirtIO-Blk & VirtIO-Net (`coral-vmm`)** | `coral-vmm` | VirtIO-MMIO v2 Block (`/dev/vda` persistent ext4/raw disk + `coral vm disk create`) และ Userspace L2/L3 VirtIO-Net (`eth0` 10.0.2.15/24, DHCP, ICMP, outbound TCP/UDP, `-p` host->guest port forward) พร้อม Auto-Extract Kernel Modules | Done |
| **5.2 Kubernetes CRI v1 CNI Plugin Chain & Streaming** | `coral-net` / `coral-daemon` | รัน CNI plugin chain (`/etc/cni/net.d/*.conflist`, `/opt/cni/bin/*`) พร้อม reverse-DEL rollback, Streaming HTTP/SPDY (`Exec`, `Attach`, `PortForward`), `ContainerStats` และ `ImageFsInfo` | Done |
| **5.3 Real-World Image Config, Log Rotation, Auto-GC & Shell Completion** | `coral-daemon` / `coral-runtime` / `coral-cli` | สืบทอด `ENTRYPOINT`/`CMD`/`ENV`/`WORKDIR`/`USER`, single-UID non-root UserNS remap (`nginx`/`redis`/`postgres`), `SIGTERM` guard, Log Rotation (`--log-max-size-mb`), Background Auto-GC, และ `coral completion` | Done |
| **5.4 Prometheus `/metrics`, Fuzzing & Coral Desktop macOS `.dmg`** | `coral-daemon` / `fuzz/` / `desktop` | HTTP `/metrics` & `/healthz`, `cargo-fuzz` targets (`oci_spec`, `tar_extract`, `wasm_http_cgi`, `virtio_ring`), UI ใหม่สำหรับ Sign/Verify/SBOM + KVM Boot, และ macOS `.dmg` CI | Done |

---

## Phase 6: Docker-Parity Ergonomics, Desktop UX/UI Overhaul & Verified Comparison (`v2.3.0`) — Completed
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **6.1 Docker-Compatible `coral run` CLI** | `coral-cli` (`units.rs`, `run_resolve.rs`) | รองรับ positional `<IMAGE> [CMD...]`, human-readable memory (`-m 4G`/`512m`), `--pids-limit`, `--pull missing\|always\|never` และ `-d` route เข้า daemon อัตโนมัติ | Done |
| **6.2 Modular Token CSS & Bilingual EN/TH UI** | `desktop/src` | จัดโครงสร้าง Design Token CSS 3 ชั้น (`primitives`, `semantic`, `components`), แยก JS เป็นโมดูล พร้อมพจนานุกรม EN/TH ครบ 468 คีย์ | Done |
| **6.3 Topbar Theme Popover & Easier Workflow** | `desktop` | ปุ่ม Theme Popover บน Topbar (Auto/Light/Dark + 5 ชุดสี Coral/Ocean/Emerald/Indigo/Amber + Density + EN/TH), Quick Fill presets (รวม Minecraft PaperMC), optional Container Name, Inline Discover Quick Pull, และแก้ GUI `$PATH` / View Stacking / PTY cleanup bugs | Done |
| **6.4 Verified Same-Machine Docker Benchmark & Report** | `docs/`, `coral_repo/docs/` | วัดผลจริงบนเครื่องเดียวกันระหว่าง `Coral v2.3.0` กับ `Docker 29.10.0 / 29.9.3` (Coral เร็วกว่า 7.3×–11.2× P50), สร้างรูปคู่มือ + กราฟเปรียบเทียบ และเขียนรายงาน `REPORT_v2.3.md` | Done |

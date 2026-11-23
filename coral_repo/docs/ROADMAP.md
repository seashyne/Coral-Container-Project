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

---

## Phase 7: Production Docker Parity, BuildKit, `docker.sock`, Embedded DNS & GPU/Devices (`v2.4.0`) — Completed
| Milestone | Component | Details | Status |
| :--- | :--- | :--- | :--- |
| **7.1 Shared Content-Addressable Layer Cache & Parallel Pull** | `coral-daemon` (`images/registry/layer_cache.rs`) | แคชเลเยอร์ตาม `sha256` digest ใน `<images_dir>/.layer-cache/sha256/<hex>.blob` ข้ามอิมเมจ — ตรวจสอบ SHA-256 ทั้งตอนดาวน์โหลดและตอนอ่านจากแคช (ไฟล์ที่ digest ไม่ตรงถูกลบอัตโนมัติ), ย้ายเข้าแคชแบบ atomic ผ่าน `rename` (มี `copy` เป็นทางสำรอง) และดาวน์โหลดแบบขนานสูงสุด 4 สตรีมก่อนประกอบกลับตามลำดับ manifest | Done |
| **7.2 Embedded UDP DNS, `--network-alias` & Live `/etc/hosts` Mesh** | `coral-net` (`dns_server.rs`) / `coral-runtime` / `coral-daemon` (`runtime_rpc/create.rs`) | เพิ่ม RFC 1035 Embedded UDP DNS responder ที่รันในตัว daemon บนพอร์ต loopback ชั่วคราว (`<name>.coral.local`), รองรับ `--network-alias` / Compose `aliases:` และอัปเดต `/etc/hosts` แบบ in-place สองทิศทางให้คอนเทนเนอร์ที่รันอยู่ — ตอบ `A`/`ANY`, คืน NODATA สำหรับ `AAAA` และ NXDOMAIN สำหรับชื่อที่ไม่รู้จัก | Done |
| **7.3 BuildKit Heredoc, `RUN --mount` & Image Config Metadata** | `coral-daemon` (`builder/buildkit.rs`, `images/config.rs`) | รองรับ `RUN <<EOF`, `RUN --mount=type=cache\|secret\|tmpfs` (secret mount แบบ read-only จาก build context เท่านั้น, tmpfs เป็น `nosuid,nodev,mode=1777`), Multi-stage `COPY --from=<stage\|index\|image>`, บันทึก `EXPOSE`/`VOLUME`/`SHELL`/`HEALTHCHECK`/`ONBUILD` ลง `image-config.json` และสืบทอด `HEALTHCHECK` อัตโนมัติ | Done |
| **7.4 Docker Engine REST API Compatibility (`docker.sock`)** | `coral-daemon` (`service/docker_compat/`) | เปิด Unix socket `<state_dir>/docker.sock` โหมด `0660` (`~/.local/state/coral/daemon/docker.sock` เมื่อรันจาก systemd unit ของแพ็กเกจ) และบน TCP รองรับ Docker REST API v1.24–v1.47 (`/_ping`, `/version`, `/info`, `/images/json`, `/containers/json`, `/containers/create`, `/containers/{id}/json`, `/containers/{id}/logs`, `/containers/{id}/{start,stop,kill,restart,wait}`, `DELETE /containers/{id}`) สำหรับ Testcontainers/Traefik/Docker clients ผ่าน `DOCKER_HOST=unix://…` | Done |
| **7.5 GPU/Device Passthrough & Multi-Backend Log Drivers** | `coral-runtime` (`rootfs/devices.rs`) / `coral-daemon` (`compose/`) / `coral-cli` (`args/run.rs`) / `desktop` | รองรับ `--device HOST_DEV[:CTR_DEV[:PERMS]]` แบบ Docker-compatible, `--gpus all\|<index>\|device=<index>` ที่ค้นพบ NVIDIA (`nvidiactl`/`nvidia-uvm`/`nvidia-modeset`/`nvidia-caps`/`nvidia0..15`), AMD ROCm (`/dev/kfd`) และ DRM (`/dev/dri`) พร้อม inject `NVIDIA_VISIBLE_DEVICES`, `NVIDIA_DRIVER_CAPABILITIES`, `ROCR_VISIBLE_DEVICES` — รวมถึง `--network-alias`, `--log-driver raw\|json-file\|syslog\|journald` ครบทั้ง CLI, Compose และ Coral Desktop v2.4.0 | Done |

---

## Phase 8 — Deep Docker Parity & Multi-Arch BuildKit (`v2.5.0`)

| ฟีเจอร์ | Crates / Modules | รายละเอียด | สถานะ |
| :--- | :--- | :--- | :--- |
| **8.1 Zero-Copy Shared OverlayFS Lowerdirs & Hardlink Layer Store** | `coral-daemon` (`images/registry/layer_store.rs`, `service/oci_spec.rs`) / `coral-cli` (`local_run.rs`) | แตก OCI layer แต่ละ digest ครั้งเดียวลง `<images_dir>/.layers/sha256/<hex>/` แล้ว materialize rootfs ด้วย zero-copy `fs::hard_link` (แชร์ inode ไม่กินพื้นที่ซ้ำ) พร้อมบันทึก `layers.json` เพื่อส่ง multi-lowerdir (`lowerdir=top:...:base`) เข้า `OverlayDirs::prepare_multi` อัตโนมัติ | Done |
| **8.2 Full `docker.sock` REST API Surface** | `coral-daemon` (`service/docker_compat/{state,endpoints,resources,containers,handlers}.rs`) | ขยาย Docker Engine HTTP REST API v1.45 ครบชุดสำหรับ Testcontainers / VS Code Dev Containers / Traefik: `POST /images/create` (pull stream), `GET/DELETE /images/{name}`, `POST /containers/{id}/exec`, `POST /exec/{id}/start` (8-byte multiplexed frame), `GET /exec/{id}/json`, `POST /containers/{id}/wait`, `GET /events`, `/networks/*`, `/volumes/*` และ `HostConfig` (`PortBindings`, `RestartPolicy`, `LogConfig`, `DeviceRequests`) | Done |
| **8.3 Pure-Rust Rootless L2 Virtual Ethernet Switch (`10.89.x.0/24`)** | `coral-net` (`vswitch.rs`, `vswitch_tap.rs`) / `coral-daemon` (`service/vswitch_net.rs`) | L2 learning switch ด้วย `/dev/net/tun` (`IFF_TAP \| IFF_NO_PI`) ภายใน unprivileged `CLONE_NEWUSER \| CLONE_NEWNET` แจก IP ส่วนตัวจริง (`10.89.x.2`, `.3`, ...) ให้คอนเทนเนอร์บน custom network คุยกันตรงทุกพอร์ตข้าม netns โดยไม่ต้อง root หรือ `-p` | Done |
| **8.4 NVIDIA Container Toolkit (CNCF CDI JSON + Host `.so` Driver Injection)** | `coral-runtime` (`rootfs/gpu_cdi.rs`, `rootfs/devices.rs`) | อ่าน CNCF CDI spec (`/etc/cdi/*.json`, `/var/run/cdi/*.json`, รองรับ `--gpus nvidia.com/gpu=all`) และค้นหา bind-mount ไลบรารีไดรเวอร์โฮสต์ (`libcuda.so*`, `libnvidia-ml.so*`, `nvidia-smi` ฯลฯ) แบบ read-only (`ro`) พร้อมตั้ง `LD_LIBRARY_PATH` ให้อัตโนมัติ | Done |
| **8.5 Multi-Arch Buildx (`--platform` + QEMU), Parallel Multi-Stage DAG & `coral machine`** | `coral-daemon` (`builder/{dag,dag_exec}.rs`) / `coral-cli` (`args/machine.rs`, `dispatch/machine.rs`) | รองรับ `--platform` บน `coral pull`/`coral build` (`TARGETOS`/`TARGETARCH`/`TARGETPLATFORM` + ตรวจ `/proc/sys/fs/binfmt_misc/qemu-*`), รัน multi-stage Dockerfile อิสระแบบ parallel topological waves พร้อม dead-stage pruning และคำสั่ง `coral machine init\|start\|stop\|status\|env` (Linux native, macOS Lima+VirtioFS, Windows WSL2) | Done |

---

## Phase 9 — Streaming `docker.sock`, In-Subnet Gateway DNS, Buildx Pro, Compose v2 & Live Lifecycle (`v2.6.0`)

| ฟีเจอร์ | Crates / Modules | รายละเอียด | สถานะ |
| :--- | :--- | :--- | :--- |
| **9.1 `docker.sock` HTTP Hijack, Chunked Streaming, `/archive` & `/build`** | `coral-daemon` (`service/docker_compat/{state,streaming,archive_build,containers}.rs`) | รองรับ `Upgrade: tcp` (`101 UPGRADED` hijack stream สำหรับ `/exec/{id}/start`), `Transfer-Encoding: chunked` (`GET /containers/{id}/logs?follow=1`, `GET /events?stream=1` broadcast), `PUT/GET /containers/{id}/archive` (tar copy สำหรับ Testcontainers), `POST /build` (tar build context), และ `/pause`, `/unpause`, `/rename`, `/update` | Done |
| **9.2 Rootless L3 Virtual Gateway (`10.89.x.1`) + In-Subnet DNS (`10.89.x.1:53`)** | `coral-net` (`vswitch_gw.rs`, `vswitch.rs`, `dns_server.rs`) / `coral-daemon` (`service/vswitch_net.rs`) | Gateway เสมือน `10.89.x.1` ฝังใน `RootlessVSwitch` ตอบ ARP, ICMP Echo (`ping 10.89.x.1`) และ RFC 1035 UDP DNS (`10.89.x.1:53`) จากคอนเทนเนอร์ใน isolated netns ได้ตรง | Done |
| **9.3 BuildKit/Buildx Pro (`.dockerignore`, `--mount=type=bind\|ssh`, Multi-Arch Index Push & `--cache-from`)** | `coral-daemon` (`builder/{dockerignore,buildkit,cache,step_copy}.rs`, `images/registry/manifest_list.rs`) / `coral-cli` (`daemon_client/images/build.rs`) | กรองไฟล์ตอน `COPY` และ hash cache ตาม `.dockerignore` (`*`, `**`, `?`, `!`), รองรับ `RUN --mount=type=bind` และ `--mount=type=ssh`, วอร์มแคชด้วย `--cache-from` และสร้าง+push Multi-Arch OCI Image Index (`--platform linux/amd64,linux/arm64 --push`) | Done |
| **9.4 Docker Compose v2 Production Parity (`secrets`, `configs`, `deploy.replicas`/`--scale`, `include`, `watch`)** | `coral-daemon` (`compose/{model,yaml,yaml_props,deps}.rs`) / `coral-cli` (`daemon_client/compose/{up,watch}.rs`) | รองรับ top-level `secrets:` (`/run/secrets/<name>:ro`), `configs:`, `include:`, `deploy.replicas` + `--scale web=N` (แชร์ alias อัตโนมัติ) และ `coral compose watch` (auto-rebuild เมื่อโค้ดเปลี่ยนโดยเคารพ `.dockerignore`) | Done |
| **9.5 Live Container Lifecycle Controls & Native ARM64 E2E CI** | `coral-cgroups` (`manager.rs`) / `coral-daemon` (`service/lifecycle_ext.rs`) / `coral-cli` (`args/container.rs`, `daemon_client/containers/lifecycle_ext.rs`) / `scripts/setup-rootfs.sh` | เพิ่ม `coral pause` / `coral unpause` (`cgroup.freeze` + fallback `SIGSTOP`/`SIGCONT`), `coral rename`, `coral update --memory --cpus --pids-limit` แบบ live และแก้ `setup-rootfs.sh` ให้ตรวจ `uname -m` โหลด Alpine `aarch64` บน ARM64 CI | Done |

---

## Phase 10 — Userspace L3 Outbound NAT, Coral Desktop GUI Parity & Docs Refresh (`v2.7.0`)

| ฟีเจอร์ | Crates / Modules | รายละเอียด | สถานะ |
| :--- | :--- | :--- | :--- |
| **10.1 Pure-Rust Userspace L3 Outbound NAT (`UDP + TCP`) ใน `RootlessVSwitch`** | `coral-net` (`vswitch_nat.rs`, `vswitch_gw.rs`, `vswitch_tap.rs`, `vswitch.rs`) | เพิ่ม default route (`0.0.0.0/0 via 10.89.x.1`) ใน container netns และ Userspace UDP/TCP NAT proxy ฝังใน `RootlessVSwitch` ส่งออกทราฟฟิก external IPv4 และ `10.89.x.254` (`host.coral.internal` -> host `127.0.0.1`) โดยไม่ต้องพึ่ง root หรือ `pasta`/`slirp4netns` | Done |
| **10.2 Coral Desktop v2.7.0 GUI Parity** | `desktop` (`cri/containers/mod.rs`, `cri/images.rs`, `partials/dialogs/image.html`, `scripts/features/containers/{actions,table}.js`, `scripts/features/images.js`) | เพิ่มปุ่ม **Pause / Resume**, **Rename**, **Live Update Limits (RAM/CPU)** บนเมนูคอนเทนเนอร์, แสดงป้าย `paused` และ IP ส่วนตัว `10.89.x.Y` ของ virtual switch พร้อมช่องเลือก **`--platform`** (`linux/amd64`/`linux/arm64`) ในหน้า Pull & Build Image | Done |
| **10.3 GitHub Pages Docs Refresh (`v2.7.0`)** | `docs/` (`partials/sections/features.html`, `i18n/features-{en,th}.js`) | อัปเดตการ์ดและตารางคำสั่งหน้าเว็บภาษาไทย/อังกฤษให้ครอบคลุมความสามารถทั้งหมดของ `v2.5.0–v2.7.0` | Done |

---

## Phase 11 — Built-in CVE Scanner, Upstream & Round-Robin DNS, Observability CLI & Native Desktop Modals (`v2.8.0`)

| ฟีเจอร์ | Crates / Modules | รายละเอียด | สถานะ |
| :--- | :--- | :--- | :--- |
| **11.1 Built-in CVE Scanner (`coral scan`)** | `coral-daemon` (`images/security/scan.rs`, `service/nextgen/image_ops.rs`) / `coral-cli` (`args/misc.rs`, `dispatch/nextgen.rs`) | สแกนช่องโหว่อิมเมจจาก SPDX SBOM (Alpine APK / Debian DPKG) ผ่าน **OSV.dev Batch API (`api.osv.dev/v1/querybatch`)** รองรับ `--json`, `--offline` และ CI exit gate `--fail-on critical\|high\|medium\|low` พร้อมปุ่มสแกนใน Coral Desktop | Done |
| **11.2 Upstream DNS Forwarding + Multi-IP Round-Robin DNS** | `coral-net` (`dns_server.rs`) / `coral-daemon` (`service/vswitch_net.rs`) | รองรับการผูกหลาย IP ต่อชื่อบริการ (`add_host_ip`) พร้อมหมุนเวียน Round-Robin อัตโนมัติสำหรับ `--scale` replicas และส่งต่อโดเมนภายนอกไปยัง Upstream DNS ของโฮสต์ (`/etc/resolv.conf`) จาก `10.89.x.1:53` | Done |
| **11.3 Observability & Sync CLI (`coral top`, `coral wait`, `coral stats --all`)** | `coral-daemon` (`service/lifecycle_ext.rs`, `service/docker_compat/containers.rs`) / `coral-cli` (`daemon_client/containers/{lifecycle_ext,stats}.rs`) | เพิ่ม `coral top <ctr>` + `GET /containers/{id}/top` อ่านโปรเซสภายใน PID namespace ตรงจาก `/proc`, `coral wait <ctr>` รอคอนเทนเนอร์จบและคืน exit code จริง, และ `coral stats --all` แสดงตาราง CPU/MEM/PIDs ทุกคอนเทนเนอร์พร้อมกัน | Done |
| **11.4 Coral Desktop v2.8.0 Native Modals & CVE UI** | `desktop` (`partials/dialogs/container_manage.html`, `scripts/features/containers/actions.js`, `scripts/features/nextgen.js`, `cri/{containers/mod,nextgen}.rs`) | แทนที่ `window.prompt` ด้วย Native `<dialog>` Modals (EN/ไทย) สำหรับ **Rename**, **Live Update Limits (RAM/CPU/PIDs)** และ **Processes (`top`)** พร้อมปุ่ม **Scan CVEs (OSV.dev)** ในแท็บ Security | Done |
| **11.5 Docker-Style Official Website Redesign (`v2.8.1`)** | `docs/` (`styles/{base,layout,hero-cards,support,responsive}.css`, `partials/{topbar,shell,footer}.html`, `partials/sections/overview.html`, `nav.js`) | ออกแบบหน้าเว็บ GitHub Pages ใหม่สไตล์ Docker: Announcement Ribbon, Responsive Popover Navbar (`ResizeObserver` + `anchor()`), Full-Bleed Dark Hero + Interactive 4-Tab CLI Terminal, Workflow Pillars (Build/Share/Run/Verify), Sticky Section Jump Bar และ Multi-Column Dark Footer รองรับ EN/ไทย และ Light/Dark Mode | Done |

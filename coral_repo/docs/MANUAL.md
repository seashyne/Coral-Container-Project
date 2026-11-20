# คู่มือการใช้งานและพัฒนา Coral Container Runtime (v1.0 Manual)

คู่มือนี้อธิบายขั้นตอนการ Build, การเตรียม Alpine Rootfs, การใช้งาน CLI (`coral`), การรัน CRI gRPC Daemon (`coral-daemon`), และการรันชุดทดสอบอัตโนมัติ (Quality Gates)

---

## 1. การเตรียมสภาพแวดล้อมและ Build โปรเจกต์

Coral ใช้ `protoc-bin-vendored` ในขั้นตอนการคอมไพล์ Protobuf จึง**ไม่จำเป็นต้องติดตั้ง `protobuf-compiler` ด้วย `sudo`** บนเครื่องโฮสต์

```bash
cd "/home/seashyne/Documents/bluecoral project/coral_repo"

# 1. ดาวน์โหลดและแตกไฟล์ Alpine Linux 3.20 minirootfs ลงใน ./rootfs (หากยังไม่มี)
./scripts/setup-rootfs.sh

# 2. คอมไพล์ทุก Crate ใน Workspace
cargo build --workspace
```

Binary ที่ได้จะอยู่ที่:
- `target/debug/coral` — OCI Container Runtime CLI
- `target/debug/coral-daemon` — Kubernetes CRI v1 gRPC Daemon

---

## 2. การใช้งาน `coral` CLI (OCI Runtime)

Coral รองรับทั้งการรันแบบ **Rootless** (ผู้ใช้ทั่วไป ไม่ต้องใช้ `sudo` โดยอาศัย Linux User Namespace) และการรันแบบ **Root** (`sudo`)

### 2.1 สร้างไฟล์มาตรฐาน OCI `config.json`
```bash
./target/debug/coral spec --rootless --rootfs rootfs --output config.json
```

### 2.2 รันคอนเทนเนอร์แบบ Foreground (`run`)
```bash
./target/debug/coral run --bundle . my-alpine-box
```

### 2.3 การแยกขั้นตอน `create` -> `state` -> `start` -> `list` -> `delete` (Split OCI Lifecycle)
```bash
# 1. สร้างคอนเทนเนอร์ในสถานะ 'created' (รอคำสั่ง start บน exec.fifo)
./target/debug/coral create --bundle . demo-1

# 2. ตรวจสอบสถานะตามมาตรฐาน OCI state.json
./target/debug/coral state demo-1

# 3. แสดงรายการคอนเทนเนอร์ทั้งหมด
./target/debug/coral list

# 4. สั่งให้คอนเทนเนอร์เริ่มรันโปรเซสจริง
./target/debug/coral start demo-1

# 5. อ่านค่าการใช้ทรัพยากรจาก Cgroups v2 (Memory / CPU / PIDs)
./target/debug/coral stats demo-1

# 6. ลบคอนเทนเนอร์เมื่อทำงานเสร็จ (หรือใส่ --force เพื่อบังคับหยุดและลบ)
./target/debug/coral delete --force demo-1
```

### 2.4 สร้างไฟล์คอนฟิกเครือข่ายมาตรฐาน CNI v1.0.0
```bash
./target/debug/coral cni-config
```

### 2.5 จัดการ Linux Base Images และรันคอนเทนเนอร์จาก Image (`catalog` / `pull` / `images` / `run --image` / `rmi`)
รองรับการดึง OCI Image จาก Docker Hub, Red Hat Registry, GHCR, หรือสร้าง `scratch` ได้ทั้งแบบทำงานร่วมกับ `coral-daemon` (เมื่อ daemon เปิดอยู่) และแบบ standalone (เก็บไว้ที่ `~/.local/state/coral/images` หรือปรับด้วย `--images-dir` / `CORAL_IMAGES_DIR`):

```bash
# ดูรายการ Linux Base Images ยอดนิยมทั้ง 7 ตัว พร้อมสถานะการติดตั้ง
./target/debug/coral catalog

# ดึง Base Image (เลือกได้ทั้ง Alpine, Debian Slim, Ubuntu, BusyBox, Rocky Linux, Red Hat UBI, scratch)
./target/debug/coral pull debian:bookworm-slim
./target/debug/coral pull ubuntu:24.04
./target/debug/coral pull busybox:musl
./target/debug/coral pull rockylinux:9-minimal
./target/debug/coral pull registry.access.redhat.com/ubi9/ubi-minimal:latest
./target/debug/coral pull scratch

# หรือ register โฟลเดอร์ rootfs ที่มีอยู่แล้วแบบออฟไลน์
./target/debug/coral pull my-alpine:local --rootfs ./rootfs

# ดูรายการ images ที่ติดตั้งและขนาดบนดิสก์
./target/debug/coral images

# รันคอนเทนเนอร์โดยตรงจาก image (เปิด OverlayFS CoW + DNS + Seccomp-BPF + Cap Drop เป็นค่าเริ่มต้น)
./target/debug/coral run --image debian:bookworm-slim -e HELLO=coral -m 128 --cpu 50 -- /bin/sh -c 'echo "$HELLO" && cat /etc/os-release'

# เข้า shell แบบโต้ตอบ (-it), bind volume หรือ named volume (-v), และ map port (-p)
./target/debug/coral run -it --image alpine:3.20 -v "$PWD:/workspace:ro" -p 8080:8000 -- /bin/sh

# รันในโหมด Isolated Bridge NetNS (แยก CLONE_NEWNET ต่อคอนเทนเนอร์ + Cross-NetNS Port Forwarding + Custom DNS/Hosts)
./target/debug/coral run --network bridge -p 8081:80 --dns 1.1.1.1 --add-host db.internal:10.0.0.5 --image alpine:3.20 -- /bin/sh

# รันแบบตัดขาดเครือข่าย (air-gapped loopback only)
./target/debug/coral run --network none --image busybox:musl -- /bin/sh -c "ip addr || cat /proc/net/dev"

# จัดการ Named Volumes (`~/.local/state/coral/volumes/<name>/_data`)
./target/debug/coral volume create app-data
./target/debug/coral volume ls
./target/debug/coral volume inspect app-data
./target/debug/coral run --image alpine:3.20 -v app-data:/data -- /bin/sh -c "echo hello > /data/msg.txt"
./target/debug/coral volume rm app-data

# ตรวจสอบไฟล์ที่เปลี่ยนแปลงในชั้น OverlayFS CoW (`A` เพิ่ม, `C` แก้ไข, `D` ลบ) และ Commit เป็น Image ใหม่
./target/debug/coral diff <CONTAINER_ID>
./target/debug/coral commit <CONTAINER_ID> my-custom-alpine:v1

# Build Image ใหม่จาก Dockerfile พร้อม Step Cache (`[CACHED]`, หรือใช้ `--no-cache`)
./target/debug/coral build -t my-service:v1 -f Dockerfile .

# Tag / Export (.tar, .tar.gz, .tar.zst) / Import Image แบบออฟไลน์ หรือ Login & Push ขึ้น OCI Registry
./target/debug/coral tag my-service:v1 ghcr.io/your-user/my-service:v1
./target/debug/coral login ghcr.io -u your-user --password-stdin < token.txt
./target/debug/coral push ghcr.io/your-user/my-service:v1
./target/debug/coral logout ghcr.io
./target/debug/coral save -o my-service.tar.zst my-service:v1
./target/debug/coral load -i my-service.tar.zst

# สร้างและจัดการ User-Defined Networks (รองรับ sibling hostname resolution + rootless egress เมื่อมี pasta/slirp4netns)
./target/debug/coral network create backend
./target/debug/coral network ls
./target/debug/coral run -d --network backend --name db --image alpine:3.20 -- /bin/sh -c "sleep 60"
./target/debug/coral network rm backend

# รัน WebAssembly (Wasm + WASI Preview 1) ระดับไมโครวินาทีโดยไม่ต้องมี Linux rootfs
./target/debug/coral wasm run ./hello.wat --fuel 5000000 --memory 16 --json
./target/debug/coral wasm pack ./hello.wat -t hello-wasm:v1
./target/debug/coral run --runtime wasm --image hello-wasm:v1

# ทดสอบ Native Linux /dev/kvm 64-bit MicroVM และรันโค้ดที่ไม่ไว้ใจใน AI Agent Sandbox
./target/debug/coral vm probe
./target/debug/coral sandbox run --image alpine:3.20 --timeout 5 --memory 64 --json -- /bin/sh -c "id && uname -a"

# จัดการ Multi-Container Stack ด้วย Compose (`coral-compose.yml` หรือ `docker-compose.yml`)
./target/debug/coral compose up -d
./target/debug/coral compose ps
./target/debug/coral compose logs
./target/debug/coral compose down -v

# เข้า Interactive Shell ของคอนเทนเนอร์ที่กำลังรันอยู่จริงด้วย setns() (ตรวจจับทั้ง local และ daemon อัตโนมัติ)
./target/debug/coral exec -it <CONTAINER_ID> -- /bin/sh

# ลบ image และโฟลเดอร์ rootfs ที่ Coral จัดการ
./target/debug/coral rmi debian:bookworm-slim
```

### 2.6 จัดการคอนเทนเนอร์และ Pod ผ่าน `coral-daemon` (CRI v1)
```bash
# รันคอนเทนเนอร์ผ่าน coral-daemon พร้อมนโยบาย Auto-Restart และ Healthcheck (จะเห็นใน Coral Desktop และ `coral ps` ทันที)
./target/debug/coral run -d --image alpine:3.20 --name demo \
  --restart on-failure:3 --health-cmd "test -f /tmp/ready" --health-interval 5 \
  -- /bin/sh -c "touch /tmp/ready && echo hi && sleep 30"

# ดูรายการคอนเทนเนอร์ (พร้อมสถานะ health) / ติดตาม log แบบเรียลไทม์ / ดูกราฟและสถิติ CPU & RAM สด
./target/debug/coral ps
./target/debug/coral inspect <CONTAINER_ID>
./target/debug/coral cp ./app.conf <CONTAINER_ID>:/etc/app.conf
./target/debug/coral cp <CONTAINER_ID>:/etc/app.conf ./downloaded.conf
./target/debug/coral logs <CONTAINER_ID> --status --tail 50 -f
./target/debug/coral stats <CONTAINER_ID> --watch --interval 1000
./target/debug/coral exec <CONTAINER_ID> -- /bin/cat /etc/alpine-release
./target/debug/coral stop <CONTAINER_ID>
./target/debug/coral rm <CONTAINER_ID>

# ดูพื้นที่ใช้จริงและล้างทรัพยากรที่ไม่ได้ใช้งาน
./target/debug/coral system df
./target/debug/coral system prune          # ล้าง exited containers, unused pods, build cache
./target/debug/coral system prune -a --volumes

# จัดการ Pod sandboxes
./target/debug/coral pods
./target/debug/coral pods stop <POD_ID>
./target/debug/coral pods rm <POD_ID>

# ตรวจสอบเวอร์ชัน CLI และสถานะ daemon
./target/debug/coral version
```

---

## 3. การใช้งาน `coral-daemon` (Kubernetes CRI v1 gRPC Server)

```bash
# รัน CRI v1 gRPC Server บนพอร์ต 50051 พร้อมลงทะเบียน Alpine rootfs เป็น image 'alpine:latest'
./target/debug/coral-daemon \
  --listen 127.0.0.1:50051 \
  --coral-bin ./target/debug/coral \
  --default-rootfs ./rootfs
```

ในอีก Terminal หนึ่ง สามารถใช้ `coral` ตรวจสอบเวอร์ชันผ่านโปรโตคอล CRI v1 gRPC ได้ทันที:
```bash
./target/debug/coral daemon-version --addr http://127.0.0.1:50051
# ผลลัพธ์: runtime=coral version=1.0.0 api=v1 cri=0.1.0
```

---

## 4. การตรวจสอบคุณภาพโค้ด (Quality Gates)

ก่อน Commit หรือ Push ทุกครั้ง ให้รันชุดตรวจสอบมาตรฐานของโปรเจกต์:

```bash
# รัน Unit Tests + OCI E2E Tests + CRI gRPC E2E Tests
cargo test --workspace

# ตรวจสอบความสะอาดของโค้ดด้วย Clippy (ไม่อนุญาตให้มี Warning)
cargo clippy --workspace --all-targets -- -D warnings

# หรือรันสคริปต์ตรวจสอบครบวงจรในคำสั่งเดียว
./verify.sh
```

---

## 5. ฟีเจอร์ระดับ Next-Gen & Deep-Tech (`v2.0` – `v2.2`)

### 5.1 Pure-Rust `/dev/kvm` Separate-Kernel Linux MicroVM + VirtIO-Blk & VirtIO-Net (Zero QEMU)
```bash
# ทดสอบฮาร์ดแวร์ /dev/kvm (VT-x/AMD-V) + 64-bit Long Mode self-test (~2.9 ms)
coral vm probe

# ดาวน์โหลด Guest Linux Kernel (Alpine 3.20 linux-virt) + แยก VirtIO/ext4 `.ko` modules อัตโนมัติ
coral vm setup-kernel

# สร้าง VirtIO-Blk disk image (ext4) แล้วบูต Linux 6.6 KVM พร้อม persistent storage + VirtIO-Net
coral vm disk create ./vmdata.img --size-mb 64 --ext4
coral vm boot --image alpine:3.20 --disk ./vmdata.img --disk-mount /mnt/data --net -p 8080:80 --timeout 15 -- \
  /bin/sh -c 'echo "Kernel=$(uname -r)"; ip -4 addr show eth0'

# รันโค้ดที่ไม่ไว้ใจใน AI Agent Sandbox (Air-gapped + Read-Only + Hard Process-Group Timeout)
coral sandbox run --image alpine:3.20 --timeout 5 --memory 64 --json -- /bin/sh -c 'echo "Safe PID=$$"'
```

### 5.2 WebAssembly WASI Preview 1 & Serverless HTTP (`coral-wasm`)
```bash
# รันไฟล์ .wasm / .wat ตรง ๆ พร้อมคุม CPU fuel และเพดาน RAM
coral wasm run app.wasm --fuel 5000000 --memory 32 --json

# แพ็ก .wasm เป็น OCI Image และรันผ่าน --runtime wasm
coral wasm pack app.wasm -t my-wasm:v1
coral run --runtime wasm my-wasm:v1

# เปิดเซิร์ฟเวอร์ HTTP/1.1 Serverless (สร้าง Wasm Sandbox แยกใหม่ทุก Request ใน ~89 µs)
coral wasm serve my-wasm:v1 --listen 127.0.0.1:8080 --fuel 10000000 --memory 32
```

### 5.3 Kubernetes CRI v1 Unix Domain Socket (`kubelet` / `crictl`)
```bash
# coral-daemon เปิดฟังทั้ง TCP (127.0.0.1:50051) และ Unix Socket (~/.local/state/coral/daemon/coral.sock) พร้อมกัน
coral-daemon --listen 127.0.0.1:50051 --unix-socket /run/coral/coral.sock

# ตรวจสอบสถานะ CRI v1 (RuntimeReady & NetworkReady) ผ่าน Unix Socket
coral --endpoint unix://$HOME/.local/state/coral/daemon/coral.sock cri-status --verbose
```

### 5.4 Supply-Chain Security: Ed25519 Signing & SPDX 2.3 SBOM
```bash
# สร้างคู่กุญแจ Ed25519 (~/.config/coral/cosign.key mode 0600 และ cosign.pub)
coral keygen

# ลงนาม Image ด้วย Ed25519 + SHA-256 Rootfs Merkle Tree Digest
coral sign alpine:3.20

# ตรวจสอบลายเซ็นและความถูกต้องของไฟล์ทุกไบต์ใน RootFS (แจ้งเตือนทันทีหากถูกดัดแปลง)
coral verify alpine:3.20

# สร้างรายงาน Software Bill of Materials มาตรฐาน SPDX 2.3 JSON (รองรับ APK, DPKG และ Wasm)
coral sbom alpine:3.20 -o alpine-sbom.spdx.json
```

### 5.5 Prometheus `/metrics`, Log Rotation, Auto-GC & Shell Autocomplete (`v2.2`)
```bash
# เปิด coral-daemon พร้อม Prometheus /metrics (ค่าเริ่มต้น 127.0.0.1:9108), Log Rotation และ Auto-GC
coral-daemon --metrics-listen 127.0.0.1:9108 --log-max-size-mb 10 --log-max-files 3 --auto-gc-interval-secs 300
curl -s http://127.0.0.1:9108/metrics | grep coral_

# ติดตั้ง Shell Autocomplete (รองรับ bash, zsh, fish, elvish, powershell)
coral completion bash | sudo tee /etc/bash_completion.d/coral >/dev/null
```

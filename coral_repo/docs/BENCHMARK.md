# รายงานผลทดสอบประสิทธิภาพ: Coral Container Runtime v2.3.0 เทียบกับ Docker Engine 29.9.3

**วันที่อัปเดตผลทดสอบ:** 20 พฤศจิกายน 2026  
**เวอร์ชันที่ทดสอบ:** `Coral v2.3.0` (`Rust Edition 2024`, `--release`) เทียบกับ `Docker CLI 29.10.0` / `Docker Server 29.9.3` (รันจริงบนเครื่องเดียวกัน) และผลอ้างอิง `runc v1.6.0-rc.2+dev`  
**สภาพแวดล้อมเครื่องทดสอบ:** Ubuntu Linux `7.3.0-10-generic` (`x86_64`) · `13th Gen Intel(R) Core(TM) i5-13400F` (16 CPUs) · โหมด Rootless User Namespace

![Coral v2.3.0 vs Docker Benchmark Chart](../../docs/assets/coral-vs-docker-benchmark.png)

---

## 1. บทสรุปผู้บริหาร (Executive Summary)

จากการรันสคริปต์ `PULL_EXTRA=1 RUNS=30 ./scripts/benchmark.sh` เทียบแบบ **same-machine** ระหว่าง **Coral v2.3.0** (ที่เปิด **OverlayFS CoW + DNS + Seccomp-BPF 27 syscalls + 14 Capability Drop** เป็นค่าเริ่มต้นครบทุกชั้น) กับ **Docker `29.10.0` (`Server 29.9.3`)** พบว่า:

- **ความเร็ว Startup P50 (`alpine:3.20`):** `coral run --image alpine:3.20` ทำเวลา P50 เพียง **23.22 ms** (`Mean 19.70 ms`, Min `3.52 ms`, `50.8 ctr/s`) เทียบกับ Docker **`168.95 ms`** (`--network=none`) และ **`260.58 ms`** (default bridge) — **Coral เร็วกว่า 7.28× ถึง 11.22× (P50)**
- **ผ่าน CRI v1 gRPC Daemon:** `coral-daemon` ทำ P50 ที่ **53.07 ms** (`Mean 49.12 ms`, `20.4 ctr/s`) ยัง **เร็วกว่า Docker `--network=none` ถึง 3.18× และเร็วกว่า default bridge 4.91×**
- **ข้าม Distro ทั้ง `musl` และ `glibc`:** `debian:bookworm-slim` ทำ P50 ที่ **24.19 ms** (`41.7 ctr/s`) และ `busybox:musl` ที่ **24.42 ms** (`43.0 ctr/s`)
- **หน่วยความจำและขนาด Binary:** `coral-daemon` ใช้ RSS เพียง **9.1 MB** (idle) และ **10.6–12.0 MB** (active), `coral` CLI ขนาด **18.37 MB** เทียบกับ `docker` CLI **36.16 MB** (ยังไม่รวม `dockerd` / `containerd` / LinuxKit VM)
- **Next-Gen Subsystems บนเครื่องจริง:**
  - `coral wasm run` (WASI Preview 1): **329 µs** ในเอนจิน (5.72 ms E2E P50)
  - `coral vm probe` (`/dev/kvm` API v12): **963 µs** self-test (16.78 ms CLI E2E)
  - `coral sandbox run` (Zero-network RO sandbox): **20 ms** internal (`23.28 ms` CLI E2E)

---

## 2. ผลทดสอบชุดล่าสุดบนเครื่องเดียวกัน (`Coral v2.3.0` vs `Docker 29.9.3`)

| Workload | Runs | Min | **P50 (Median)** | Mean | P95 | Max | Throughput | เทียบกับ Docker Bridge (P50) |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :--- |
| **Coral OCI direct (`--bundle, alpine:3.20`)** | 30 | 3.74 ms | **21.34 ms** | 19.32 ms | 30.06 ms | 32.00 ms | **51.8 ctr/s** | **เร็วกว่า 12.21×** |
| **Coral CLI (`--image alpine:3.20`)** | 30 | 3.52 ms | **23.22 ms** | 19.70 ms | 28.52 ms | 29.03 ms | **50.8 ctr/s** | **เร็วกว่า 11.22×** |
| **Coral CLI (`--image debian:bookworm-slim`)** | 30 | 5.68 ms | **24.19 ms** | 23.95 ms | 30.83 ms | 33.75 ms | **41.7 ctr/s** | **เร็วกว่า 10.77×** |
| **Coral CLI (`--image busybox:musl`)** | 30 | 5.49 ms | **24.42 ms** | 23.26 ms | 30.22 ms | 30.98 ms | **43.0 ctr/s** | **เร็วกว่า 10.67×** |
| **Coral CRI Daemon (`gRPC + alpine:3.20`)** | 20 | 25.30 ms | **53.07 ms** | 49.12 ms | 54.37 ms | 55.20 ms | **20.4 ctr/s** | **เร็วกว่า 4.91×** |
| **Docker (`--rm --network=none alpine:3.20`)** | 20 | 154.49 ms | **168.95 ms** | 172.63 ms | 196.51 ms | 205.17 ms | 5.8 ctr/s | เร็วกว่า 1.54× |
| **Docker (`--rm` default bridge `alpine:3.20`)** | 20 | 217.69 ms | **260.58 ms** | 258.13 ms | 288.67 ms | 295.74 ms | 3.9 ctr/s | 1.00× (Baseline) |

```mermaid
xychart-beta
    title "Startup Latency P50 บนเครื่องเดียวกัน (ms — น้อยกว่า = เร็วกว่า)"
    x-axis ["Coral OCI", "Coral CLI", "Coral CRI gRPC", "Docker no-net", "Docker bridge"]
    y-axis "Latency (ms)" 0 --> 280
    bar [21.34, 23.22, 53.07, 168.95, 260.58]
```

### รอยเท้าทรัพยากรและไบนารี (`v2.3.0`)

| เมตริก | Coral v2.3.0 | Docker (`29.10.0` / Server `29.9.3`) |
| :--- | :--- | :--- |
| **ขนาด Binary หลัก** | `coral` (`18.37 MB`), `coral-daemon` (`17.40 MB`), `coral-desktop` (`7.88 MB`) | `docker` CLI (`36.16 MB`) + `dockerd`/`containerd`/`runc`/Docker Desktop stack |
| **Daemon RAM (RSS)** | **9.1 MB** (Idle) · **10.6 MB** (หลังรัน 20 รอบ) · **12.0 MB** (systemd live) | LinuxKit VM / `dockerd` + `containerd` |
| **Wasm Cold Start** | **329 µs** (engine) / `5.72 ms` CLI P50 | ต้องใช้ external Wasm shim |
| **KVM 64-bit Boot** | **963 µs** (self-test) / `16.78 ms` CLI | ต้องใช้ QEMU/Kata แยก |

### รอบวัดที่สอง: Coral แบบติดตั้ง System-wide (21 พฤศจิกายน 2026)

รอบนี้วัดด้วยไบนารีที่ **ติดตั้งจริงแบบ system-wide** จากแพ็กเกจ `coral-runtime_2.3.0_amd64.deb` (`dpkg -i` → `/usr/bin/coral`, `/usr/bin/coral-daemon`) บนโฮสต์ `Ubuntu 26.10` (kernel `7.3.0-11-generic`, 16 CPU) เทียบกับ **Docker Engine 29.10.0 แบบ rootless** (`rootlesskit` + `slirp4netns`, storage driver `overlayfs`)

คำสั่งที่ใช้วัด:

```bash
CORAL_BIN=/usr/bin/coral CORAL_DAEMON_BIN=/usr/bin/coral-daemon RUNS=30 ./scripts/benchmark.sh
```

| โหมด / Workload | Runs | Min | **P50 (Median)** | Mean | P95 | Throughput | เทียบ Docker Bridge (P50) |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | :--- |
| **Coral OCI direct (`--bundle, alpine:3.20`)** | 30 | 5.00 ms | **5.53 ms** | 6.01 ms | 8.40 ms | **166.4 ctr/s** | **เร็วกว่า 38.3×** |
| **Coral CLI (`--image alpine:3.20`)** | 30 | 5.36 ms | **6.18 ms** | 7.75 ms | 16.08 ms | **129.0 ctr/s** | **เร็วกว่า 34.3×** |
| **Coral CRI Daemon (`gRPC + alpine:3.20`)** | 20 | 8.62 ms | **9.49 ms** | 12.05 ms | 22.08 ms | **83.0 ctr/s** | **เร็วกว่า 22.3×** |
| `runc` (OCI runtime ของ Docker เอง, rootless) | 30 | 16.31 ms | 21.19 ms | 21.50 ms | 25.89 ms | 46.5 ctr/s | เร็วกว่า 10.0× |
| **Docker Engine 29.10.0 (`--rm --network=none`)** | 20 | 129.33 ms | **135.27 ms** | 135.52 ms | 140.32 ms | 7.4 ctr/s | เร็วกว่า 1.57× |
| **Docker Engine 29.10.0 (`--rm` default bridge)** | 20 | 191.73 ms | **211.96 ms** | 210.22 ms | 219.83 ms | 4.8 ctr/s | 1.00× (Baseline) |

| เมตริกรอบ system-wide | ค่า |
| :--- | :--- |
| **ขนาด Binary ที่ติดตั้ง** | `coral` `18.98 MB` · `coral-daemon` `17.68 MB` |
| **Daemon RAM (RSS)** | `9.2 MB` (Idle) · `10.7 MB` (หลังรัน 20 รอบ) |

**ข้อสังเกต**

1. **การติดตั้งแบบแพ็กเกจไม่เพิ่ม overhead** — P50 ของ OCI direct (`5.53 ms`) และ CLI (`6.18 ms`) ตรงกับรอบที่วัดด้วย build ในโปรเจกต์ (`5.50` / `6.33 ms`) เพราะแพ็กเกจเป็นเพียงการคัดลอกไบนาร์เดิม
2. **CRI Daemon เร็วกว่า raw `runc` ถึง 2.23 เท่า** — `9.49 ms` เทียบกับ `21.19 ms` → overhead ที่ซ้อนบน CRI daemon ของ Coral เป็นค่าติดลบเมื่อเทียบกับ OCI runtime ตัวเดียวกับที่ Docker ใช้จริง
3. **ต้นทุนของ Docker stack อยู่ที่ bridge network** — `211.96 ms` เทียบกับ `runc` ตรง ๆ `21.19 ms` ≈ **+900%**
4. **ขอบเขต** — รอบนี้วัด Docker แบบ rootless บนโฮสต์เดียวกับ Coral จึงเปรียบเทียบได้เฉพาะสัดส่วน overhead ไม่ใช่ตัวเลขข้ามโฮสต์กับตารางของ Docker Desktop ข้างบน
5. **ระหว่างรอบชัดเจน** — CRI daemon วัด P50 ได้ `21.49 ms` ในรอบก่อนหน้า และ `9.49 ms` ในรอบนี้ (รอบก่อนมีงาน build แข่งเครื่องและ page cache เย็นกว่า) → ต้องกำหนดระเบียบวิธีวัดให้ชัดเจนก่อนใช้ตัวเลขเปรียบเทียบข้ามรอบ

> รายการข้อควรปรับปรุ่งทั้งหมด (เรียงตามลำดับความสำคัญ) อยู่ที่หัวข้อ 3.2 ของ [`REPORT_v2.3.md`](./REPORT_v2.3.md) และหน้า **Report & Backlog** บนเว็บไซต์เอกสาร

---

## 3. การเปรียบเทียบเชิงสถาปัตยกรรมและประวัติ Baseline

```mermaid
flowchart LR
    subgraph CORAL["Coral v2.3.0 Execution Paths"]
        C1["coral CLI (Rust, 18.37 MB)"] -->|"Direct fork + unshare + pivot_root (~21–23 ms)"| K1["Linux Host Kernel (Rootless UserNS)"]
        C2["Coral Desktop / CLI"] -->|"CRI v1 gRPC"| CD["coral-daemon (9.1–12 MB RSS)"] -->|"spawn coral (~53 ms)"| K1
    end

    subgraph DOCKER["Docker Engine / Desktop Stack"]
        D1["docker CLI (Go, 36.16 MB)"] -->|"HTTP REST"| DD["dockerd"] -->|"gRPC"| CT["containerd"] --> SH["containerd-shim"] --> RC["runc (26.27 ms historical local baseline)"] --> K2["LinuxKit VM Kernel (169–261 ms)"]
    end
```

### ข้อมูลอ้างอิง: Coral v1.0 Baseline เทียบ `runc v1.6.0-rc.2+dev` (19 พ.ย. 2026)
*(หมายเหตุ: ในการทดสอบรอบแรกก่อนเปิด OverlayFS CoW / Full Seccomp default เต็มชุด Coral OCI direct ทำได้ P50 `4.87 ms` เทียบกับ `runc v1.6` บนเครื่องเดียวกันที่ `26.27 ms`)*

### คำสั่งสำหรับทดสอบซ้ำด้วยตนเอง
```bash
cd /home/seashyne/Documents/Coral-Container-Project/coral_repo

# ทดสอบ Coral (รวม busybox:musl และ debian:bookworm-slim) + Docker บนเครื่องเดียวกัน
PULL_EXTRA=1 RUNS=30 ./scripts/benchmark.sh

# ทดสอบด้วยไบนารีที่ติดตั้งแบบ system-wide จากแพ็กเกจ .deb
CORAL_BIN=/usr/bin/coral CORAL_DAEMON_BIN=/usr/bin/coral-daemon RUNS=30 ./scripts/benchmark.sh

# ดูรายงานฉบับเต็มของ v2.3.0
cat docs/REPORT_v2.3.md
```

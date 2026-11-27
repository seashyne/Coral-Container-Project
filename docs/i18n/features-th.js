/**
 * Coral Container Project — Thai (th) dictionary, part 2
 * * * Platform integration & deep Docker-parity features through v2.9.0.
 * * * Loaded before i18n.js, which merges this into window.CORAL_I18N.
 */
window.CORAL_I18N_TH_FEATURES = {
  "nav.features": "ฟีเจอร์เชิงแพลตฟอร์ม",
  "feat2.title": "ฟีเจอร์เชิงแพลตฟอร์มระดับลึก (v2.9.0)",
  "feat2.lead":
    "ความสามารถระดับ Production ครบชุดใน v2.9.0 เขียนด้วย Rust ล้วน ไม่ต้องติดตั้ง daemon ภายนอก และมีชุด unit/E2E test ครอบคลุมทุกข้อ",
  "feat2.cache.title": "Zero-Copy Shared Layer Store & Cache",
  "feat2.cache.desc":
    "coral pull แคชไฟล์บีบอัดใน .layer-cache/sha256/ (โหลดขนาน 4 สตรีม + ตรวจ SHA-256 สองชั้น) และแตกแต่ละ layer เพียงครั้งเดียวลง .layers/sha256/ แล้วแชร์พื้นที่ข้ามอิมเมจด้วย zero-copy hardlink พร้อม multi-lowerdir OverlayFS",
  "feat2.dns.title": "Rootless L2/L3 Virtual Switch, NAT & DNS",
  "feat2.dns.desc":
    "เครือข่าย custom รัน Pure-Rust RootlessVSwitch ผ่าน /dev/net/tun (IFF_TAP) แบบไร้ root แจก IP ส่วนตัวจริง 10.89.x.Y ข้าม netns คุยกันทุกพอร์ตโดยไม่ต้องใช้ -p พร้อม gateway 10.89.x.1 ที่ตอบ ARP, ICMP, UDP DNS แบบ Multi-IP Round-Robin + Upstream Forwarding (10.89.x.1:53) และ Userspace UDP/TCP NAT ในตัว",
  "feat2.buildkit.title": "BuildKit/Buildx Pro & Parallel Multi-Stage DAG",
  "feat2.buildkit.desc":
    "รองรับ .dockerignore, RUN <<EOF, RUN --mount=type=cache|secret|tmpfs|bind|ssh, รัน multi-stage อิสระแบบขนาน (parallel topological waves + ตัด dead stage), --platform + ตรวจ QEMU binfmt, --cache-from และ push Multi-Arch OCI Image Index (--platform linux/amd64,linux/arm64 --push)",
  "feat2.dockersock.title": "Streaming docker.sock Engine API",
  "feat2.dockersock.desc":
    "เปิด Docker Engine REST API (/v1.24–/v1.47) บน Unix socket และ TCP รองรับ HTTP 101 UPGRADED hijack สำหรับ /exec/{id}/start, chunked live logs?follow=1 & /events broadcast, PUT/GET /containers/{id}/archive, POST /build, /networks, /volumes และ /pause, /rename, /update, /top",
  "feat2.gpu.title": "CVE Scanner, NVIDIA CDI/.so, Compose v2 & Observability",
  "feat2.gpu.desc":
    "สแกนช่องโหว่ในตัวด้วย coral scan (SPDX SBOM + OSV.dev + --fail-on CI gate), coral top / wait / stats --all, CNCF CDI + ไลบรารี NVIDIA .so/nvidia-smi, Compose v2 (secrets, configs, include, --scale, watch), pause/rename/update และ coral machine (Linux/Lima VirtioFS/WSL2)",
  "feat2.usageTitle": "ใช้งานอย่างไร",
  "feat2.usageLead": "สรุปคำสั่งสำคัญของแต่ละความสามารถใน v2.9.0",
  "feat2.caption": "คำสั่งตัวอย่างสำหรับฟีเจอร์เชิงแพลตฟอร์มใน v2.9.0",
  "feat2.colCapability": "ความสามารถ",
  "feat2.colUsage": "วิธีใช้",
  "feat2.rowCache": "Zero-copy shared layer store",
  "feat2.rowDns": "Rootless L2/L3 vswitch & DNS",
  "feat2.rowBuildkit": "Multi-arch Buildx & .dockerignore",
  "feat2.rowDocker": "Streaming Docker Engine API",
  "feat2.rowGpu": "NVIDIA CDI / .so และอุปกรณ์",
  "feat2.rowAlias": "CVE Scan, Observability & Compose v2",
  "feat2.descCache": "แชร์ inode จาก .layers/sha256/ และส่ง multi-lowerdir OverlayFS อัตโนมัติ",
  "feat2.descDns": "แจก IP 10.89.x.Y + Gateway DNS 10.89.x.1:53 (Round-Robin + Upstream) & NAT",
  "feat2.descBuildkit": "Parallel DAG + .dockerignore + --mount=type=cache|secret|bind|ssh",
  "feat2.descDocker": "รองรับ Upgrade: tcp, chunked logs/events, /archive, /build และ /top",
  "feat2.descGpu": "อ่าน CNCF CDI + ฉีด libcuda.so* / nvidia-smi แบบ read-only อัตโนมัติ",
  "feat2.descAlias": "สแกน CVE (OSV.dev), coral top/wait/stats --all, pause/rename/update และ compose --scale",
  "feat2.composeLead":
    "ใช้งาน secrets, configs, include, deploy.replicas, gpus, devices, log_driver และ aliases ได้โดยตรงทั้งใน coral-compose.yml และ docker-compose.yml",
};

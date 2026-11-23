/**
 * Coral Container Project — Thai (th) dictionary
 * * * Loaded before i18n.js, which merges both locales into window.CORAL_I18N.
 */
window.CORAL_I18N_TH = {
  "nav.overview": "ภาพรวม",
  "nav.install": "ดาวน์โหลด & ติดตั้ง",
  "nav.quickstart": "ตัวอย่างคำสั่ง CLI",
  "nav.nextgen": "Wasm & KVM MicroVM",
  "nav.desktop": "Coral Desktop",
  "nav.benchmarks": "ผลทดสอบ & เปรียบเทียบ",
  "nav.report": "รายงาน & ข้อควรปรับปรุ่ง",
  "nav.architecture": "สถาปัตยกรรม 10 Crates",
  "nav.support": "สนับสนุน / โดเนท",
  "topbar.toc": "สารบัญ",
  "topbar.releases": "Releases",
  "topbar.themeDark": "เปลี่ยนเป็นธีมมืด",
  "topbar.themeLight": "เปลี่ยนเป็นธีมสว่าง",
  "hero.badge": "Release v2.8.0 · Rust Edition 2024 · x86_64 & ARM64 · Apache-2.0",
  "hero.title": "Container Runtime ยุคใหม่ด้วย Rust",
  "hero.subtitle":
    "คำสั่งสไตล์ Docker + docker.sock REST API, Shared Layer Cache + Parallel Pull, BuildKit heredoc/cache/secret mount, Embedded DNS, ส่งต่อ GPU/Device, Rootless Multi-UID OCI, Kubernetes CRI v1, WASI HTTP, KVM MicroVM และ Coral Desktop v2.8.0",
  "hero.ctaInstall": "ดาวน์โหลด & ติดตั้ง v2.8.0",
  "hero.ctaBench": "ผลเทียบกับ Docker & รายงาน",
  "hero.ctaGithub": "ดูซอร์สโค้ดบน GitHub",
  "hero.ctaSupport": "สนับสนุนโปรเจกต์",
  "metric.startLabel": "OCI Container P50 Startup",
  "metric.startSub": "เร็วกว่า Docker 17.9×–26.8× บนเครื่องเดียวกัน",
  "metric.wasmLabel": "WASI WebAssembly Execution",
  "metric.wasmSub": "coral-wasm · wasmi 2.0 (Fuel + RAM cap)",
  "metric.kvmLabel": "Native /dev/kvm 64-bit Boot",
  "metric.kvmSub": "coral-vmm · Pure-Rust Long Mode Self-Test",
  "metric.memLabel": "coral-daemon Idle RAM",
  "metric.memSub": "0 Clippy warnings · 10 Rust Crates",
  "overview.title": "ทำไมต้อง Coral?",
  "overview.lead":
    "Coral ออกแบบมาเพื่อแก้ปัญหาความช้าของ Runtime แบบเก่า ลดช่องโหว่ด้านหน่วยความจำด้วย Rust และเพิ่มความสามารถระดับ Next-Gen สำหรับยุค AI Agent และ Serverless WebAssembly",
  "feat.oci.title": "OCI v1.0.2 & Rootless by Default",
  "feat.oci.desc":
    "แยก Namespace ครบ (PID, UTS, IPC, Mount, Net, User), pivot_root, OverlayFS Copy-on-Write หลายชั้น, Cgroups v2, Seccomp-BPF 27 syscalls และ Capabilities drop ตั้งแต่แกะกล่อง",
  "feat.dev.title": "Workflow ครบเทียบชั้น Docker/Podman",
  "feat.dev.desc":
    "รองรับ docker.sock REST API v1.45, Shared Layer Cache + Parallel Pull, BuildKit RUN <<EOF และ --mount=type=cache|secret, Embedded UDP DNS + --network-alias, --gpus/--device, --log-driver (json-file/syslog/journald), coral compose และ Boot Auto-Resume",
  "feat.wasm.title": "WebAssembly Runtime (coral-wasm)",
  "feat.wasm.desc":
    "รัน .wasm / .wat และ OCI Wasm Image ตรง ๆ ผ่าน WASI Preview 1 ในเวลาเพียง 326 µs ไม่ต้องพก Linux rootfs คุม CPU ด้วย Fuel Metering และเพดาน RAM แบบ Deterministic",
  "feat.vmm.title": "KVM MicroVM & AI Sandbox (coral-vmm)",
  "feat.vmm.desc":
    "Pure-Rust Hypervisor คุยกับ /dev/kvm โดยตรง บูต 64-bit Long Mode ใน 2.94 ms พร้อมโหมด AI Agent Sandbox ตัดเน็ต Read-Only และสังหาร Process Group ทันทีเมื่อหมดเวลา (รองรับเฉพาะเครื่อง x86_64)",
  "install.title": "ดาวน์โหลด & วิธีติดตั้ง",
  "install.lead":
    "เลือกวิธีติดตั้งที่เหมาะกับระบบของคุณ — รองรับ Linux x86_64 (Rootless-first) และ Windows ผ่าน WSL2 + Coral Desktop",
  "install.tabOneliner": "One-Liner (Linux)",
  "install.tabDeb": "Debian / Ubuntu (.deb)",
  "install.tabDesktop": "Coral Desktop (GUI)",
  "install.tabSource": "Build จากซอร์ส",
  "install.onelinerDesc":
    "ติดตั้ง coral CLI, coral-daemon, Alpine 3.20 base rootfs และเปิดใช้งาน systemd user service อัตโนมัติ (ตรวจสอบ SHA-256 ให้อัตโนมัติ):",
  "install.onelinerVerify": "ตรวจสอบหลังติดตั้ง:",
  "install.debDesc":
    "เหมาะสำหรับ Ubuntu 22.04+ / Debian 12+ ติดตั้งไฟล์ไบนารีเข้า /usr/bin และยูนิต systemd โดยตรง:",
  "install.downloadDeb": "ดาวน์โหลด coral-runtime_2.8.0_amd64.deb",
  "install.downloadTar": "ดาวน์โหลด coral-v2.8.0-linux-x86_64.tar.gz",
  "install.allReleases": "ดูไฟล์ทั้งหมดบน GitHub Releases",
  "install.desktopDesc":
    "จัดการคอนเทนเนอร์, Pods, Images, Volumes, Networks, GPU/Device Passthrough, Full-Duplex PTY Terminal และแท็บ Next-Gen (Wasm & KVM) ผ่านแอป Desktop (Tauri v2.8.0):",
  "install.desktopLinuxTitle": "Linux (Ubuntu / Fedora / AppImage)",
  "install.desktopLinuxStep":
    "ดาวน์โหลด Coral Desktop_2.8.0_amd64.deb หรือ .rpm จาก GitHub Releases หรือติดตั้งจากซอร์สลง ~/.local/bin เพื่อเชื่อมต่อกับ coral-daemon อัตโนมัติ",
  "install.desktopWinTitle": "Windows 10 / 11 (WSL2)",
  "install.desktopWinStep":
    "ติดตั้ง Coral Runtime ภายใน WSL2 Linux distro ด้วยคำสั่ง One-Liner จากนั้นติดตั้ง Coral Desktop (.msi / .exe) บน Windows แล้วเลือกโหมด WSL2 ในหน้า Settings",
  "install.sourceDesc":
    "ต้องการ Rust Toolchain (stable, Edition 2024) — ไม่จำเป็นต้องติดตั้ง protoc แยก:",
  "install.prereqTitle": "ข้อกำหนดระบบ (Prerequisites)",
  "install.prereqList":
    "Linux Kernel 5.15+ (รองรับ Cgroups v2 & User Namespaces) · แนะนำติดตั้ง slirp4netns หรือ passt สำหรับออกอินเทอร์เน็ตแบบ Rootless · /dev/kvm (Intel VT-x / AMD-V) สำหรับโหมด Hardware MicroVM",
  "cli.title": "คู่มือและตัวอย่างคำสั่ง CLI",
  "cli.lead":
    "คำสั่งคุ้นมือสไตล์ Docker (ไม่ต้องพิมพ์ --image หรือ --) พร้อมคำสั่ง Next-Gen สำหรับ WebAssembly, KVM MicroVM และ AI Agent Sandbox",
  "cli.guideCaption": "รูปคู่มือสรุปขั้นตอนการติดตั้งและใช้งาน Coral CLI + Coral Desktop v2.8.0 ในภาพเดียว",
  "cli.tabContainers": "Containers & Exec",
  "cli.tabBuild": "Build, Compose & Push",
  "cli.tabWasm": "WebAssembly (Wasm)",
  "cli.tabVmm": "KVM MicroVM & Sandbox",
  "nextgen.title": "เจาะลึกฟีเจอร์ Next-Gen (v2.8.0)",
  "nextgen.lead":
    "สองเอนจินใหม่ที่ทำให้ Coral แตกต่างจาก Container Runtime ทั่วไป — สร้างมาเพื่อ AI Agent Sandboxing และ Microsecond Serverless",
  "nextgen.wasmCardTitle": "coral-wasm — Microsecond WASI Preview 1 Engine",
  "nextgen.wasmBullet1": "รันไฟล์ .wasm และ .wat ตรง ๆ ด้วย wasmi 2.0 ไม่ต้องสร้าง Linux rootfs",
  "nextgen.wasmBullet2": "คุม CPU ด้วย --fuel หยุด infinite loop แบบ deterministic 100%",
  "nextgen.wasmBullet3": "คุมเพดาน Linear Memory ด้วย --memory-mb ผ่าน wasmi StoreLimits",
  "nextgen.wasmBullet4": "แพ็กเป็น OCI Image ด้วย coral wasm pack และรันผ่าน coral run --runtime wasm",
  "nextgen.vmmCardTitle": "coral-vmm — Native /dev/kvm Hypervisor & AI Sandbox",
  "nextgen.vmmBullet1": "คุยกับ Linux /dev/kvm ioctl โดยตรงด้วย Pure Rust (CPUID, 4-level Paging, COM1 16550 UART)",
  "nextgen.vmmBullet2": "จำกัดเวลา wall-clock และ VM-exit budget ป้องกันโค้ดแปลกปลอมยึด vCPU",
  "nextgen.vmmBullet3": "สร้าง SVR4 newc initramfs + ตรวจสอบ Linux bzImage (HdrS) สำหรับบูตแยกเคอร์เนลเต็มรูปแบบ",
  "nextgen.vmmBullet4": "AI Agent Sandbox: --network none + Read-Only RootFS + Seccomp-BPF + ตัด Process Group ทั้งยวงเมื่อหมดเวลา",
  "desktop.title": "Coral Desktop (Tauri v2)",
  "desktop.lead":
    "แอป GUI น้ำหนักเบาที่คุยกับ coral-daemon ผ่าน CRI v1 gRPC โดยตรง รองรับทั้ง Local Linux, WSL2 และ Remote Host",
  "desktop.f1Title": "Full-Duplex Streaming PTY Terminal",
  "desktop.f1Desc": "เปิดเชลล์โต้ตอบจริงผ่าน gRPC ExecStream พร้อม resize หน้าต่างแบบเรียลไทม์",
  "desktop.f2Title": "Live Metrics & CoW Inspector",
  "desktop.f2Desc": "กราฟ SVG Sparklines แสดง CPU / RAM สด ๆ พร้อมตรวจสอบไฟล์ OverlayFS diff และสั่ง Commit ได้ในคลิกเดียว",
  "desktop.f3Title": "Quick Fill + Inline Quick Pull",
  "desktop.f3Desc": "เติมค่า Nginx / Redis / Postgres / Alpine / Minecraft PaperMC ในคลิกเดียว พร้อมช่อง Volumes & Env ในหน้าแรกและดึงอิมเมจจาก Discover ได้ทันที",
  "desktop.f4Title": "Theme Popover 5 ชุดสี (EN / ไทย)",
  "desktop.f4Desc": "สลับ System / Light / Dark, ชุดสี Coral / Ocean / Emerald / Indigo / Amber, Density และภาษา EN/ไทย ได้จากปุ่ม Theme บน Topbar ทุกหน้า",
  "bench.title": "ผลทดสอบจริง & ตารางเปรียบเทียบกับ Docker",
  "bench.lead":
    "ทดสอบจริงบนเครื่องเดียวกัน Linux x86_64 (Intel Core i5-13400F, 16 CPUs) ระหว่าง Coral v2.3.0 กับ Docker 29.10.0 / Server 29.9.3",
  "bench.chartCaption": "ผลวัดความเร็วเริ่มต้นคอนเทนเนอร์และทรัพยากรจริงแบบควบคุมสภาพแวดล้อม (Coral v2.3.0 vs Docker Engine 29.10.0 rootless)",
  "bench.reportBtn": "อ่านรายงานสรุปฉบับเต็ม (REPORT_v2.3.md)",
  "bench.rawBtn": "ดูข้อมูล Benchmark ละเอียด (BENCHMARK.md)",
  "bench.caption": "ตารางสรุปผลวัดความเร็วแบบควบคุมสภาพแวดล้อมและสถาปัตยกรรม (5 รอบ × 30 runs · median-of-medians · alpine / busybox / debian)",
  "bench.colFeature": "หัวข้อ / ตัวชี้วัดจริง",
  "bench.colCoral": "Coral CLI v2.3.0",
  "bench.colRunc": "Coral Daemon (CRI v1)",
  "bench.colDocker": "Docker 29.10 / 29.9.3",
  "bench.r1": "Startup P50 Latency (alpine:3.20)",
  "bench.r1c": "8.88 ms (Min 7.14 ms · 96.7 ctr/s)",
  "bench.r1r": "28.57 ms (Mean 35.61 ms · 28.1 ctr/s)",
  "bench.r1d": "159.32 ms (no-net) · 237.56 ms (bridge)",
  "bench.r2": "ขนาด Binary & Daemon RAM",
  "bench.r2c": "18.98 MB CLI (Rust Zero-GC)",
  "bench.r2r": "8.4 MB idle / 15.0 MB หลัง 5 รอบ RSS",
  "bench.r2d": "36.16 MB CLI + dockerd/containerd VM",
  "bench.r3": "WASI WebAssembly Cold Start",
  "bench.r3c": "329 µs ในเอนจิน (5.72 ms CLI P50)",
  "bench.r3r": "รองรับผ่าน coral run --runtime wasm",
  "bench.r3d": "ต้องติดตั้ง Wasm shim เสริม",
  "bench.r4": "Native /dev/kvm 64-bit MicroVM",
  "bench.r4c": "963 µs self-test (Zero QEMU)",
  "bench.r4r": "พร้อม VirtIO-Blk & VirtIO-Net ในตัว",
  "bench.r4d": "ไม่มีในตัว (ต้องใช้ Kata/VM แยก)",
  "bench.r5": "AI Code Sandbox (ตัดเน็ต + RO)",
  "bench.r5c": "20 ms internal (23.28 ms CLI)",
  "bench.r5r": "คุมเวลา Process Group SIGKILL",
  "bench.r5d": "ต้องเขียนสคริปต์ครอบเอง",
  "bench.r6": "Desktop GUI & Theme System",
  "bench.r6c": "Tauri 7.88 MB · 5 Palettes · TH/EN",
  "bench.r6r": "เชื่อม Local / WSL2 / Remote CRI",
  "bench.r6d": "Docker Desktop (Electron)",
  "arch.title": "สถาปัตยกรรม 10 Rust Crates",
  "arch.lead":
    "ออกแบบแยกโมดูลชัดเจนตามหลัก Separation of Concerns ทุก Crate ผ่าน cargo clippy -- -D warnings แบบ 0 warnings",
  "arch.caption": "รายการ 10 Crates ภายใน Workspace coral_repo",
  "arch.colCrate": "Crate",
  "arch.colPath": "โฟลเดอร์",
  "arch.colRole": "หน้าที่หลัก",
  "support.title": "สนับสนุนโปรเจกต์ (Donate & Sponsor)",
  "support.lead":
    "Coral เป็นซอฟต์แวร์ Open-Source ภายใต้ Apache-2.0 หากโปรเจกต์นี้ช่วยงานหรือสร้างแรงบันดาลใจให้คุณ สามารถร่วมสนับสนุนค่ากาแฟและค่าเซิร์ฟเวอร์ทดสอบได้ที่ช่องทางด้านล่าง",
  "support.templateBadge": "เทมเพลตพร้อมตั้งค่า",
  "support.activeBadge": "พร้อมรับการสนับสนุน",
  "support.ppTitle": "PromptPay QR (ประเทศไทย)",
  "support.ppDesc":
    "สแกน QR Code โอนผ่านแอปธนาคารไทยทุกธนาคาร ไม่มีค่าธรรมเนียม (รองรับทั้งโอนครั้งเดียวตามสะดวก)",
  "support.ppBtn": "เปิดหน้าต่าง PromptPay QR",
  "support.ghTitle": "GitHub Sponsors",
  "support.ghDesc":
    "สนับสนุนรายเดือนหรือครั้งเดียวผ่าน GitHub โดยตรง พร้อมรับป้าย Sponsor บนโปรไฟล์ GitHub",
  "support.ghBtn": "ไปที่ GitHub Sponsors",
  "support.bmcTitle": "Buy Me a Coffee / Ko-fi",
  "support.bmcDesc":
    "เลี้ยงกาแฟนักพัฒนาได้ง่าย ๆ ด้วยบัตรเครดิต/เดบิต หรือ Apple Pay / Google Pay ไม่ต้องสมัครสมาชิก",
  "support.bmcBtn": "เลี้ยงกาแฟนักพัฒนา",
  "support.configHint":
    "สำหรับเจ้าของโปรเจกต์: แก้ไขลิงก์จริงและใส่รูป QR ได้ในไฟล์เดียวที่ docs/donate-config.js (วางรูป QR ที่ docs/assets/promptpay-qr.png แล้วเปลี่ยน configured: true)",
  "dialog.ppTitle": "สนับสนุนผ่าน PromptPay QR",
  "dialog.ppPlaceholderTitle": "ยังไม่ได้อัปโหลดรูป PromptPay QR",
  "dialog.ppPlaceholderDesc":
    "วางไฟล์รูป QR ของคุณที่ docs/assets/promptpay-qr.png แล้วตั้งค่า promptpay.qrImage และ configured: true ในไฟล์ docs/donate-config.js",
  "dialog.ppAccount": "ชื่อบัญชี:",
  "dialog.ppId": "หมายเลขพร้อมเพย์:",
  "dialog.ppHint": "กดปุ่ม Esc หรือคลิกพื้นที่ด้านนอกเพื่อปิดหน้าต่าง",
  "dialog.close": "ปิดหน้าต่าง",
  "copy.btn": "คัดลอก",
  "copy.done": "คัดลอกแล้ว!",
  "footer.copy": "Coral Container Project v2.8.0 · Open-Source ภายใต้ Apache License 2.0",
  "footer.reportLink": "รายงานสรุป (REPORT_v2.3.md)",
  "footer.docsLink": "คู่มือฉบับเต็ม (MANUAL.md)",
  "footer.roadmapLink": "Roadmap",
  "footer.issuesLink": "แจ้งบั๊ก / ขอฟีเจอร์",
  "report.title": "รายงานผลวัดล่าสุด & ข้อควรปรับปรุ่ง",
  "report.lead":
    "ผลวัดรอบล่าสุดจากแพ็กเกจ coral-runtime 2.3.0 ที่ติดตั้งแบบ system-wide บนโฮสต์จริง เทียบกับ Docker Engine 29.10.0 แบบ rootless พร้อมสรุปสิ่งที่ควรปรับปรุ่งต่อ",
  "report.envTitle": "สภาพแวดล้อมการวัด",
  "report.envDesc":
    "Ubuntu 26.10 · kernel 7.3.0-11-generic · 16 CPU (pin ที่ 8-15) · Coral 2.3.0 ติดตั้งแบบ system-wide จากแพ็กเกจ .deb (/usr/bin/coral, /usr/bin/coral-daemon) · Docker Engine 29.10.0 แบบ rootless (rootlesskit + slirp4netns, storage driver overlayfs)",
  "report.tableCaption":
    "ผลวัดรอบควบคุมสภาพแวดล้อม: 5 รอบ × 30 runs แบบ median-of-medians แยกโหมด warm (page cache อุ่น) · alpine / busybox / debian",
  "report.colWorkload": "โหมด / Workload",
  "report.colRuns": "รอบ",
  "report.colMin": "Min",
  "report.colP50": "P50",
  "report.colMean": "Mean",
  "report.colThr": "Throughput",
  "report.colVs": "เทียบ Docker bridge",
  "report.findTitle": "ข้อค้นพบสำคัญจากรอบวัดนี้",
  "report.find1Title": "ความเร็วไม่ขึ้นกับ distro ที่ใช้",
  "report.find1Desc":
    "alpine (8.88 ms), busybox (8.38 ms) และ debian:bookworm-slim (8.38 ms) ให้ผลใกล้เคียงกันภายในไม่กี่มิลลิวินาที แปลว่า overhead ของ engine มาจากการสร้างคอนเทนเนอร์ ไม่ใช่จากขนาดของ rootfs",
  "report.find2Title": "CRI daemon ยังชนะ Docker แต่แพ้ raw runc",
  "report.find2Desc":
    "เมื่อควบคุม page cache และภาระ CPU แล้ว coral-daemon ทำงานที่ 28.57 ms เทียบกับ runc ที่ 24.20 ms — ชั้น gRPC เพิ่มราว 4 ms การวัดรอบเดียวก่อนหน้าเคยรายงานว่า CRI daemon เร็วกว่า runc ถึง 2.23 เท่า ซึ่งเป็นผลจาก noise ของการวัด ไม่ใช่คุณสมบัติของ Coral — ข้อสรุปที่ยืนยันได้จริงคือ CRI daemon ยังชนะ Docker ถึง 8.3 เท่า",
  "report.find3Title": "ต้นทุนของ Docker stack อยู่ที่ bridge network",
  "report.find3Desc":
    "docker run แบบ bridge ใช้เวลา 237.56 ms เทียบกับ runc ตรง ๆ ที่ 24.20 ms คิดเป็น ~+213 ms (+881%) สำหรับ docker CLI → dockerd → containerd → shim → runc พร้อมการตั้งค่า network namespace",
  "report.find4Title": "ขอบเขตของการเทียบเทียบ",
  "report.find4Desc":
    "รอบนี้วัด Docker แบบ rootless บนโฮสต์เดียวกับ Coral จึงเปรียบเทียบได้เฉพาะสัดส่วน overhead ไม่ใช่ตัวเลขข้ามโฮสต์กับผลของ Docker Desktop ในตารางเดิม",
  "report.backlogTitle": "ข้อควรปรับปรุ่ง (Backlog)",
  "report.backlogLead":
    "รายการที่คัดจากผลวัดและจากการติดตั้งจริงบนเครื่อง เรียงตามลำดับความสำคัญ",
  "report.backlogCaption":
    "รายการข้อควรปรับปรุ่งหลังจากรอบวัดแบบ system-wide (พฤศจิกายน 2026)",
  "report.blColNo": "#",
  "report.blColArea": "หัวข้อ",
  "report.blColFinding": "สิ่งที่พบ / หลักฐาน",
  "report.blColAction": "สิ่งที่ควรทำ",
  "report.blColStatus": "สถานะ",
  "report.bl1Area": "ระเบียบวิธีวัด Benchmark",
  "report.bl1Finding":
    "CRI daemon วัด P50 ต่างกัน 2.3× ระหว่างสองรอบบนโฮสต์เดียวกัน (21.49 → 9.49 ms) เพราะยังไม่ได้ควบคุม page cache และภาระเครื่องให้เหมือนกันทุกรอบ",
  "report.bl1Action":
    "วัดหลายรอบแล้วรายงาน median-of-medians แยก cold/warm cache และ pin CPU ให้เหมือนกันทุกรอบ",
  "report.bl2Area": "เอกสารเว็บไม่ตรงกับผลวัดล่าสุด",
  "report.bl2Finding":
    "ตารางผลทดสอบบนเว็บยังแสดงเฉพาะรอบแรก (Docker Desktop บนอีกเครื่อง) ยังไม่มีผลรอบ rootless / system-wide",
  "report.bl2Action":
    "เพิ่มตารางรอบ system-wide พร้อมระบุโฮสต์และโหมดของแต่ละรอบให้ชัดเจน (ทำในรอบนี้)",
  "report.bl3Area": "systemd unit ของแพ็กเกจไม่ระบุ state dir",
  "report.bl3Finding":
    "unit ที่แพ็กเกจติดตั้งมาไม่มี --state-dir/--images-dir/--default-rootfs daemon จึงใช้ XDG_RUNTIME_DIR (tmpfs) แล้วมองไม่เห็น state เดิม ต้องเขียน drop-in เอง — แก้แล้วด้วยการชี้ไปที่ %h/.local/state/coral ซึ่งเป็นตำแหน่งแบบ persistent",
  "report.bl3Action":
    "ship ค่า default ที่ถูกต้อง หรือคำสั่ง coral-daemon --init ที่สร้าง unit พร้อมค่า state ของเครื่องให้เอง",
  "report.bl4Area": "AppImage ของ Coral Desktop bundle ไม่ผ่าน",
  "report.bl4Finding":
    "linuxdeploy เรียก plugin gtk แล้วล้ม 2 จุด: ไม่มีคำสั่ง dpkg-architecture (แพ็กเกจ dpkg-dev) และ /usr/bin/cp ของ Ubuntu 26.10 เป็น uutils coreutils ซึ่ง cp -r --parents ผิดพลาดเมื่อ source path เป็น absolute path",
  "report.bl4Action":
    "ติดตั้ง dpkg-dev และให้ PATH มองเห็น GNU cp ก่อนขั้นตอน bundle (แก้แล้วในรอบนี้)",
  "report.bl5Area": "ไม่มีการตรวจสอบหลังติดตั้งแพ็กเกจ",
  "report.bl5Finding":
    "หลัง dpkg -i ไม่มีการยืนยันว่า coral / coral-daemon ใช้งานได้จริง และ service เริ่มทำงานตามปกติ",
  "report.bl5Action":
    "เพิ่ม postinst smoke check: coral --version, coral daemon-version และ systemctl --user is-active coral-daemon",
  "report.bl6Area": "ไม่มี CI ที่วัด latency อัตโนมัติ",
  "report.bl6Finding":
    "workflow ปัจจุบันมีเฉพาะ Desktop และ Pages ยังไม่มีงานที่รัน benchmark ต่อเนื่อง",
  "report.bl6Action":
    "เพิ่ม nightly job ที่รัน scripts/benchmark.sh แล้วอัปโหลดผลเป็น artifact เพื่อจับ regression ของ startup latency",
  "report.bl7Area": "ชุดวัดยังครอบคลุมแค่ alpine:3.20",
  "report.bl7Finding":
    "รอบ system-wide วัดเฉพาะ alpine:3.20 ขณะที่รอบแรกยังครอบคลุม debian:bookworm-slim และ busybox:musl",
  "report.bl7Action":
    "เพิ่ม distro matrix ใน benchmark.sh และรันรอบควบคุมสภาพแวดล้อมด้วย PULL_EXTRA=1 จนครอบคลุม alpine, busybox และ debian พร้อมกัน (ทำในรอบนี้)",
  "report.bl8Area": "ARM64 ไม่เคยผ่าน E2E จริง",
  "report.bl8Finding":
    "CI สร้าง ARM64 artifact แต่ไม่รันชุดทดสอบ และ coral-vmm ไม่ผ่านการ compile บน ARM เลย เพราะ import และ cfg gate ไม่สอดคล้องกัน",
  "report.bl8Action":
    "เพิ่ม ARM64 เข้า verify matrix แล้วแก้บั๊กทั้งหมดจน build, clippy และ unit test ผ่านบน aarch64 (ทำในรอบนี้)",
  "report.bl9Area": "CRI daemon สูญเสีย state.json เป็นครั้งคราว",
  "report.bl9Finding":
    "รอบวัดรอบที่สองพบความล้มเหลว 2 ครั้งจาก ~300 runs ของ CRI daemon ด้วยข้อความ rename state.json: No such file or directory เกิดเมื่อ daemon จัดการ container หลายตัวพร้อมกัน",
  "report.bl9Action":
    "ใช้ชื่อไฟล์ชั่วคราวที่ไม่ซ้ำกันต่อการเขียน พร้อม fsync ก่อน rename และถือว่าการลบ container พร้อมกันเป็นเคสไม่ผิดพลาด พร้อม regression test 6 เคส (ทำในรอบนี้)",
  "report.bl10Area": "coral load ไฟล์ .tar.zst ล้มเหลวเป็นครั้งคราว",
  "report.bl10Finding":
    "CI จับได้จาก oci_features_e2e::image_tag_save_zstd_and_load_roundtrip_cli ด้วยข้อความ zstd decompression failed: (ว่าง) เพราะ tar reader หยุดที่ end-of-archive marker โดยไม่ drain pipe ทำให้ pipe ถูกปิดทั้งที่ zstd ยังเขียนข้อมูลไม่เสร็จ และตายด้วย SIGPIPE",
  "report.bl10Action":
    "ระบาย stdout ของ process ให้หมดก่อน wait และเก็บกวาด child process ฝั่ง save เมื่อเขียนไม่สำเร็จ พร้อม regression test ที่ทำให้เกิด race ได้แน่นอน (ทำในรอบนี้)",
  "report.stOpen": "ยังค้าง",
  "report.stDone": "แก้แล้วในรอบนี้",
  "report.reportBtn": "อ่านรายงานสรุปฉบับเต็ม (REPORT_v2.3.md)",
  "report.rawBtn": "ดูข้อมูล Benchmark ละเอียด (BENCHMARK.md)",
};

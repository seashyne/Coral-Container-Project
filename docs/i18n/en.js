/**
 * Coral Container Project — English (en) dictionary
 * * * Loaded before i18n.js, which merges both locales into window.CORAL_I18N.
 */
window.CORAL_I18N_EN = {
  "nav.overview": "Overview",
  "nav.install": "Download & Install",
  "nav.quickstart": "CLI Cookbook",
  "nav.nextgen": "Wasm & KVM MicroVM",
  "nav.desktop": "Coral Desktop",
  "nav.benchmarks": "Benchmarks",
  "nav.report": "Report & Backlog",
  "nav.architecture": "10-Crate Architecture",
  "nav.support": "Support / Donate",
  "topbar.toc": "On this page",
  "topbar.releases": "Releases",
  "topbar.themeDark": "Switch to dark theme",
  "topbar.themeLight": "Switch to light theme",
  "hero.badge": "Release v2.8.4 · Rust Edition 2024 · x86_64 & ARM64 · Apache-2.0",
  "hero.title": "Develop faster. Run anywhere — in Rust.",
  "hero.subtitle":
    "Docker-compatible CLI & docker.sock REST shim, parallel shared layer cache, BuildKit heredoc/cache/secret mounts, embedded DNS, GPU/device passthrough, rootless Multi-UID OCI, Kubernetes CRI v1, WASI HTTP (coral-wasm), /dev/kvm MicroVMs, and Coral Desktop v2.8.4.",
  "hero.ctaInstall": "Download & Install v2.8.4",
  "hero.ctaBench": "Docker Comparison & Report",
  "hero.ctaGithub": "View on GitHub",
  "hero.ctaSupport": "Support the Project",
  "metric.startLabel": "OCI Container P50 Startup",
  "metric.startSub": "17.9×–26.8× faster than Docker on same host",
  "metric.wasmLabel": "WASI WebAssembly Execution",
  "metric.wasmSub": "coral-wasm · wasmi 2.0 (Fuel + RAM cap)",
  "metric.kvmLabel": "Native /dev/kvm 64-bit Boot",
  "metric.kvmSub": "coral-vmm · Pure-Rust Long Mode Self-Test",
  "metric.memLabel": "coral-daemon Idle RAM",
  "metric.memSub": "0 Clippy warnings · 10 Rust Crates",
  "overview.title": "Why Coral?",
  "overview.lead":
    "Built from the ground up in Rust to eliminate GC latency and memory-safety pitfalls while adding built-in WebAssembly and KVM MicroVM sandboxing for modern workloads.",
  "feat.oci.title": "OCI v1.0.2 & Rootless by Default",
  "feat.oci.desc":
    "Full Linux namespace isolation (PID, UTS, IPC, Mount, Net, User), pivot_root, multi-lowerdir OverlayFS CoW, Cgroups v2 limits, 27-syscall Seccomp-BPF filter, and OCI Capability drop out of the box.",
  "feat.dev.title": "Docker/Podman-Class Developer UX",
  "feat.dev.desc":
    "Includes docker.sock REST API v1.45 shim, shared .layer-cache + parallel pull, BuildKit RUN <<EOF & --mount=type=cache|secret, Embedded UDP DNS + --network-alias, --gpus/--device, --log-driver (json-file/syslog/journald), coral compose, and Boot Auto-Resume.",
  "feat.wasm.title": "WebAssembly Runtime (coral-wasm)",
  "feat.wasm.desc":
    "Execute .wasm / .wat and OCI Wasm artifacts directly via WASI Preview 1 in 326 µs without a Linux rootfs, enforcing deterministic CPU fuel metering and linear memory ceilings.",
  "feat.vmm.title": "KVM MicroVM & AI Sandbox (coral-vmm)",
  "feat.vmm.desc":
    "Pure-Rust /dev/kvm 64-bit Long Mode hypervisor booting in 2.94 ms, plus a zero-network read-only AI Agent Sandbox with full process-group timeout enforcement. x86_64 hosts only.",
  "install.title": "Download & Installation",
  "install.lead":
    "Pick the method that fits your setup — native Linux x86_64 (rootless-first) and Windows via WSL2 + Coral Desktop.",
  "install.tabOneliner": "One-Liner (Linux)",
  "install.tabDeb": "Debian / Ubuntu (.deb)",
  "install.tabDesktop": "Coral Desktop (GUI)",
  "install.tabSource": "Build from Source",
  "install.onelinerDesc":
    "Installs coral CLI, coral-daemon, Alpine 3.20 base rootfs, and enables the systemd user service automatically (with SHA-256 checksum verification):",
  "install.onelinerVerify": "Verify installation:",
  "install.debDesc":
    "Recommended for Ubuntu 22.04+ and Debian 12+. Installs binaries into /usr/bin and registers the systemd user unit:",
  "install.downloadDeb": "Download coral-runtime_2.8.4_amd64.deb",
  "install.downloadTar": "Download coral-v2.8.4-linux-x86_64.tar.gz",
  "install.allReleases": "Browse all assets on GitHub Releases",
  "install.desktopDesc":
    "Manage containers, pods, images, volumes, networks, GPU/device passthrough, live streaming PTY terminals, and Next-Gen (Wasm & KVM) playgrounds in Coral Desktop v2.8.4:",
  "install.desktopLinuxTitle": "Linux (Ubuntu / Fedora / AppImage)",
  "install.desktopLinuxStep":
    "Download Coral Desktop_2.8.4_amd64.deb or .rpm from GitHub Releases or build from source into ~/.local/bin with automatic daemon discovery.",
  "install.desktopWinTitle": "Windows 10 / 11 (WSL2)",
  "install.desktopWinStep":
    "Install Coral Runtime inside your WSL2 Linux distro using the One-Liner above, install Coral Desktop (.msi / .exe) on Windows, and select WSL2 mode in Settings.",
  "install.sourceDesc":
    "Requires Rust stable toolchain (Edition 2024) — no system protoc installation needed:",
  "install.prereqTitle": "System Prerequisites",
  "install.prereqList":
    "Linux Kernel 5.15+ (Cgroups v2 & User Namespaces) · Optional slirp4netns or passt for rootless internet egress · /dev/kvm (Intel VT-x / AMD-V) for hardware MicroVM execution.",
  "cli.title": "CLI Cookbook & Quick Examples",
  "cli.lead":
    "Docker-compatible commands (no --image or -- boilerplate required) paired with Next-Gen WebAssembly, KVM MicroVM, and AI Sandbox tools.",
  "cli.guideCaption": "Visual quick guide covering installation, Docker-style CLI commands, Coral Desktop v2.8.4 theme/run workflow, and Wasm/KVM.",
  "cli.tabContainers": "Containers & Exec",
  "cli.tabBuild": "Build, Compose & Push",
  "cli.tabWasm": "WebAssembly (Wasm)",
  "cli.tabVmm": "KVM MicroVM & Sandbox",
  "nextgen.title": "Inside v2.8.4 Next-Gen Engines",
  "nextgen.lead":
    "Two purpose-built engines that set Coral apart — engineered for untrusted AI code execution and microsecond serverless compute.",
  "nextgen.wasmCardTitle": "coral-wasm — Microsecond WASI Preview 1 Engine",
  "nextgen.wasmBullet1": "Runs .wasm and .wat directly on wasmi 2.0 without carrying a Linux rootfs.",
  "nextgen.wasmBullet2": "Deterministic CPU metering with --fuel stops infinite loops instantly.",
  "nextgen.wasmBullet3": "Enforces linear memory caps (--memory-mb) via wasmi StoreLimits.",
  "nextgen.wasmBullet4": "Pack modules into OCI images with coral wasm pack and run via --runtime wasm.",
  "nextgen.vmmCardTitle": "coral-vmm — Native /dev/kvm Hypervisor & AI Sandbox",
  "nextgen.vmmBullet1": "Direct Pure-Rust Linux /dev/kvm ioctl engine (CPUID, 4-level paging, COM1 16550 UART).",
  "nextgen.vmmBullet2": "Enforces wall-clock deadlines and max VM-exit budgets against runaway guests.",
  "nextgen.vmmBullet3": "Includes SVR4 newc initramfs builder and Linux bzImage (HdrS) validator for full kernel boot.",
  "nextgen.vmmBullet4": "AI Agent Sandbox: --network none, read-only rootfs, Seccomp-BPF, and full process-group SIGKILL on timeout.",
  "desktop.title": "Coral Desktop (Tauri v2)",
  "desktop.lead":
    "A crisp, lightweight cross-platform GUI speaking native CRI v1 gRPC to coral-daemon locally, over WSL2, or on a remote Linux host.",
  "desktop.f1Title": "Full-Duplex Streaming PTY Terminal",
  "desktop.f1Desc": "True interactive Linux PTY sessions over CRI gRPC ExecStream with dynamic terminal resizing.",
  "desktop.f2Title": "Live Metrics & CoW Inspector",
  "desktop.f2Desc": "Real-time CPU/RAM SVG sparklines, OverlayFS filesystem diffs, and one-click image commits.",
  "desktop.f3Title": "Quick Fill + Inline Quick Pull",
  "desktop.f3Desc": "1-click templates (Nginx, Redis, Postgres, Alpine, Minecraft PaperMC), visible Volumes/Env inputs, and direct image pull from Discover.",
  "desktop.f4Title": "Topbar Theme Popover (5 Palettes & EN/TH)",
  "desktop.f4Desc": "Instant Light/Dark/System, Coral/Ocean/Emerald/Indigo/Amber accent switcher, density toggle, and bilingual EN/TH from any screen.",
  "bench.title": "Verified Benchmarks & Docker Comparison",
  "bench.lead":
    "Measured on the same Linux x86_64 host (Intel Core i5-13400F, 16 CPUs) comparing Coral v2.3.0 against Docker 29.10.0 / Server 29.9.3.",
  "bench.chartCaption": "Controlled-environment startup latency and resource footprint comparison (Coral v2.3.0 vs Docker Engine 29.10.0 rootless)",
  "bench.reportBtn": "Read Full Engineering Report (REPORT_v2.3.md)",
  "bench.rawBtn": "View Raw Benchmark Data (BENCHMARK.md)",
  "bench.caption": "Controlled-environment startup benchmark and capability comparison (5 rounds × 30 runs · median-of-medians · alpine / busybox / debian)",
  "bench.colFeature": "Capability / Measured Metric",
  "bench.colCoral": "Coral CLI v2.3.0",
  "bench.colRunc": "Coral Daemon (CRI v1)",
  "bench.colDocker": "Docker 29.10 / 29.9.3",
  "bench.r1": "Startup P50 Latency (alpine:3.20)",
  "bench.r1c": "8.88 ms (Min 7.14 ms · 96.7 ctr/s)",
  "bench.r1r": "28.57 ms (Mean 35.61 ms · 28.1 ctr/s)",
  "bench.r1d": "159.32 ms (no-net) · 237.56 ms (bridge)",
  "bench.r2": "Binary Footprint & Daemon RAM",
  "bench.r2c": "18.98 MB CLI (Rust Zero-GC)",
  "bench.r2r": "8.4 MB idle / 15.0 MB after 5 rounds RSS",
  "bench.r2d": "36.16 MB CLI + dockerd/containerd VM",
  "bench.r3": "WASI WebAssembly Cold Start",
  "bench.r3c": "329 µs internal (5.72 ms CLI P50)",
  "bench.r3r": "Supported via coral run --runtime wasm",
  "bench.r3d": "External Wasm shim required",
  "bench.r4": "Native /dev/kvm 64-bit MicroVM",
  "bench.r4c": "963 µs self-test (Zero QEMU)",
  "bench.r4r": "Built-in VirtIO-Blk & VirtIO-Net",
  "bench.r4d": "Requires external Kata/VM",
  "bench.r5": "AI Code Sandbox (Air-Gapped + RO)",
  "bench.r5c": "20 ms internal (23.28 ms CLI)",
  "bench.r5r": "Process-group SIGKILL timeout",
  "bench.r5d": "Custom wrapper needed",
  "bench.r6": "Desktop GUI & Theme System",
  "bench.r6c": "Tauri 7.88 MB · 5 Palettes · EN/TH",
  "bench.r6r": "Local / WSL2 / Remote CRI",
  "bench.r6d": "Docker Desktop (Electron)",
  "arch.title": "10-Crate Rust Workspace",
  "arch.lead":
    "Clean separation of concerns across 10 crates — every target verified with strict cargo clippy -- -D warnings (0 warnings).",
  "arch.caption": "Overview of the 10 crates in the coral_repo workspace",
  "arch.colCrate": "Crate",
  "arch.colPath": "Path",
  "arch.colRole": "Responsibility",
  "support.title": "Support & Donate",
  "support.lead":
    "Coral is 100% open-source under Apache-2.0. If Coral helps your workflow or research, you can fuel ongoing development and CI hardware through any channel below.",
  "support.templateBadge": "Ready to Configure",
  "support.activeBadge": "Active Channel",
  "support.ppTitle": "PromptPay QR (Thailand)",
  "support.ppDesc":
    "Instant zero-fee transfer from any Thai banking app via PromptPay QR code.",
  "support.ppBtn": "Show PromptPay QR",
  "support.ghTitle": "GitHub Sponsors",
  "support.ghDesc":
    "Monthly or one-time sponsorship directly on GitHub with a public Sponsor badge on your profile.",
  "support.ghBtn": "Sponsor on GitHub",
  "support.bmcTitle": "Buy Me a Coffee / Ko-fi",
  "support.bmcDesc":
    "Support with a quick one-time coffee using card, Apple Pay, or Google Pay — no account needed.",
  "support.bmcBtn": "Buy a Coffee",
  "support.configHint":
    "For maintainers: edit docs/donate-config.js (and optionally drop your QR at docs/assets/promptpay-qr.png) to activate your live sponsorship links in seconds.",
  "dialog.ppTitle": "Support via PromptPay QR",
  "dialog.ppPlaceholderTitle": "PromptPay QR image not uploaded yet",
  "dialog.ppPlaceholderDesc":
    "Place your QR image at docs/assets/promptpay-qr.png and set promptpay.qrImage & configured: true in docs/donate-config.js.",
  "dialog.ppAccount": "Account Name:",
  "dialog.ppId": "PromptPay ID:",
  "dialog.ppHint": "Press Esc or click outside this dialog to close.",
  "dialog.close": "Close",
  "copy.btn": "Copy",
  "copy.done": "Copied!",
  "footer.copy": "Coral Container Project v2.8.4 · Open-Source under Apache License 2.0",
  "footer.reportLink": "Full Report (REPORT_v2.3.md)",
  "footer.docsLink": "Full Manual (MANUAL.md)",
  "footer.roadmapLink": "Roadmap",
  "footer.issuesLink": "Issues & Feature Requests",
  "report.title": "Latest Measurement Report & Improvement Backlog",
  "report.lead":
    "The latest measurement round, taken from the coral-runtime 2.3.0 .deb package installed system-wide on a real host, compared against rootless Docker Engine 29.10.0 — plus what still needs to be improved.",
  "report.envTitle": "Measurement environment",
  "report.envDesc":
    "Ubuntu 26.10 · kernel 7.3.0-11-generic · 16 CPU (pinned to 8-15) · Coral 2.3.0 installed system-wide from the .deb package (/usr/bin/coral, /usr/bin/coral-daemon) · Docker Engine 29.10.0 rootless (rootlesskit + slirp4netns, overlayfs storage driver)",
  "report.tableCaption":
    "Controlled-environment round: 5 rounds × 30 runs as median-of-medians, warm cache mode · alpine / busybox / debian",
  "report.colWorkload": "Mode / Workload",
  "report.colRuns": "Runs",
  "report.colMin": "Min",
  "report.colP50": "P50",
  "report.colMean": "Mean",
  "report.colThr": "Throughput",
  "report.colVs": "vs Docker bridge",
  "report.findTitle": "Key findings from this round",
  "report.find1Title": "Speed does not depend on the distro",
  "report.find1Desc":
    "alpine (8.88 ms), busybox (8.38 ms) and debian:bookworm-slim (8.38 ms) land within a few milliseconds of each other, so the engine overhead comes from creating the container rather than from the size of the rootfs.",
  "report.find2Title": "The CRI daemon still beats Docker, but not raw runc",
  "report.find2Desc":
    "With the page cache and CPU load under control, coral-daemon completes in 28.57 ms against runc at 24.20 ms — the gRPC layer adds roughly 4 ms. An earlier single-round measurement had shown the CRI daemon 2.23× faster than runc; that was measurement noise, not a property of Coral. The defensible claim is that the CRI daemon still beats Docker by 8.3×.",
  "report.find3Title": "Docker's stack cost sits in bridge networking",
  "report.find3Desc":
    "docker run with the default bridge takes 237.56 ms against 24.20 ms for runc directly — roughly +213 ms (+881%) for docker CLI → dockerd → containerd → shim → runc plus network namespace setup.",
  "report.find4Title": "Scope of the comparison",
  "report.find4Desc":
    "This round measures rootless Docker on the same host as Coral, so only the overhead ratio is comparable; the numbers are not cross-host comparable with the earlier Docker Desktop table.",
  "report.backlogTitle": "Improvement backlog",
  "report.backlogLead":
    "Collected from the measurement results and from installing the package on this machine, ordered by priority.",
  "report.backlogCaption":
    "Improvement backlog after the controlled-environment measurement round (November 2026)",
  "report.blColNo": "#",
  "report.blColArea": "Area",
  "report.blColFinding": "Finding / evidence",
  "report.blColAction": "Action",
  "report.blColStatus": "Status",
  "report.bl1Area": "Benchmark methodology",
  "report.bl1Finding":
    "CRI daemon P50 differs by 2.3× between two rounds on the same host (21.49 → 9.49 ms) because page cache and machine load are not controlled yet.",
  "report.bl1Action":
    "Run several rounds, report median-of-medians, separate cold/warm cache runs, and pin the CPU so every round is comparable.",
  "report.bl2Area": "Website numbers lag the latest results",
  "report.bl2Finding":
    "The site's benchmark table still shows only the first round (Docker Desktop on another machine); the rootless / system-wide round was missing.",
  "report.bl2Action":
    "Add the system-wide table and label the host and mode of every round (done in this round).",
  "report.bl3Area": "Packaged systemd unit omits the state dir",
  "report.bl3Finding":
    "The packaged unit carries no --state-dir/--images-dir/--default-rootfs, so the daemon falls back to XDG_RUNTIME_DIR (a tmpfs), cannot see existing state, and needs a hand-written drop-in — fixed by pointing the unit at the persistent %h/.local/state/coral.",
  "report.bl3Action":
    "Ship sensible defaults, or add a coral-daemon --init command that writes a unit with the machine's state paths.",
  "report.bl4Area": "Coral Desktop AppImage fails to bundle",
  "report.bl4Finding":
    "the linuxdeploy gtk plugin failed twice: dpkg-architecture was missing (dpkg-dev), and Ubuntu 26.10's /usr/bin/cp is uutils coreutils, whose cp -r --parents mishandles absolute source paths",
  "report.bl4Action":
    "install dpkg-dev and put GNU cp ahead on PATH for the bundle step (done in this round)",
  "report.bl5Area": "No post-install verification",
  "report.bl5Finding":
    "After dpkg -i nothing verifies that coral / coral-daemon actually work and that the service came up.",
  "report.bl5Action":
    "Add a postinst smoke check: coral --version, coral daemon-version and systemctl --user is-active coral-daemon.",
  "report.bl6Area": "No CI that measures latency automatically",
  "report.bl6Finding":
    "The current workflows cover Desktop and Pages only; no job runs the benchmark continuously.",
  "report.bl6Action":
    "Add a nightly job that runs scripts/benchmark.sh and uploads the results as an artifact to catch startup-latency regressions.",
  "report.bl7Area": "The suite still covers only alpine:3.20",
  "report.bl7Finding":
    "The system-wide round measured alpine:3.20 only, while the first round also covered debian:bookworm-slim and busybox:musl.",
  "report.bl7Action":
    "Extend benchmark.sh with a distro matrix and run the controlled-environment round with PULL_EXTRA=1, so alpine, busybox and debian are all covered at once (done in this round).",
  "report.bl8Area": "ARM64 never ran the E2E suite",
  "report.bl8Finding":
    "CI built ARM64 artifacts but never ran the test suite, and coral-vmm did not compile on ARM at all because its imports and cfg gates disagreed.",
  "report.bl8Action":
    "Add ARM64 to the verify matrix and fix every break until build, clippy and the unit tests pass on aarch64 (done in this round).",
  "report.bl9Area": "CRI daemon intermittently loses state.json",
  "report.bl9Finding":
    "The second measurement round saw 2 failures in ~300 CRI daemon runs with `rename state.json: No such file or directory`, triggered while the daemon handles several containers at once.",
  "report.bl9Action":
    "Use a unique temporary file name per write, fsync before the rename, and treat a concurrently removed container as a non-error — with six regression tests (done in this round).",
  "report.bl10Area": "coral load of a .tar.zst archive intermittently fails",
  "report.bl10Finding":
    "CI caught it in oci_features_e2e::image_tag_save_zstd_and_load_roundtrip_cli with `zstd decompression failed:` (empty). The tar reader stops at the end-of-archive marker without draining the pipe, so the pipe is closed while zstd is still writing and the decompressor dies with SIGPIPE.",
  "report.bl10Action":
    "Drain the decompressor's stdout before waiting, and reap the compressor on the save side when the write fails — with a regression test that reproduces the race deterministically (done in this round).",
  "report.stOpen": "Open",
  "report.stDone": "Fixed in this round",
  "report.reportBtn": "Read Full Engineering Report (REPORT_v2.3.md)",
  "report.rawBtn": "View Raw Benchmark Data (BENCHMARK.md)",
  "announce.pill": "NEW",
  "announce.text":
    "Coral v2.8.4 — Built-in CVE Scanner (OSV.dev), Rootless L3 Virtual Switch, Multi-Arch Buildx & 17.9× Faster Startup",
  "announce.cta": "Install now →",
  "topbar.menu": "Menu",
  "topbar.download": "Download",
  "top.features": "Features",
  "top.cli": "CLI",
  "top.nextgen": "Wasm & KVM",
  "top.desktop": "Desktop",
  "top.benchmarks": "Benchmarks",
  "top.arch": "Architecture",
  "pillars.title": "Build · Share · Run · Verify — One Rust Engine",
  "pillars.lead":
    "The familiar Docker developer workflow you already love, engineered from scratch in Rust with microsecond Wasm and native KVM MicroVMs built in.",
  "pillar.buildTitle": "Build",
  "pillar.buildDesc":
    "Modern BuildKit Dockerfiles with .dockerignore, parallel multi-stage DAG, cache/secret/bind/ssh mounts, and multi-arch OCI Image Index publishing.",
  "pillar.shareTitle": "Share",
  "pillar.shareDesc":
    "4-stream parallel pulls with double SHA-256 verification, zero-copy hardlink shared layers, multi-lowerdir OverlayFS, and instant registry push.",
  "pillar.runTitle": "Run",
  "pillar.runDesc":
    "8.88 ms rootless OCI startup, 10.89.x.Y virtual switch with round-robin DNS & NAT, streaming docker.sock, NVIDIA CDI, WASI Wasm, and /dev/kvm MicroVMs.",
  "pillar.verifyTitle": "Verify",
  "pillar.verifyDesc":
    "Built-in coral scan CVE auditing (SPDX SBOM + OSV.dev + --fail-on CI gates), Ed25519 image signing, and live coral top / wait / stats --all observability.",
  "footer.tagline":
    "Memory-safe Rust OCI container runtime, streaming docker.sock Engine API, WASI WebAssembly, /dev/kvm MicroVMs, and Coral Desktop.",
  "footer.colProducts": "Products",
  "footer.colDocs": "Documentation",
  "footer.colCommunity": "Community",
  "cmd.trigger": "Search",
  "eco.label": "Compatible with",
  "bench.replay": "▶ Replay Race",
  "arch.pickLabel": "Select CPU Architecture",
  "arch.x86Sub": "Intel / AMD 64-bit · Linux PC, Server & WSL2",
  "arch.armSub": "Apple Silicon (Lima), Raspberry Pi 4/5 & AWS Graviton",
};

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
  "hero.badge": "Release v2.3.0 · Rust Edition 2024 · x86_64 & ARM64 · Apache-2.0",
  "hero.title": "Next-Gen Container Runtime in Rust",
  "hero.subtitle":
    "Docker-compatible CLI ergonomics (coral run -d -m 4G ...), rootless OCI containers with OverlayFS CoW, Kubernetes CRI v1 + CNI plugin chains, Serverless WASI HTTP (coral-wasm), Pure-Rust /dev/kvm MicroVMs, and the redesigned 5-palette Coral Desktop v2.3.0.",
  "hero.ctaInstall": "Download & Install v2.3.0",
  "hero.ctaBench": "Docker Comparison & Report",
  "hero.ctaGithub": "View on GitHub",
  "hero.ctaSupport": "Support the Project",
  "metric.startLabel": "OCI Container P50 Startup",
  "metric.startSub": "7.3×–11.2× faster than Docker on same host",
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
    "Includes coral build (content-addressable step cache), coral compose, coral login/push/pull (OCI Distribution v2), custom networks (pasta/slirp4netns egress), Named Volumes, Healthchecks, and Boot Auto-Resume.",
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
  "install.downloadDeb": "Download coral-runtime_2.3.0_amd64.deb",
  "install.downloadTar": "Download coral-v2.3.0-linux-x86_64.tar.gz",
  "install.allReleases": "Browse all assets on GitHub Releases",
  "install.desktopDesc":
    "Manage containers, pods, images, volumes, networks, live streaming PTY terminals, and Next-Gen (Wasm & KVM) playgrounds in Coral Desktop v2.3.0:",
  "install.desktopLinuxTitle": "Linux (Ubuntu / Fedora / AppImage)",
  "install.desktopLinuxStep":
    "Download Coral Desktop_2.3.0_amd64.deb or .rpm from GitHub Releases or build from source into ~/.local/bin with automatic daemon discovery.",
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
  "cli.guideCaption": "Visual quick guide covering installation, Docker-style CLI commands, Coral Desktop v2.3.0 theme/run workflow, and Wasm/KVM.",
  "cli.tabContainers": "Containers & Exec",
  "cli.tabBuild": "Build, Compose & Push",
  "cli.tabWasm": "WebAssembly (Wasm)",
  "cli.tabVmm": "KVM MicroVM & Sandbox",
  "nextgen.title": "Inside v2.3.0 Next-Gen Engines",
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
  "bench.chartCaption": "Same-machine startup latency and resource footprint comparison (Coral v2.3.0 vs Docker Engine 29.9.3)",
  "bench.reportBtn": "Read Full Engineering Report (REPORT_v2.3.md)",
  "bench.rawBtn": "View Raw Benchmark Data (BENCHMARK.md)",
  "bench.caption": "Same-machine startup benchmark and capability comparison (Alpine 3.20 · 20–30 runs)",
  "bench.colFeature": "Capability / Measured Metric",
  "bench.colCoral": "Coral CLI v2.3.0",
  "bench.colRunc": "Coral Daemon (CRI v1)",
  "bench.colDocker": "Docker 29.10 / 29.9.3",
  "bench.r1": "Startup P50 Latency (alpine:3.20)",
  "bench.r1c": "23.22 ms (Min 3.52 ms · 50.8 ctr/s)",
  "bench.r1r": "53.07 ms (Mean 49.12 ms · 20.4 ctr/s)",
  "bench.r1d": "168.95 ms (no-net) · 260.58 ms (bridge)",
  "bench.r2": "Binary Footprint & Daemon RAM",
  "bench.r2c": "18.37 MB CLI (Rust Zero-GC)",
  "bench.r2r": "9.1 MB idle / 12.0 MB active RSS",
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
  "footer.copy": "Coral Container Project v2.3.0 · Open-Source under Apache License 2.0",
  "footer.reportLink": "Full Report (REPORT_v2.3.md)",
  "footer.docsLink": "Full Manual (MANUAL.md)",
  "footer.roadmapLink": "Roadmap",
  "footer.issuesLink": "Issues & Feature Requests",
  "report.title": "Latest Measurement Report & Improvement Backlog",
  "report.lead":
    "The latest measurement round, taken from the coral-runtime 2.3.0 .deb package installed system-wide on a real host, compared against rootless Docker Engine 29.10.0 — plus what still needs to be improved.",
  "report.envTitle": "Measurement environment",
  "report.envDesc":
    "Ubuntu 26.10 · kernel 7.3.0-11-generic · 16 CPU · Coral 2.3.0 installed system-wide from the .deb package (/usr/bin/coral, /usr/bin/coral-daemon) · Docker Engine 29.10.0 rootless (rootlesskit + slirp4netns, overlayfs storage driver)",
  "report.tableCaption":
    "System-wide round: Coral v2.3.0 (.deb package) vs rootless Docker Engine 29.10.0 · alpine:3.20 · 30/20 runs",
  "report.colWorkload": "Mode / Workload",
  "report.colRuns": "Runs",
  "report.colMin": "Min",
  "report.colP50": "P50",
  "report.colMean": "Mean",
  "report.colThr": "Throughput",
  "report.colVs": "vs Docker bridge",
  "report.findTitle": "Key findings from this round",
  "report.find1Title": "Packaging adds no overhead",
  "report.find1Desc":
    "P50 for OCI direct (5.53 ms) and CLI (6.18 ms) matches the in-repo build round (5.50 / 6.33 ms), because the package only copies the same binaries — no wrapper or extra process layer.",
  "report.find2Title": "CRI Daemon is 2.23× faster than raw runc",
  "report.find2Desc":
    "coral-daemon completes in 9.49 ms against runc at 21.19 ms — the overhead stacked on Coral's CRI daemon is effectively negative next to the very OCI runtime Docker ships.",
  "report.find3Title": "Docker's stack cost sits in bridge networking",
  "report.find3Desc":
    "docker run with the default bridge takes 211.96 ms against 21.19 ms for runc directly — roughly +191 ms (+900%) for docker CLI → dockerd → containerd → shim → runc plus network namespace setup.",
  "report.find4Title": "Scope of the comparison",
  "report.find4Desc":
    "This round measures rootless Docker on the same host as Coral, so only the overhead ratio is comparable; the numbers are not cross-host comparable with the earlier Docker Desktop table.",
  "report.backlogTitle": "Improvement backlog",
  "report.backlogLead":
    "Collected from the measurement results and from installing the package on this machine, ordered by priority.",
  "report.backlogCaption":
    "Improvement backlog after the system-wide measurement round (November 2026)",
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
    "The packaged unit carries no --state-dir/--images-dir/--default-rootfs, so the daemon falls back to XDG_RUNTIME_DIR, cannot see existing state, and needs a hand-written drop-in.",
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
    "Extend benchmark.sh with a distro matrix and run the system-wide round with PULL_EXTRA=1.",
  "report.bl8Area": "ARM64 never ran the E2E suite",
  "report.bl8Finding":
    "CI built ARM64 artifacts but never ran the test suite, and coral-vmm did not compile on ARM at all because its imports and cfg gates disagreed.",
  "report.bl8Action":
    "Add ARM64 to the verify matrix and fix every break until build, clippy and the unit tests pass on aarch64 (done in this round).",
  "report.stOpen": "Open",
  "report.stDone": "Fixed in this round",
  "report.reportBtn": "Read Full Engineering Report (REPORT_v2.3.md)",
  "report.rawBtn": "View Raw Benchmark Data (BENCHMARK.md)",
};

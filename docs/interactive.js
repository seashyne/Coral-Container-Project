(() => {
  "use strict";

  /* 1. Scroll Progress Bar fallback (scroll-progress-indicator guidance) */
  const progressBar = document.getElementById("scroll-progress");
  if (
    progressBar &&
    !CSS.supports("animation-timeline", "scroll()") &&
    window.matchMedia("(prefers-reduced-motion: no-preference)").matches
  ) {
    const updateScroll = () => {
      const scrollable = document.documentElement.scrollHeight - window.innerHeight;
      const ratio = scrollable > 0 ? Math.min(1, Math.max(0, window.scrollY / scrollable)) : 0;
      progressBar.style.transform = `scaleX(${ratio})`;
    };
    window.addEventListener("scroll", updateScroll, { passive: true });
    updateScroll();
  }

  /* 2. Hero OS Quick-Install Command Switcher */
  const OS_COMMANDS = {
    linux: "curl -fsSL https://seashyne.github.io/Coral-Container-Project/install.sh | bash",
    deb: "curl -fsSLO https://github.com/seashyne/Coral-Container-Project/releases/download/v2.8.3/coral-runtime_2.8.3_amd64.deb && sudo dpkg -i coral-runtime_2.8.3_amd64.deb",
    wsl: "wsl --install -d Ubuntu && wsl bash -c 'curl -fsSL https://seashyne.github.io/Coral-Container-Project/install.sh | bash'",
    mac: "coral machine init --vm lima --cpus 4 --memory 4096 && coral machine start",
  };
  const heroCmdEl = document.getElementById("hero-install-cmd");
  document.querySelectorAll("[data-os-cmd]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const key = btn.dataset.osCmd;
      if (heroCmdEl && OS_COMMANDS[key]) heroCmdEl.textContent = OS_COMMANDS[key];
      document.querySelectorAll("[data-os-cmd]").forEach((b) => {
        b.setAttribute("aria-pressed", b === btn ? "true" : "false");
      });
    });
  });

  /* 3. Hero Terminal Copy Active Tab Command */
  const heroTermCopy = document.getElementById("hero-term-copy");
  if (heroTermCopy) {
    heroTermCopy.addEventListener("click", async () => {
      const activePanel = document.querySelector(".hero-terminal [role='tabpanel']:not([hidden]) pre");
      if (!activePanel) return;
      const cmds = Array.from(activePanel.querySelectorAll(".t-cmd"))
        .map((el) => el.textContent.replace(/^\$\s*/, "").trim())
        .join("\n");
      if (!cmds) return;
      try {
        await navigator.clipboard.writeText(cmds);
      } catch {}
      const prev = heroTermCopy.textContent;
      heroTermCopy.textContent = "✓";
      setTimeout(() => {
        heroTermCopy.textContent = prev;
      }, 1400);
    });
  }

  /* 4. Interactive Coral Desktop Mockup Accent Switcher */
  const mockWin = document.getElementById("desktop-mock");
  document.querySelectorAll("[data-mock-accent]").forEach((btn) => {
    btn.addEventListener("click", () => {
      if (mockWin) mockWin.dataset.accent = btn.dataset.mockAccent;
      document.querySelectorAll("[data-mock-accent]").forEach((b) => {
        b.setAttribute("aria-pressed", b === btn ? "true" : "false");
      });
    });
  });

  /* 5. Interactive Live Benchmark Race Visualizer */
  const RACE_DATA = {
    alpine: {
      badge: "17.9× faster than Docker",
      note: "Alpine 3.20 cold start P50 · Intel Core i5-13400F Linux (5×30 median-of-medians).",
      rows: [
        { label: "Coral CLI (Rootless OCI)", value: "8.88 ms", pct: 6, cls: "" },
        { label: "Coral Daemon (CRI v1 gRPC)", value: "23.2 ms", pct: 15, cls: "daemon" },
        { label: "Docker Engine 29.10 (Rootless)", value: "159.3 ms", pct: 100, cls: "docker" },
      ],
    },
    busybox: {
      badge: "18.8× faster than Docker",
      note: "BusyBox musl container startup P50 on the same host without root.",
      rows: [
        { label: "Coral CLI (Rootless OCI)", value: "8.45 ms", pct: 5.5, cls: "" },
        { label: "Coral Daemon (CRI v1 gRPC)", value: "21.8 ms", pct: 14, cls: "daemon" },
        { label: "Docker Engine 29.10 (Rootless)", value: "158.9 ms", pct: 100, cls: "docker" },
      ],
    },
    debian: {
      badge: "17.4× faster than Docker",
      note: "Debian Bookworm Slim container startup P50 with shared zero-copy layers.",
      rows: [
        { label: "Coral CLI (Rootless OCI)", value: "9.34 ms", pct: 6, cls: "" },
        { label: "Coral Daemon (CRI v1 gRPC)", value: "24.1 ms", pct: 15, cls: "daemon" },
        { label: "Docker Engine 29.10 (Rootless)", value: "162.7 ms", pct: 100, cls: "docker" },
      ],
    },
    ram: {
      badge: "14× lower idle RAM",
      note: "Idle background daemon RSS footprint (coral-daemon vs dockerd + containerd).",
      rows: [
        { label: "coral-daemon (Rust single binary)", value: "8.4 MB", pct: 7, cls: "" },
        { label: "dockerd + containerd idle", value: "118 MB", pct: 100, cls: "docker" },
      ],
    },
    nextgen: {
      badge: "Sub-millisecond Next-Gen",
      note: "Pure-Rust WASI Preview 1 (wasmi 2.0) and native /dev/kvm 64-bit Long Mode boot.",
      rows: [
        { label: "coral-wasm (WASI Preview 1)", value: "329 µs (0.33 ms)", pct: 4, cls: "" },
        { label: "coral-vmm (/dev/kvm 64-bit boot)", value: "963 µs (0.96 ms)", pct: 11, cls: "daemon" },
        { label: "Coral OCI Container P50", value: "8.88 ms", pct: 100, cls: "docker" },
      ],
    },
  };

  let activeRace = "alpine";
  const raceRows = document.getElementById("race-rows");
  const raceNote = document.getElementById("race-note");
  const raceBadge = document.getElementById("race-badge");

  function renderRace(key, animate = true) {
    activeRace = RACE_DATA[key] ? key : "alpine";
    const data = RACE_DATA[activeRace];
    if (!raceRows) return;
    raceRows.innerHTML = data.rows
      .map(
        (r) => `
        <div class="race-row">
          <div class="race-row-head">
            <span>${r.label}</span>
            <span class="race-val">${r.value}</span>
          </div>
          <div class="race-track">
            <div class="race-fill ${r.cls}" style="--pct:${animate ? 0 : r.pct}%"></div>
          </div>
        </div>`
      )
      .join("");
    if (raceNote) raceNote.textContent = data.note;
    if (raceBadge) raceBadge.textContent = data.badge;
    if (animate) {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          raceRows.querySelectorAll(".race-fill").forEach((el, idx) => {
            el.style.setProperty("--pct", `${data.rows[idx].pct}%`);
          });
        });
      });
    }
  }

  document.querySelectorAll("[data-race]").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll("[data-race]").forEach((b) => {
        b.setAttribute("aria-pressed", b === btn ? "true" : "false");
      });
      renderRace(btn.dataset.race, true);
    });
  });

  document.getElementById("race-replay")?.addEventListener("click", () => renderRace(activeRace, true));
  renderRace("alpine", false);

  /* 6. Ctrl+K Quick Command Palette (<dialog closedby="any"> + Safari fallback) */
  const cmdDialog = document.getElementById("cmd-dialog");
  const cmdInput = document.getElementById("cmd-input");
  const cmdList = document.getElementById("cmd-list");
  const CMD_ITEMS = [
    { label: "Download & Install Coral v2.8.3", hint: "#install", href: "#install" },
    { label: "CLI Cookbook & Examples", hint: "#quickstart", href: "#quickstart" },
    { label: "Platform Features (vSwitch, Buildx, docker.sock)", hint: "#features", href: "#features" },
    { label: "Coral Desktop (Tauri v2 · 8 Languages)", hint: "#desktop", href: "#desktop" },
    { label: "Interactive Benchmarks vs Docker", hint: "#benchmarks", href: "#benchmarks" },
    { label: "WASI WebAssembly & /dev/kvm MicroVM", hint: "#nextgen", href: "#nextgen" },
    { label: "10-Crate Rust Architecture", hint: "#architecture", href: "#architecture" },
    { label: "Copy: coral run -d --name web -p 8080:80 nginx:alpine", hint: "CLI", copy: "coral run -d --name web -p 8080:80 nginx:alpine" },
    { label: "Copy: coral scan alpine:3.20 --fail-on high", hint: "CVE Scan", copy: "coral scan alpine:3.20 --fail-on high" },
    { label: "Copy: coral build --platform linux/amd64,linux/arm64 --push .", hint: "Buildx", copy: "coral build --platform linux/amd64,linux/arm64 --push ." },
    { label: "Copy: coral compose up -d --scale web=3", hint: "Compose", copy: "coral compose up -d --scale web=3" },
    { label: "Toggle Light / Dark Theme", hint: "Theme", action: () => document.getElementById("theme-toggle")?.click() },
  ];

  function renderCmdList(query = "") {
    if (!cmdList) return;
    const q = query.trim().toLowerCase();
    const matches = CMD_ITEMS.filter(
      (item) => !q || item.label.toLowerCase().includes(q) || item.hint.toLowerCase().includes(q)
    );
    cmdList.innerHTML = "";
    matches.forEach((item) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "cmd-item";
      btn.innerHTML = `<span>${item.label}</span><small>${item.hint}</small>`;
      btn.addEventListener("click", async () => {
        if (item.href) {
          cmdDialog?.close();
          document.querySelector(item.href)?.scrollIntoView({ behavior: "smooth" });
        } else if (item.copy) {
          try {
            await navigator.clipboard.writeText(item.copy);
          } catch {}
          cmdDialog?.close();
        } else if (item.action) {
          item.action();
          cmdDialog?.close();
        }
      });
      cmdList.appendChild(btn);
    });
  }

  function openCmdPalette() {
    if (!cmdDialog) return;
    if (cmdInput) cmdInput.value = "";
    renderCmdList("");
    cmdDialog.showModal();
    cmdInput?.focus();
  }

  document.getElementById("cmd-open-btn")?.addEventListener("click", openCmdPalette);
  cmdInput?.addEventListener("input", () => renderCmdList(cmdInput.value));

  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      if (cmdDialog?.open) cmdDialog.close();
      else openCmdPalette();
    }
  });

  // Mandatory light-dismiss fallback for browsers without <dialog closedby="any">
  if (cmdDialog && !("closedBy" in HTMLDialogElement.prototype)) {
    cmdDialog.addEventListener("click", (event) => {
      if (event.target !== cmdDialog) return;
      const rect = cmdDialog.getBoundingClientRect();
      const inside =
        rect.top <= event.clientY &&
        event.clientY <= rect.top + rect.height &&
        rect.left <= event.clientX &&
        event.clientX <= rect.left + rect.width;
      if (!inside) cmdDialog.close();
    });
  }
})();

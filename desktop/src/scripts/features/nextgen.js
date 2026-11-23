// Next-Gen Runtimes view: AI Sandbox, KVM MicroVM, WebAssembly, Security & SBOM.

async function probeKvmHardware() {
  if (!state.connected) {
    toast(t("engine.connectPromptFirst"), true);
    return;
  }
  const btn = $("kvm-probe-btn");
  if (btn) btn.disabled = true;
  try {
    await guard(async () => {
      const res = await invoke("vmm_probe");
      const caps = res.capabilities || {};
      const st = res.selftest;
      const pill = $("kvm-status-pill");
      if (pill) {
        pill.textContent = caps.available ? "KVM Active (VT-x/AMD-V)" : "KVM Unavailable";
        pill.className = "health-badge " + (caps.available ? "healthy" : "unhealthy");
      }
      const meta = $("kvm-probe-meta");
      if (meta) {
        meta.textContent = `${caps.arch || "unknown"} • API v${caps.apiVersion || 0} • Max ${caps.maxVcpus || 0} vCPUs • ${caps.detail || ""}`;
      }
      const out = $("kvm-selftest-out");
      if (out) {
        if (st) {
          out.textContent =
            `[64-bit Long Mode KVM Self-Test]\n` +
            `Serial Output: ${st.serialOutput}` +
            `Exit Code:     ${st.exitCode} (${st.exitReason})\n` +
            `VM Exits:      ${st.vmExits}\n` +
            `Boot + Exec:   ${st.durationUsec} µs (${(st.durationUsec / 1000).toFixed(2)} ms)\n` +
            `Guest RAM:     ${st.memoryMb} MB`;
        } else {
          out.textContent = caps.detail || "KVM device not accessible on daemon host.";
        }
      }
    });
  } finally {
    if (btn) btn.disabled = false;
  }
}

function applySandboxPreset(presetKey) {
  const p = SANDBOX_PRESETS[presetKey];
  if (!p) return;
  $("sbx-image").value = p.image;
  $("sbx-cmd").value = p.cmd;
  $("sbx-timeout").value = p.timeout;
  $("sbx-mem").value = p.mem;
  $("sbx-net").checked = p.net;
  toast(t("toast.appliedPreset", { name: presetKey }));
}

async function submitAiSandbox(e) {
  e.preventDefault();
  const image = $("sbx-image").value.trim() || "alpine:3.20";
  const cmd = $("sbx-cmd").value.trim();
  const timeoutSecs = Number($("sbx-timeout").value) || 5;
  const memoryMb = Number($("sbx-mem").value) || 64;
  const allowNetwork = $("sbx-net").checked;
  if (!cmd) return;
  $("sbx-run-btn").disabled = true;
  try {
    await guard(async () => {
      const r = await invoke("sandbox_run", {
        image,
        cmd,
        timeoutSecs,
        memoryMb,
        allowNetwork,
      });
      $("sbx-stats").textContent =
        `mode=${r.isolationMode} • exit=${r.exitCode} • ${r.durationMs} ms` +
        (r.timedOut ? " • TIMED OUT" : "");
      let text = r.stdout || "";
      if (r.stderr) text += `\n[stderr]\n${r.stderr}`;
      $("sbx-output").textContent = text || "(completed with no output)";
    });
  } finally {
    $("sbx-run-btn").disabled = false;
  }
}

async function setupMicroVmKernel() {
  if (!state.connected) {
    toast(t("engine.connectPromptFirst"), true);
    return;
  }
  const btn = $("vm-setup-kernel-btn");
  if (btn) btn.disabled = true;
  $("vm-boot-stats").textContent = "Downloading Alpine 6.6 kernel + VirtIO modules…";
  try {
    await guard(async () => {
      const rep = await invoke("vm_setup_kernel");
      $("vm-boot-stats").textContent = `Kernel v${rep.protocolVersion || "2.15"} (${rep.sizeBytes || 0} B)`;
      $("vm-boot-output").textContent =
        `Installed: ${rep.kernelPath}\nSource:    ${rep.source}\nVirtIO modules extracted ready for MicroVM boot.`;
      toast(t("toast.kernelReady"));
    });
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function submitVmBoot(e) {
  e.preventDefault();
  if (!state.connected) {
    toast(t("engine.connectPromptFirst"), true);
    return;
  }
  const image = $("vm-boot-image").value.trim() || "alpine:3.20";
  const cmd = $("vm-boot-cmd").value.trim() || "uname -a";
  const memoryMb = Number($("vm-boot-mem").value) || 256;
  const timeoutSecs = Number($("vm-boot-timeout").value) || 15;
  const disk = $("vm-boot-disk").value.trim() || null;
  const allowNet = $("vm-boot-net").checked;
  $("vm-boot-btn").disabled = true;
  $("vm-boot-stats").textContent = "Booting KVM MicroVM…";
  try {
    await guard(async () => {
      const r = await invoke("vm_boot", {
        image,
        cmd,
        memoryMb,
        timeoutSecs,
        disk,
        allowNet,
      });
      $("vm-boot-stats").textContent =
        `${r.isolationMode || "kvm"} • exit=${r.exitCode} • ${r.durationMs} ms` +
        (r.timedOut ? " • TIMED OUT" : "");
      let text = r.stdout || "";
      if (r.stderr) text += `\n[stderr]\n${r.stderr}`;
      $("vm-boot-output").textContent = text || "(VM booted cleanly with empty output)";
    });
  } finally {
    $("vm-boot-btn").disabled = false;
  }
}

function applyWasmTemplate(key) {
  const src = WASM_TEMPLATES[key];
  if (!src) return;
  $("wasm-source").value = src;
  toast(t("toast.loadedWasmTpl", { name: key }));
}

async function submitWasmPlayground(e) {
  e.preventDefault();
  const source = $("wasm-source").value;
  const fuelLimit = Number($("wasm-fuel").value) || 5000000;
  const memoryLimitMb = Number($("wasm-mem").value) || 16;
  if (!source.trim()) return;
  $("wasm-run-btn").disabled = true;
  try {
    await guard(async () => {
      const out = await invoke("wasm_run", { source, fuelLimit, memoryLimitMb });
      $("wasm-stats").textContent =
        `exit=${out.exitCode} • ${out.durationUsec} µs • fuel=${out.fuelConsumed} • entry=${out.entrypoint}` +
        (out.trapped ? ` • TRAP: ${out.trapped}` : "");
      let text = out.stdout || "";
      if (out.stderr) text += `\n[stderr]\n${out.stderr}`;
      if (out.trapped) text += `\n[wasm trap] ${out.trapped}`;
      $("wasm-output").textContent = text || "(completed with no output)";
    });
  } finally {
    $("wasm-run-btn").disabled = false;
  }
}

async function runImageSecurityAction(action) {
  if (!state.connected) {
    toast(t("engine.connectPromptFirst"), true);
    return;
  }
  const image = $("sec-image").value.trim();
  if (!image) {
    toast("Enter an image tag first", true);
    return;
  }
  const btns = ["sec-sign-btn", "sec-verify-btn", "sec-sbom-btn", "sec-scan-btn"];
  btns.forEach((id) => {
    if ($(id)) $(id).disabled = true;
  });
  try {
    await guard(async () => {
      const cmdMap = {
        sign: "image_sign",
        verify: "image_verify",
        sbom: "image_sbom",
        scan: "image_scan",
      };
      const res = await invoke(cmdMap[action], { image });
      if (action === "sign") {
        $("sec-stats").textContent = `Signed (${res.algorithm || "Ed25519"})`;
        toast(t("toast.signedImage", { image }));
      } else if (action === "verify") {
        $("sec-stats").textContent = res.verified ? "Signature VERIFIED" : "Signature INVALID";
        toast(t(res.verified ? "toast.sigValid" : "toast.sigInvalid"), !res.verified);
      } else if (action === "scan") {
        const s = res.summary || {};
        $("sec-stats").textContent =
          `CVEs: ${s.total || 0} (C:${s.critical || 0} H:${s.high || 0} M:${s.medium || 0}) • ${res.packagesScanned || 0} pkgs`;
        toast(`Scanned ${res.packagesScanned || 0} packages (${s.total || 0} findings)`, (s.critical || 0) + (s.high || 0) > 0);
      } else {
        const count = Array.isArray(res.packages) ? res.packages.length : 0;
        $("sec-stats").textContent = `SPDX 2.3 • ${count} packages`;
        toast(t("toast.sbomGenerated", { count }));
      }
      $("sec-output").textContent = JSON.stringify(res, null, 2);
    });
  } finally {
    btns.forEach((id) => {
      if ($(id)) $(id).disabled = false;
    });
  }
}

function updateWasmServePreview() {
  const target = $("wasm-serve-target").value.trim() || "hello-wasm:latest";
  const addr = $("wasm-serve-addr").value.trim() || "127.0.0.1:8080";
  $("wasm-serve-preview").textContent = `coral wasm serve ${target} --listen ${addr}`;
}

async function copyWasmServeCmd() {
  updateWasmServePreview();
  const cmd = $("wasm-serve-preview").textContent;
  try {
    await navigator.clipboard.writeText(cmd);
    toast(t("toast.copiedWasmCmd"));
  } catch {
    toast(cmd);
  }
}

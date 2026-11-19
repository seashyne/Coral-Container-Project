// Coral v2.0 Next-Gen View: Native /dev/kvm MicroVM Probe, WebAssembly Playground (`coral-wasm`),
// and AI Agent Untrusted Code Sandbox (`coral-vmm`).

async function probeKvmHardware() {
  if (!state.connected) {
    toast("Connect to coral-daemon first", true);
    return;
  }
  $("kvm-probe-btn").disabled = true;
  try {
    await guard(async () => {
      const res = await invoke("vmm_probe");
      const caps = res.capabilities || {};
      const st = res.selftest;
      const pill = $("kvm-status-pill");
      pill.textContent = caps.available ? "KVM Active (VT-x/AMD-V)" : "KVM Unavailable";
      pill.className = "health-badge " + (caps.available ? "healthy" : "unhealthy");
      $("kvm-probe-meta").textContent =
        `${caps.arch || "unknown"} • API v${caps.apiVersion || 0} • Max ${caps.maxVcpus || 0} vCPUs • ${caps.detail || ""}`;
      if (st) {
        $("kvm-selftest-out").textContent =
          `[64-bit Long Mode KVM Self-Test]\n` +
          `Serial Output: ${st.serialOutput}` +
          `Exit Code:     ${st.exitCode} (${st.exitReason})\n` +
          `VM Exits:      ${st.vmExits}\n` +
          `Boot + Exec:   ${st.durationUsec} µs (${(st.durationUsec / 1000).toFixed(2)} ms)\n` +
          `Guest RAM:     ${st.memoryMb} MB`;
      } else {
        $("kvm-selftest-out").textContent = caps.detail || "KVM device not accessible on daemon host.";
      }
    });
  } finally {
    $("kvm-probe-btn").disabled = false;
  }
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

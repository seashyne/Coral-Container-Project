// Containers view: 1-click starters and the Run dialog submit handler.
async function launchQuickStarter(starterId) {
  const s = QUICK_STARTERS.find((item) => item.id === starterId);
  if (!s) return;
  if (!state.connected) {
    toast(t("engine.connectPromptFirst"), true);
    return;
  }
  await guard(async () => {
    toast(t("toast.preparingImage", { image: s.image }));
    await ensureImageInstalled(s.image);
    const suffix = Math.floor((Date.now() / 1000) % 1000);
    const params = {
      name: `${s.name}-${suffix}`,
      image: s.image,
      command: s.command,
      args: s.args,
      memoryMb: s.memoryMb,
      cpuPercent: 100,
      pidsLimit: 128,
      network: s.network,
      restartPolicy: "no",
      healthCmd: "",
      healthIntervalSecs: 5,
      ports: s.ports,
      volumes: "",
      env: s.env,
    };
    const id = await invoke("run_container", { params });
    const ports = parseMappedPorts(s.ports);
    const portHint = ports[0]?.hostPort ? ` — http://localhost:${ports[0].hostPort}` : "";
    toast(t("toast.startedContainer", { name: params.name }) + portHint);
    await loadContainers(false);
    await loadPods(true);
    updateChrome();
    openDetail(id, s.id === "alpine-shell" ? "term" : "logs");
  });
}

async function submitRun() {
  const keepAlive = Boolean($("run-keep-alive")?.checked);
  let cmd = $("run-command").value.trim();
  let args = $("run-args").value;
  if (keepAlive && !cmd) {
    cmd = "/bin/sh";
    args = '-c "sleep 3600"';
  }
  const image = $("run-image").value.trim() || "alpine:3.20";
  const name = $("run-name").value.trim() || `coral-${state.runSeq++}`;

  const params = {
    name,
    image,
    command: cmd,
    args,
    memoryMb: Number($("run-memory").value) || 0,
    cpuPercent: Number($("run-cpu").value) || 0,
    pidsLimit: Number($("run-pids").value) || 0,
    network: $("run-network").value,
    restartPolicy: $("run-restart").value,
    healthCmd: $("run-health-cmd").value.trim(),
    healthIntervalSecs: 5,
    networkAliases: ($("run-network-aliases")?.value || "").trim(),
    gpus: ($("run-gpus")?.value || "").trim(),
    devices: ($("run-devices")?.value || "").trim(),
    logDriver: $("run-log-driver")?.value || "raw",
    capDrop: $("run-cap-drop-all")?.checked ? "ALL" : "",
    securityOpt: $("run-landlock")?.checked ? "landlock=enforce" : "",
    readOnly: Boolean($("run-read-only")?.checked),
    ports: $("run-ports").value,
    volumes: $("run-volumes").value,
    env: $("run-env").value,
  };
  $("run-submit").disabled = true;
  try {
    await guard(async () => {
      await ensureImageInstalled(image);
      const id = await invoke("run_container", { params });
      closeDialog("run-dialog");
      toast(t("toast.startedContainer", { name: params.name }));
      if (state.view !== "containers") switchView("containers");
      await loadContainers(false);
      await loadPods(true);
      updateChrome();
      openDetail(id, keepAlive ? "term" : "logs");
    });
  } finally {
    $("run-submit").disabled = false;
  }
}


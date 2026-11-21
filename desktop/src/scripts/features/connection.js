// Daemon connection lifecycle, engine chrome updates, polling, port opener, and view routing.

function updateChrome() {
  const on = state.connected;
  const info = state.connInfo;
  const runningCount = state.containers.filter((c) => c.state === "running").length;

  $("n-containers").textContent = String(state.containers.length);
  $("n-images").textContent = String(state.images.length);
  $("n-volumes").textContent = String(state.volumes.length);
  $("n-pods").textContent = String(state.pods.length);

  $("vm-card").classList.toggle("on", on);
  $("vm-state").textContent = t(on ? "engine.running" : "engine.stopped");
  $("vm-sub").textContent =
    on && info
      ? t("engine.runningMeta", {
          version: info.runtimeVersion ? `v${info.runtimeVersion} · ` : "",
          count: runningCount,
        })
      : t("engine.clickConnect");

  const vmBtn = $("vm-btn");
  vmBtn.textContent = t(on ? "engine.disconnect" : "engine.connect");
  vmBtn.classList.toggle("primary", !on);

  const connBadge = $("settings-conn-status");
  if (connBadge) {
    connBadge.textContent =
      on && info
        ? t("engine.connectedBadge", { mode: info.mode, endpoint: info.endpoint })
        : t("engine.disconnectedBadge");
    connBadge.className = "health-badge " + (on ? "healthy" : "unhealthy");
  }

  const sbDot = document.querySelector("#sb-right .dot");
  if (sbDot) sbDot.classList.toggle("on", on);
  const sbEp = $("sb-endpoint");
  sbEp.textContent = on && info ? t("status.connectedMode", { mode: info.mode }) : t("engine.disconnectedBadge");
  sbEp.title = on && info ? `${info.mode} · ${info.endpoint}` : "";
  $("sb-left").textContent =
    on && info?.runtimeVersion
      ? t("status.criLeft", { version: info.runtimeVersion })
      : t("status.defaultLeft");

  if (state.pulling.size > 0) {
    const names = Array.from(state.pulling).join(", ");
    $("sb-mid").textContent = t("status.pulling", { names });
  } else if (on) {
    $("sb-mid").textContent = t("status.summary", {
      total: state.containers.length,
      running: runningCount,
      images: state.images.length,
    });
  } else {
    $("sb-mid").textContent = "";
  }
}

function setConnected(on, info = null) {
  state.connected = on;
  state.connInfo = on ? info : null;
  if (!on) {
    state.containers = [];
    state.pods = [];
    state.images = [];
    state.volumes = [];
    state.diskUsage = null;
    state.selectedIds.clear();
    state.lastContainers = "";
    state.lastPods = "";
    state.lastImages = "";
    state.lastVolumes = "";
    stopPolling();
  } else {
    startPolling();
  }
  updateChrome();
  renderBaseCatalog();
}

async function connect(silent = false) {
  const vmBtn = $("vm-btn");
  if (vmBtn) vmBtn.disabled = true;
  if (!silent) $("vm-state").textContent = t("engine.connecting");
  try {
    const info = await invoke("connect");
    setConnected(true, info);
    if (!silent) {
      toast(
        t(info.attached ? "toast.connectedAt" : "toast.startedAt", {
          endpoint: info.endpoint,
        }),
      );
    }
    await refreshAll(true);
    return true;
  } catch (e) {
    setConnected(false);
    renderEmptyViews();
    if (!silent) toast(String(e), true);
    return false;
  } finally {
    if (vmBtn) vmBtn.disabled = false;
  }
}

async function disconnect() {
  await guard(async () => {
    await invoke("disconnect");
    setConnected(false);
    renderEmptyViews();
  });
}

function startPolling() {
  stopPolling();
  state.pollTimer = setInterval(() => {
    if (state.connected && state.view !== "settings") refreshAll(true);
  }, 2500);
}

function stopPolling() {
  if (state.pollTimer) clearInterval(state.pollTimer);
  state.pollTimer = null;
}

async function refreshAll(quiet = false) {
  if (!state.connected) {
    renderEmptyViews();
    return;
  }
  await Promise.all([
    loadContainers(quiet),
    loadImages(quiet),
    loadVolumes(quiet),
    loadPods(quiet),
  ]);
  updateChrome();
}

function updateSearchPlaceholder(view) {
  const keyMap = {
    containers: "search.containers",
    images: "search.images",
    volumes: "search.volumes",
    nextgen: "search.nextgen",
    pods: "search.pods",
    settings: "search.default",
  };
  const searchEl = $("search");
  if (searchEl) searchEl.placeholder = t(keyMap[view] || "search.default");
}

function switchView(view) {
  closeRowMenu();
  state.view = view;
  document.querySelectorAll(".nav-item").forEach((b) => {
    const active = b.dataset.view === view;
    b.classList.toggle("active", active);
    if (active) b.setAttribute("aria-current", "page");
    else b.removeAttribute("aria-current");
  });
  document.querySelectorAll("main > .view").forEach((v) => {
    const isTarget = v.id === `view-${view}`;
    v.hidden = !isTarget;
  });

  updateSearchPlaceholder(view);

  if (view === "settings") {
    loadSettingsForm();
  } else if (view === "images") {
    renderBaseCatalog();
    if (state.connected) loadImages(false);
    else renderInstalledImages([]);
  } else if (view === "volumes") {
    if (state.connected) loadVolumes(false);
    else renderVolumesEmpty();
  } else if (view === "containers") {
    renderContainerToolbar();
    if (state.connected) loadContainers(false);
    else renderContainersEmpty();
  } else if (view === "pods") {
    if (state.connected) loadPods(false);
    else renderPodsEmpty();
  }
}

function renderEmptyViews() {
  renderContainerToolbar();
  renderContainersEmpty();
  renderVolumesEmpty();
  renderPodsEmpty();
  renderBaseCatalog();
  renderInstalledImages([]);
}

function parseMappedPorts(portSpec) {
  if (!portSpec) return [];
  return String(portSpec)
    .split(/[,\n]+/)
    .map((s) => s.trim())
    .filter(Boolean)
    .map((entry) => {
      const m = entry.match(/^(?:[^:]+:)?(\d+):(\d+)(?:\/(tcp|udp))?$/i);
      if (!m) return { label: entry, hostPort: null };
      const proto = (m[3] || "tcp").toLowerCase();
      if (proto === "udp") {
        return { label: `${m[1]} -> ${m[2]}/udp`, hostPort: null };
      }
      return { label: `${m[1]} -> ${m[2]}`, hostPort: m[1] };
    });
}

async function openMappedPort(hostPort) {
  if (!hostPort) return;
  const url = `http://localhost:${hostPort}`;
  try {
    await invoke("open_external_url", { url });
  } catch (e) {
    toast(`${url}: ${e}`, true);
  }
}

// Daemon connection lifecycle, status bar & sidebar chrome updates, polling, and view routing.

function updateChrome() {
  const on = state.connected;
  const info = state.connInfo;
  const runningCount = state.containers.filter((c) => c.state === "running").length;

  $("n-containers").textContent = String(state.containers.length);
  $("n-images").textContent = String(state.images.length);
  $("n-volumes").textContent = String(state.volumes.length);
  $("n-pods").textContent = String(state.pods.length);

  $("vm-card").classList.toggle("on", on);
  $("vm-state").textContent = on ? "Running" : "Stopped";
  $("vm-sub").textContent = on && info ? `${info.mode} · ${info.endpoint}` : "Not connected";
  $("vm-running").textContent = String(runningCount);
  $("vm-pods").textContent = String(state.pods.length);
  $("vm-version").textContent = on && info?.runtimeVersion ? `v${info.runtimeVersion}` : "—";

  const vmBtn = $("vm-btn");
  vmBtn.textContent = on ? "Disconnect" : "Connect";
  vmBtn.classList.toggle("primary", !on);

  const sbDot = document.querySelector("#sb-right .dot");
  if (sbDot) sbDot.classList.toggle("on", on);
  $("sb-endpoint").textContent = on && info ? `${info.mode} · ${info.endpoint}` : "Disconnected";
  $("sb-left").textContent = on && info?.runtimeVersion
    ? `Coral v${info.runtimeVersion} · CRI v1`
    : "Coral Desktop";

  if (state.pulling.size > 0) {
    const names = Array.from(state.pulling).join(", ");
    $("sb-mid").textContent = `Pulling ${names}…`;
  } else if (on) {
    $("sb-mid").textContent = `${state.containers.length} containers (${runningCount} running) · ${state.images.length} images`;
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

async function connect() {
  $("vm-btn").disabled = true;
  try {
    const info = await invoke("connect");
    setConnected(true, info);
    toast(info.attached ? `Attached to daemon at ${info.endpoint}` : `Daemon started at ${info.endpoint}`);
    await refreshAll(false);
  } catch (e) {
    toast(String(e), true);
  } finally {
    $("vm-btn").disabled = false;
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

function switchView(view) {
  closeRowMenu();
  state.view = view;
  document.querySelectorAll(".nav-item").forEach((b) => {
    const active = b.dataset.view === view;
    b.classList.toggle("active", active);
    if (active) b.setAttribute("aria-current", "page");
    else b.removeAttribute("aria-current");
  });
  document.querySelectorAll(".view").forEach((v) => v.classList.add("hidden"));
  $(`view-${view}`).classList.remove("hidden");

  const placeholders = {
    containers: "Search containers…",
    images: "Search Linux base & installed images…",
    volumes: "Search named volumes…",
    nextgen: "Next-Gen Wasm & KVM…",
    pods: "Search pod sandboxes…",
    settings: "Search…",
  };
  $("search").placeholder = placeholders[view] || "Search…";

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

function emptyStateHtml(iconName, title, body) {
  return `
    <div class="empty">
      <div>
        <div class="art">${ICONS[iconName] || ""}</div>
        <h3>${esc(title)}</h3>
        <p>${esc(body)}</p>
      </div>
    </div>`;
}

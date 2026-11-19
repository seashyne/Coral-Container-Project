// Containers view, filter tabs, bulk actions, row context menu, Run Container modal,
// and Container Detail Drawer (logs, live ExecSync, OverlayFS CoW diff & Commit to Image).

function renderContainersEmpty() {
  $("containers-table").innerHTML = emptyStateHtml(
    "cube",
    state.connected ? "No containers yet" : "Daemon disconnected",
    state.connected
      ? "Run a container from any installed Linux base image."
      : "Configure your daemon in Settings or click Connect in the sidebar.",
  );
}

function renderContainerToolbar() {
  const total = state.containers.length;
  const running = state.containers.filter((c) => c.state === "running").length;
  const exited = state.containers.filter((c) => c.state === "exited").length;

  const tabs = [
    { key: "all", label: "All", count: total, dot: "" },
    { key: "running", label: "Running", count: running, dot: "moss" },
    { key: "exited", label: "Exited", count: exited, dot: "" },
  ];
  $("ctabs").innerHTML = tabs
    .map(
      (t) => `
      <button type="button" class="ltab" data-filter="${t.key}" aria-pressed="${state.filter === t.key}">
        ${t.dot ? `<i class="${t.dot}"></i>` : ""}
        <span>${t.label}</span>
        <span class="n">${t.count}</span>
      </button>`,
    )
    .join("");

  const selCount = state.selectedIds.size;
  const bulk = $("cbulk");
  if (selCount > 0) {
    bulk.classList.remove("hidden");
    bulk.innerHTML = `
      <span class="bcount">
        <b>${selCount}</b> selected
        <button type="button" class="x" data-bulk="clear" title="Clear selection" aria-label="Clear selection">${ICONS.x}</button>
      </span>
      <button type="button" class="btn sm" data-bulk="stop">Stop</button>
      <button type="button" class="btn sm danger" data-bulk="remove">Remove</button>`;
  } else {
    bulk.classList.add("hidden");
    bulk.innerHTML = "";
  }

  $("cright").innerHTML = `
    <button id="compose-btn" type="button" class="btn">${ICONS.hexagon}<span class="hlbl">Compose</span></button>
    <button id="run-btn" type="button" class="btn primary">${ICONS.plus}<span class="hlbl">Run container</span></button>
    <button id="refresh-containers" type="button" class="btn ic" title="Refresh" aria-label="Refresh">${ICONS.refresh}</button>`;
}

async function loadContainers(quiet) {
  try {
    const rows = await invoke("list_containers");
    state.containers = rows;
    const validIds = new Set(rows.map((r) => r.id));
    for (const id of Array.from(state.selectedIds)) {
      if (!validIds.has(id)) state.selectedIds.delete(id);
    }
    const json = JSON.stringify(rows);
    if (json !== state.lastContainers || state.view === "containers") {
      state.lastContainers = json;
      renderContainerToolbar();
      renderContainersTable();
    }
    if (state.detailId) refreshDetail(true);
  } catch (e) {
    if (!quiet) toast(String(e), true);
    if (String(e).includes("not connected")) setConnected(false);
  }
}

function filteredContainers() {
  const q = state.search.trim().toLowerCase();
  return state.containers.filter((c) => {
    if (state.filter === "running" && c.state !== "running") return false;
    if (state.filter === "exited" && c.state !== "exited") return false;
    if (!q) return true;
    return (
      c.name.toLowerCase().includes(q) ||
      c.id.toLowerCase().includes(q) ||
      c.image.toLowerCase().includes(q) ||
      c.state.toLowerCase().includes(q)
    );
  });
}

function renderContainersTable() {
  if (!state.connected) {
    renderContainersEmpty();
    return;
  }
  const rows = filteredContainers();
  if (!rows.length) {
    if (state.containers.length === 0) {
      renderContainersEmpty();
    } else {
      $("containers-table").innerHTML = emptyStateHtml(
        "search",
        "No matching containers",
        "Try clearing the filter or search query.",
      );
    }
    return;
  }

  const allSelected = rows.length > 0 && rows.every((r) => state.selectedIds.has(r.id));
  const someSelected = !allSelected && rows.some((r) => state.selectedIds.has(r.id));
  const headCbClass = allSelected ? "cb all" : someSelected ? "cb some" : "cb";

  const body = rows
    .map((c) => {
      const chosen = state.selectedIds.has(c.id);
      const pillClass = c.state === "running" ? "running" : c.state === "unknown" ? "failed" : "";
      const mono = (c.name || c.id).slice(0, 2).toUpperCase();
      const health = c.health || c.labels?.["coral.health.status"] || "";
      const healthHtml =
        health && health !== "none"
          ? `<span class="health-badge ${esc(health)}">${esc(health)}</span>`
          : "";
      return `
        <tr class="row ${chosen ? "chosen" : ""}" data-cid="${esc(c.id)}">
          <td class="ck">
            <div class="ckbox" data-ck="${esc(c.id)}" role="checkbox" aria-checked="${chosen}" tabindex="0">
              <span class="cb">${ICONS.check}</span>
            </div>
          </td>
          <td>
            <div class="name">
              <span class="tile cmp">${esc(mono)}</span>
              <div>
                <div class="nm" data-act="logs" data-id="${esc(c.id)}">${esc(c.name)}</div>
                <div class="sub-name mono" title="${esc(c.id)}">${esc(shortId(c.id))}</div>
              </div>
            </div>
          </td>
          <td class="mono">${esc(c.image)}</td>
          <td><span class="pill ${pillClass}">${esc(c.state)}</span>${healthHtml}</td>
          <td class="muted">${esc(fmtTime(c.createdAt))}</td>
          <td class="actions">
            <div class="acts-row">
              <button type="button" class="btn ic" data-act="term" data-id="${esc(c.id)}" title="Open Terminal" aria-label="Open Terminal">${ICONS.terminal}</button>
              ${
                c.state === "running"
                  ? `<button type="button" class="btn ic halt" data-act="stop" data-id="${esc(c.id)}" title="Stop" aria-label="Stop">${ICONS.square}</button>`
                  : ""
              }
              <button type="button" class="btn ic danger" data-act="remove" data-id="${esc(c.id)}" title="Remove" aria-label="Remove">${ICONS.trash}</button>
              <button type="button" class="btn ic" data-menu-id="${esc(c.id)}" title="More actions" aria-label="More actions">${ICONS.more}</button>
            </div>
          </td>
        </tr>`;
    })
    .join("");

  $("containers-table").innerHTML = `
    <table>
      <thead>
        <tr>
          <th class="ck">
            <div class="ckbox" data-ck-all="1" role="checkbox" aria-checked="${allSelected}" tabindex="0">
              <span class="${headCbClass}">${ICONS.check}</span>
            </div>
          </th>
          <th>Name</th>
          <th>Image</th>
          <th>State</th>
          <th>Created</th>
          <th class="r">Actions</th>
        </tr>
      </thead>
      <tbody>${body}</tbody>
    </table>`;
}

async function containerAction(act, id) {
  closeRowMenu();
  if (act === "logs") return openDetail(id, "logs");
  if (act === "term") return openDetail(id, "term");
  if (act === "metrics") return openDetail(id, "metrics");
  if (act === "stop") {
    await guard(async () => {
      await invoke("stop_container", { id });
      toast("Container stopped");
      await loadContainers(false);
      updateChrome();
    });
  } else if (act === "remove") {
    await guard(async () => {
      await invoke("remove_container", { id });
      state.selectedIds.delete(id);
      toast("Container removed");
      if (state.detailId === id) closeDetail();
      await loadContainers(false);
      updateChrome();
    });
  }
}

async function bulkContainerAction(action) {
  if (action === "clear") {
    state.selectedIds.clear();
    renderContainerToolbar();
    renderContainersTable();
    return;
  }
  const ids = Array.from(state.selectedIds);
  if (!ids.length) return;
  await guard(async () => {
    for (const id of ids) {
      if (action === "stop") await invoke("stop_container", { id });
      else if (action === "remove") await invoke("remove_container", { id });
    }
    if (action === "remove") {
      if (state.detailId && state.selectedIds.has(state.detailId)) closeDetail();
      state.selectedIds.clear();
    }
    toast(`${action === "stop" ? "Stopped" : "Removed"} ${ids.length} container(s)`);
    await loadContainers(false);
    updateChrome();
  });
}

function openRowMenu(btn, id) {
  const c = state.containers.find((row) => row.id === id);
  if (!c) return;
  state.menuTarget = id;
  const menu = $("row-menu");
  menu.innerHTML = `
    <button type="button" data-menu-act="logs">${ICONS.terminal}<span>Live Logs</span></button>
    <button type="button" data-menu-act="term">${ICONS.terminal}<span>Open Terminal</span></button>
    <button type="button" data-menu-act="metrics">${ICONS.sliders}<span>Metrics &amp; CoW</span></button>
    ${c.state === "running" ? `<button type="button" data-menu-act="stop">${ICONS.square}<span>Stop container</span></button>` : ""}
    <div class="sep"></div>
    <button type="button" class="danger" data-menu-act="remove">${ICONS.trash}<span>Remove container</span></button>`;
  menu.classList.remove("hidden");
  const rect = btn.getBoundingClientRect();
  const mw = menu.offsetWidth || 180;
  const mh = menu.offsetHeight || 120;
  const left = Math.max(8, Math.min(rect.right - mw, window.innerWidth - mw - 8));
  const top = rect.bottom + mh + 8 > window.innerHeight ? rect.top - mh - 4 : rect.bottom + 4;
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
}

function closeRowMenu() {
  state.menuTarget = null;
  $("row-menu").classList.add("hidden");
}

function openRunModal(preset = null) {
  if (preset) {
    const baseSlug = (preset.slug || preset.image || "container")
      .split("/")
      .pop()
      .split(":")[0]
      .replace(/[^a-zA-Z0-9_-]/g, "-");
    $("run-name").value = `${baseSlug}-${state.runSeq++}`;
    $("run-image").value = preset.image || "";
    $("run-command").value = preset.command ?? "/bin/sh";
    $("run-args").value = preset.args ?? '-c "cat /etc/os-release || uname -a"';
  }
  $("run-modal").classList.remove("hidden");
  $("run-name").focus();
}

async function submitRun() {
  const params = {
    name: $("run-name").value.trim(),
    image: $("run-image").value.trim() || "alpine:latest",
    command: $("run-command").value.trim(),
    args: $("run-args").value,
    memoryMb: Number($("run-memory").value) || 0,
    cpuPercent: Number($("run-cpu").value) || 0,
    pidsLimit: Number($("run-pids").value) || 0,
    network: $("run-network").value,
    restartPolicy: $("run-restart").value,
    healthCmd: $("run-health-cmd").value.trim(),
    healthIntervalSecs: 5,
    ports: $("run-ports").value,
    volumes: $("run-volumes").value,
    env: $("run-env").value,
  };
  $("run-submit").disabled = true;
  try {
    await guard(async () => {
      const id = await invoke("run_container", { params });
      $("run-modal").classList.add("hidden");
      toast(`Container ${params.name} started`);
      if (state.view !== "containers") switchView("containers");
      await loadContainers(false);
      await loadPods(true);
      updateChrome();
      openDetail(id, "logs");
    });
  } finally {
    $("run-submit").disabled = false;
  }
}

async function openDetail(id, tabKey = "logs") {
  state.detailId = id;
  state.baseLog = "";
  $("drawer-backdrop").classList.remove("hidden");
  $("detail-drawer").classList.remove("hidden");
  switchDetailTab(tabKey, false);
  await refreshDetail();
}

function closeDetail() {
  if (state.detailId) disconnectLivePty(state.detailId);
  state.detailId = null;
  $("drawer-backdrop").classList.add("hidden");
  $("detail-drawer").classList.add("hidden");
}

async function refreshDetail(quiet = false) {
  const id = state.detailId;
  if (!id) return;
  try {
    const d = await invoke("container_detail", { id });
    const row = state.containers.find((c) => c.id === id);
    const displayName = row?.name || id;
    $("detail-title").textContent = displayName;
    $("detail-tile").textContent = displayName.slice(0, 2).toUpperCase();
    $("detail-tile").className = "tile cmp";
    const pillClass = d.state === "running" ? "running" : d.state === "unknown" ? "failed" : "";
    const healthHtml =
      d.healthStatus && d.healthStatus !== "none"
        ? `<span class="health-badge ${esc(d.healthStatus)}">${esc(d.healthStatus)}</span>`
        : "";
    $("detail-meta").innerHTML = `
      <span class="k">Container ID</span><span class="v mono">${esc(id)}</span>
      <span class="k">State</span><span class="v"><span class="pill ${pillClass}">${esc(d.state)}</span>${healthHtml}</span>
      <span class="k">Image</span><span class="v mono">${esc(d.image)}</span>
      <span class="k">Restart</span><span class="v mono">${esc(d.restartPolicy || "no")} (${esc(d.restartCount || 0)} restarts)</span>
      <span class="k">Exit / Reason</span><span class="v mono">${esc(d.exitCode)} ${d.reason ? `• ${esc(d.reason)}` : ""}</span>
      <span class="k">Started</span><span class="v">${esc(fmtTime(d.startedAt))}</span>
      ${d.message ? `<span class="k">Message</span><span class="v">${esc(d.message)}</span>` : ""}`;

    const logBadge = $("log-live-badge");
    if (logBadge) {
      logBadge.textContent = d.state === "running" ? "Streaming Live" : `Snapshot (${d.state})`;
      logBadge.className = "live-badge" + (d.state === "running" ? " live" : "");
    }

    const logEl = $("detail-log");
    const nextLog = d.log || "(no output yet)";
    const changed = nextLog !== state.baseLog;
    state.baseLog = nextLog;
    if (logEl && (changed || !logEl.textContent)) {
      logEl.textContent = nextLog;
      if (state.logAutoScroll) {
        logEl.scrollTop = logEl.scrollHeight;
      }
    }

    if ($("detail-overlay-diff")) {
      $("detail-overlay-diff").textContent =
        d.overlayDiff && d.overlayDiff.trim()
          ? d.overlayDiff
          : "(no OverlayFS changes recorded)";
    }

    renderMetricsPanel(id, d);
  } catch (e) {
    if (!quiet) toast(String(e), true);
    if (String(e).includes("not found")) closeDetail();
  }
}

async function commitContainer() {
  if (!state.detailId) return;
  const imageName = $("commit-image-name").value.trim();
  if (!imageName) {
    toast("Enter a target image tag (e.g. my-app:v1)", true);
    return;
  }
  $("commit-btn").disabled = true;
  try {
    await guard(async () => {
      await invoke("commit_container", { id: state.detailId, imageName });
      toast(`Committed container to ${imageName}`);
      $("commit-image-name").value = "";
      await loadImages(false);
      updateChrome();
    });
  } finally {
    $("commit-btn").disabled = false;
  }
}

function openComposeModal() {
  $("compose-modal").classList.remove("hidden");
  $("compose-project").focus();
}

async function submitComposeUp() {
  const yaml = $("compose-yaml").value;
  const projectName = $("compose-project").value.trim() || null;
  const baseDir = $("compose-basedir").value.trim() || null;
  if (!yaml.trim()) {
    toast("Enter Compose YAML definition", true);
    return;
  }
  $("compose-up-btn").disabled = true;
  try {
    await guard(async () => {
      const res = await invoke("compose_up", { yaml, projectName, baseDir });
      $("compose-modal").classList.add("hidden");
      toast(`Compose stack started (${res})`);
      await refreshAll(false);
    });
  } finally {
    $("compose-up-btn").disabled = false;
  }
}

async function submitComposeDown() {
  const projectName = $("compose-project").value.trim();
  if (!projectName) {
    toast("Enter the Compose project name to tear down", true);
    return;
  }
  $("compose-down-btn").disabled = true;
  try {
    await guard(async () => {
      const removed = await invoke("compose_down", { projectName });
      $("compose-modal").classList.add("hidden");
      toast(`Stopped & removed ${removed} container(s) in '${projectName}'`);
      await refreshAll(false);
    });
  } finally {
    $("compose-down-btn").disabled = false;
  }
}

// Named Volumes management and System Disk Usage (`df`) / Prune UI for Coral Desktop.

function renderVolumesEmpty() {
  if ($("sys-df-cards")) {
    $("sys-df-cards").innerHTML = "";
  }
  if ($("volumes-table")) {
    $("volumes-table").innerHTML = emptyStateHtml(
      "database",
      state.connected ? "No named volumes" : "Daemon disconnected",
      state.connected
        ? "Create a persistent named volume above or mount one with `-v my-data:/data`."
        : "Connect to a daemon to manage volumes and inspect disk usage.",
    );
  }
}

async function loadVolumes(quiet) {
  try {
    const [vols, df] = await Promise.all([
      invoke("list_volumes"),
      invoke("system_df"),
    ]);
    state.volumes = vols;
    state.diskUsage = df;
    const snap = JSON.stringify({ vols, df });
    if (snap !== state.lastVolumes || state.view === "volumes") {
      state.lastVolumes = snap;
      renderDiskUsage();
      renderVolumesTable();
    }
  } catch (e) {
    if (!quiet) toast(String(e), true);
    if (String(e).includes("not connected")) setConnected(false);
  }
}

function renderDiskUsage() {
  const host = $("sys-df-cards");
  if (!host) return;
  const df = state.diskUsage;
  if (!state.connected || !df) {
    host.innerHTML = "";
    return;
  }
  host.innerHTML = `
    <div class="metric-card">
      <span class="mk">Images</span>
      <span class="mv">${esc(fmtBytes(df.imagesBytes))}</span>
      <span class="ms">${esc(df.imagesCount)} installed</span>
    </div>
    <div class="metric-card">
      <span class="mk">Containers</span>
      <span class="mv">${esc(fmtBytes(df.containersBytes))}</span>
      <span class="ms">${esc(df.containersRunning)} running · ${esc(df.containersExited)} exited</span>
    </div>
    <div class="metric-card">
      <span class="mk">Named Volumes</span>
      <span class="mv">${esc(fmtBytes(df.volumesBytes))}</span>
      <span class="ms">${esc(df.volumesCount)} volume(s)</span>
    </div>
    <div class="metric-card">
      <span class="mk">Build Cache</span>
      <span class="mv">${esc(fmtBytes(df.cacheBytes))}</span>
      <span class="ms">${esc(df.cacheEntries)} layer(s) · Total ${esc(fmtBytes(df.totalBytes))}</span>
    </div>`;
}

function renderVolumesTable() {
  if (!state.connected) {
    renderVolumesEmpty();
    return;
  }
  const q = state.search.trim().toLowerCase();
  const rows = state.volumes.filter(
    (v) =>
      !q ||
      v.name.toLowerCase().includes(q) ||
      v.mountpoint.toLowerCase().includes(q) ||
      (v.usedBy || []).some((u) => u.toLowerCase().includes(q)),
  );
  if (!rows.length) {
    if (!state.volumes.length) {
      $("volumes-table").innerHTML = emptyStateHtml(
        "database",
        "No named volumes",
        "Create a persistent named volume above or mount one with `-v my-data:/data`.",
      );
    } else {
      $("volumes-table").innerHTML = emptyStateHtml(
        "search",
        "No matching volumes",
        "Try clearing the search query.",
      );
    }
    return;
  }

  const body = rows
    .map((v) => {
      const used = (v.usedBy || []).length
        ? v.usedBy.map((u) => `<span class="spec-chip">${esc(u)}</span>`).join(" ")
        : '<span class="muted">Unused</span>';
      return `
        <tr class="row">
          <td>
            <div class="name">
              <span class="tile cmp">${esc(v.name.slice(0, 2).toUpperCase())}</span>
              <div>
                <div class="nm">${esc(v.name)}</div>
                <div class="sub-name mono" title="${esc(v.mountpoint)}">${esc(v.mountpoint)}</div>
              </div>
            </div>
          </td>
          <td class="mono">${esc(fmtBytes(v.size))}</td>
          <td>${used}</td>
          <td class="actions">
            <div class="acts-row">
              <button type="button" class="btn ic danger" data-vol-remove="${esc(v.name)}" title="Remove volume" aria-label="Remove volume">${ICONS.trash}</button>
            </div>
          </td>
        </tr>`;
    })
    .join("");

  $("volumes-table").innerHTML = `
    <table>
      <thead>
        <tr>
          <th>Volume</th>
          <th>Size</th>
          <th>Used By</th>
          <th class="r">Actions</th>
        </tr>
      </thead>
      <tbody>${body}</tbody>
    </table>`;
}

async function createVolumeSubmit(e) {
  e.preventDefault();
  const input = $("vol-name-input");
  const name = input.value.trim();
  if (!name) return;
  $("vol-create-btn").disabled = true;
  try {
    await guard(async () => {
      await invoke("create_volume", { name });
      input.value = "";
      toast(`Created volume '${name}'`);
      await loadVolumes(false);
      updateChrome();
    });
  } finally {
    $("vol-create-btn").disabled = false;
  }
}

async function removeVolume(name) {
  await guard(async () => {
    await invoke("remove_volume", { name });
    toast(`Removed volume '${name}'`);
    await loadVolumes(false);
    updateChrome();
  });
}

function openPruneModal() {
  $("prune-all").checked = false;
  $("prune-volumes").checked = false;
  $("prune-modal").classList.remove("hidden");
}

async function submitSystemPrune() {
  const all = $("prune-all").checked;
  const volumes = $("prune-volumes").checked;
  $("prune-submit").disabled = true;
  try {
    await guard(async () => {
      const r = await invoke("system_prune", { all, volumes });
      $("prune-modal").classList.add("hidden");
      toast(
        `Pruned ${r.containersRemoved} container(s), ${r.cacheEntriesRemoved} cache layer(s), reclaimed ${fmtBytes(r.bytesReclaimed)}`,
      );
      await refreshAll(false);
    });
  } finally {
    $("prune-submit").disabled = false;
  }
}

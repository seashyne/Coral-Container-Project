// Named Volumes management, System Storage Overview (`df`), and Reclaim Disk Space UI.

function renderVolumesEmpty() {
  if ($("sys-df-cards")) {
    $("sys-df-cards").innerHTML = "";
  }
  if ($("volumes-table")) {
    $("volumes-table").innerHTML = emptyStateHtml(
      "database",
      state.connected ? t("volumes.emptyTitle") : t("images.disconnectedTitle"),
      state.connected ? t("volumes.emptySub") : t("volumes.disconnectedSub"),
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
      <span class="mk">${esc(t("volumes.dfImages"))}</span>
      <span class="mv">${esc(fmtBytes(df.imagesBytes))}</span>
      <span class="ms">${esc(t("volumes.dfImagesSub", { count: df.imagesCount }))}</span>
    </div>
    <div class="metric-card">
      <span class="mk">${esc(t("volumes.dfContainers"))}</span>
      <span class="mv">${esc(fmtBytes(df.containersBytes))}</span>
      <span class="ms">${esc(t("volumes.dfContainersSub", { running: df.containersRunning, exited: df.containersExited }))}</span>
    </div>
    <div class="metric-card">
      <span class="mk">${esc(t("volumes.dfVolumes"))}</span>
      <span class="mv">${esc(fmtBytes(df.volumesBytes))}</span>
      <span class="ms">${esc(t("volumes.dfVolumesSub", { count: df.volumesCount }))}</span>
    </div>
    <div class="metric-card">
      <span class="mk">${esc(t("volumes.dfCache"))}</span>
      <span class="mv">${esc(fmtBytes(df.cacheBytes))}</span>
      <span class="ms">${esc(t("volumes.dfCacheSub", { count: df.cacheEntries, total: fmtBytes(df.totalBytes) }))}</span>
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
      renderVolumesEmpty();
    } else {
      $("volumes-table").innerHTML = emptyStateHtml(
        "search",
        t("volumes.noMatchTitle"),
        t("volumes.noMatchSub"),
      );
    }
    return;
  }

  const body = rows
    .map((v) => {
      const used = (v.usedBy || []).length
        ? v.usedBy.map((u) => `<span class="spec-chip">${esc(u)}</span>`).join(" ")
        : `<span class="muted">${esc(t("common.unused"))}</span>`;
      return `
        <tr class="row">
          <td data-label="${esc(t("volumes.colVolume"))}">
            <div class="name">
              <span class="tile cmp">${esc(v.name.slice(0, 2).toUpperCase())}</span>
              <div>
                <div class="nm" title="${esc(v.mountpoint)}">${esc(v.name)}</div>
              </div>
            </div>
          </td>
          <td class="mono" data-label="${esc(t("volumes.colSize"))}">${esc(fmtBytes(v.size))}</td>
          <td data-label="${esc(t("volumes.colUsedBy"))}">${used}</td>
          <td class="actions">
            <div class="acts-row">
              <button type="button" class="btn ic danger" data-vol-remove="${esc(v.name)}" title="${esc(t("volumes.removeTitle"))}" aria-label="${esc(t("volumes.removeTitle"))}">${ICONS.trash}</button>
            </div>
          </td>
        </tr>`;
    })
    .join("");

  $("volumes-table").innerHTML = `
    <div class="responsive-table-wrap" tabindex="0" role="region" aria-label="${esc(t("volumes.title"))}">
      <table>
        <thead>
          <tr>
            <th scope="col">${esc(t("volumes.colVolume"))}</th>
            <th scope="col">${esc(t("volumes.colSize"))}</th>
            <th scope="col">${esc(t("volumes.colUsedBy"))}</th>
            <th scope="col" class="r">${esc(t("volumes.colActions"))}</th>
          </tr>
        </thead>
        <tbody>${body}</tbody>
      </table>
    </div>`;
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
      toast(t("toast.volCreated", { name }));
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
    toast(t("toast.volRemoved", { name }));
    await loadVolumes(false);
    updateChrome();
  });
}

function openPruneModal() {
  if ($("prune-all")) $("prune-all").checked = false;
  if ($("prune-volumes")) $("prune-volumes").checked = false;
  openDialog("prune-dialog");
}

async function submitSystemPrune() {
  const all = Boolean($("prune-all")?.checked);
  const volumes = Boolean($("prune-volumes")?.checked);
  $("prune-submit").disabled = true;
  try {
    await guard(async () => {
      const r = await invoke("system_prune", { all, volumes });
      closeDialog("prune-dialog");
      toast(
        t("toast.pruned", {
          containers: r.containersRemoved,
          layers: r.cacheEntriesRemoved,
          bytes: fmtBytes(r.bytesReclaimed),
        }),
      );
      await refreshAll(false);
    });
  } finally {
    $("prune-submit").disabled = false;
  }
}

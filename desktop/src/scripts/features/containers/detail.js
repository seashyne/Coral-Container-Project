// Containers view: detail drawer, live refresh and image commit.
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
        ? `<span class="health-badge ${esc(d.healthStatus)}">${esc(tHealth(d.healthStatus))}</span>`
        : "";
    $("detail-meta").innerHTML = `
      <span class="k">${esc(t("drawer.metaId"))}</span><span class="v mono">${esc(id)}</span>
      <span class="k">${esc(t("drawer.metaState"))}</span><span class="v"><span class="pill ${pillClass}">${esc(tState(d.state))}</span>${healthHtml}</span>
      <span class="k">${esc(t("drawer.metaImage"))}</span><span class="v mono">${esc(d.image)}</span>
      <span class="k">${esc(t("drawer.metaRestart"))}</span><span class="v mono">${esc(t("drawer.restartsCount", { policy: d.restartPolicy || "no", count: d.restartCount || 0 }))}</span>
      <span class="k">${esc(t("drawer.metaExit"))}</span><span class="v mono">${esc(d.exitCode)} ${d.reason ? `• ${esc(d.reason)}` : ""}</span>
      <span class="k">${esc(t("drawer.metaStarted"))}</span><span class="v">${esc(fmtTime(d.startedAt))}</span>
      ${d.message ? `<span class="k">${esc(t("drawer.metaMessage"))}</span><span class="v">${esc(d.message)}</span>` : ""}`;

    const logBadge = $("log-live-badge");
    if (logBadge) {
      logBadge.textContent =
        d.state === "running"
          ? t("drawer.streamingLive")
          : t("drawer.snapshotState", { state: tState(d.state) });
      logBadge.className = "live-badge" + (d.state === "running" ? " live" : "");
    }

    const logEl = $("detail-log");
    const nextLog = d.log || t("drawer.noOutputYet");
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
        d.overlayDiff && d.overlayDiff.trim() ? d.overlayDiff : t("drawer.noCowChanges");
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
    toast(t("toast.enterCommitTag"), true);
    return;
  }
  $("commit-btn").disabled = true;
  try {
    await guard(async () => {
      await invoke("commit_container", { id: state.detailId, imageName });
      toast(t("toast.committed", { tag: imageName }));
      $("commit-image-name").value = "";
      await loadImages(false);
      updateChrome();
    });
  } finally {
    $("commit-btn").disabled = false;
  }
}


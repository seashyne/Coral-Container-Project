// Containers view: row/stack/bulk actions, run presets and the run modal.
async function containerAction(act, id) {
  closeRowMenu();
  if (act === "logs") return openDetail(id, "logs");
  if (act === "term") return openDetail(id, "term");
  if (act === "metrics") return openDetail(id, "metrics");
  if (act === "start") {
    await guard(async () => {
      await invoke("start_container", { id });
      toast(t("toast.ctrStarted"));
      await loadContainers(false);
      updateChrome();
    });
  } else if (act === "stop") {
    await guard(async () => {
      await invoke("stop_container", { id });
      toast(t("toast.ctrStopped"));
      await loadContainers(false);
      updateChrome();
    });
  } else if (act === "pause" || act === "unpause") {
    await guard(async () => {
      await invoke(act === "pause" ? "pause_container" : "unpause_container", { id });
      toast(t(act === "pause" ? "containers.toastPaused" : "containers.toastResumed"));
      await loadContainers(false);
      updateChrome();
    });
  } else if (act === "rename") {
    const cur = state.containers.find((r) => r.id === id)?.name || "";
    $("rename-cid").value = id;
    $("rename-input").value = cur;
    openDialog("rename-dialog");
    $("rename-input").focus();
  } else if (act === "update") {
    $("upd-cid").value = id;
    $("upd-mem").value = "";
    $("upd-cpu").value = "";
    $("upd-pids").value = "";
    openDialog("update-limits-dialog");
    $("upd-mem").focus();
  } else if (act === "top") {
    await openContainerTop(id);
  } else if (act === "remove") {
    await guard(async () => {
      await invoke("remove_container", { id });
      state.selectedIds.delete(id);
      toast(t("toast.ctrRemoved"));
      if (state.detailId === id) closeDetail();
      await loadContainers(false);
      updateChrome();
    });
  }
}

async function stackAction(act, projectName) {
  await guard(async () => {
    if (act === "down") {
      const removed = await invoke("compose_down", { projectName });
      toast(t("toast.stackRemoved", { name: projectName, count: removed }));
    } else if (act === "stop") {
      const members = state.containers.filter(
        (c) => c.labels?.["coral.compose.project"] === projectName && c.state === "running",
      );
      for (const m of members) {
        await invoke("stop_container", { id: m.id });
      }
      toast(t("toast.stackStopped", { name: projectName }));
    }
    await refreshAll(false);
  });
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
      if (action === "start") await invoke("start_container", { id });
      else if (action === "stop") await invoke("stop_container", { id });
      else if (action === "remove") await invoke("remove_container", { id });
    }
    if (action === "remove") {
      if (state.detailId && state.selectedIds.has(state.detailId)) closeDetail();
      state.selectedIds.clear();
    }
    const toastKey =
      action === "start"
        ? "toast.bulkStarted"
        : action === "stop"
          ? "toast.bulkStopped"
          : "toast.bulkRemoved";
    toast(t(toastKey, { count: ids.length }));
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
    <button type="button" data-menu-act="logs">${ICONS.logs}<span>${esc(t("containers.menuLogs"))}</span></button>
    <button type="button" data-menu-act="term">${ICONS.terminal}<span>${esc(t("containers.menuTerm"))}</span></button>
    <button type="button" data-menu-act="metrics">${ICONS.sliders}<span>${esc(t("containers.menuMetrics"))}</span></button>
    ${
      c.state === "running"
        ? `${
            c.paused
              ? `<button type="button" data-menu-act="unpause">${ICONS.play}<span>${esc(t("containers.menuUnpause"))}</span></button>`
              : `<button type="button" data-menu-act="pause">${ICONS.sliders}<span>${esc(t("containers.menuPause"))}</span></button>`
          }
          <button type="button" data-menu-act="top">${ICONS.terminal}<span>${esc(t("containers.menuTop"))}</span></button>
          <button type="button" data-menu-act="stop">${ICONS.square}<span>${esc(t("containers.stopContainer"))}</span></button>`
        : `<button type="button" data-menu-act="start">${ICONS.play}<span>${esc(t("containers.startContainer"))}</span></button>`
    }
    <button type="button" data-menu-act="rename">${ICONS.sliders}<span>${esc(t("containers.menuRename"))}</span></button>
    <button type="button" data-menu-act="update">${ICONS.sliders}<span>${esc(t("containers.menuUpdate"))}</span></button>
    <div class="sep"></div>
    <button type="button" class="danger" data-menu-act="remove">${ICONS.trash}<span>${esc(t("containers.removeContainer"))}</span></button>`;
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

function syncRunMemoryPills() {
  const cur = String(Number($("run-memory")?.value) || 0);
  document.querySelectorAll("[data-run-mem]").forEach((btn) => {
    btn.setAttribute("aria-pressed", btn.dataset.runMem === cur ? "true" : "false");
  });
}

function syncRunPresetChips(activeKey = "") {
  document.querySelectorAll("[data-run-preset]").forEach((btn) => {
    btn.setAttribute("aria-pressed", btn.dataset.runPreset === activeKey ? "true" : "false");
  });
}

function applyRunPreset(preset, presetKey = "") {
  if (!preset) return;
  const baseSlug = (preset.slug || preset.image || "container")
    .split("/")
    .pop()
    .split(":")[0]
    .replace(/[^a-zA-Z0-9_-]/g, "-");
  $("run-name").value = `${baseSlug}-${state.runSeq++}`;
  $("run-image").value = preset.image || "";
  $("run-ports").value = preset.ports ?? "";
  $("run-volumes").value = preset.volumes ?? "";
  $("run-env").value = preset.env ?? "";
  $("run-command").value = preset.command ?? "";
  $("run-args").value = preset.args ?? "";
  $("run-memory").value = String(preset.memoryMb ?? 0);
  if ($("run-keep-alive")) {
    $("run-keep-alive").checked =
      preset.command === "/bin/sh" && String(preset.args || "").includes("sleep");
  }
  syncRunMemoryPills();
  syncRunPresetChips(presetKey);
}

function openRunModal(preset = null) {
  if (preset) {
    applyRunPreset(preset, preset.presetKey || "");
  } else {
    $("run-name").value = "";
    $("run-image").value = "";
    $("run-ports").value = "";
    $("run-volumes").value = "";
    $("run-env").value = "";
    $("run-command").value = "";
    $("run-args").value = "";
    $("run-memory").value = "0";
    $("run-cpu").value = "0";
    $("run-pids").value = "0";
    if ($("run-health-cmd")) $("run-health-cmd").value = "";
    if ($("run-network-aliases")) $("run-network-aliases").value = "";
    if ($("run-gpus")) $("run-gpus").value = "";
    if ($("run-devices")) $("run-devices").value = "";
    if ($("run-log-driver")) $("run-log-driver").value = "raw";
    if ($("run-keep-alive")) $("run-keep-alive").checked = false;
    syncRunMemoryPills();
    syncRunPresetChips("");
  }
  openDialog("run-dialog");
}

async function ensureImageInstalled(imageRef) {
  const existing = state.images.find((i) => i.repoTags.includes(imageRef));
  if (existing) return true;
  await pullOciImage(imageRef);
  return true;
}

async function openContainerTop(id) {
  $("top-cid").value = id;
  $("top-output").textContent = "Loading processes…";
  openDialog("top-dialog");
  await guard(async () => {
    const procs = await invoke("container_top", { id });
    if (!Array.isArray(procs) || !procs.length) {
      $("top-output").textContent = "(no active processes)";
      return;
    }
    const header = "PID      PPID     STATE        RSS        CMD";
    const lines = procs.map(
      (p) =>
        `${String(p.pid).padEnd(9)}${String(p.ppid).padEnd(9)}${String(p.state).padEnd(13)}${`${p.rssKb || 0} KiB`.padEnd(11)}${p.cmd || ""}`,
    );
    $("top-output").textContent = [header, ...lines].join("\n");
  });
}

async function submitRenameContainer(e) {
  e.preventDefault();
  const id = $("rename-cid").value;
  const newName = $("rename-input").value.trim();
  if (!id || !newName) return;
  $("rename-submit").disabled = true;
  try {
    await guard(async () => {
      await invoke("rename_container", { id, newName });
      toast(t("containers.toastRenamed", { name: newName }));
      closeDialog("rename-dialog");
      await loadContainers(false);
      updateChrome();
    });
  } finally {
    $("rename-submit").disabled = false;
  }
}

async function submitUpdateLimits(e) {
  e.preventDefault();
  const id = $("upd-cid").value;
  const memoryMb = Number($("upd-mem").value) > 0 ? Math.round(Number($("upd-mem").value)) : null;
  const cpuPercent = Number($("upd-cpu").value) > 0 ? Math.round(Number($("upd-cpu").value)) : null;
  const pidsLimit = Number($("upd-pids").value) > 0 ? Math.round(Number($("upd-pids").value)) : null;
  if (!id || (!memoryMb && !cpuPercent && !pidsLimit)) return;
  $("upd-submit").disabled = true;
  try {
    await guard(async () => {
      await invoke("update_container", { id, memoryMb, cpuPercent, pidsLimit });
      toast(t("containers.toastUpdated"));
      closeDialog("update-limits-dialog");
      await loadContainers(false);
    });
  } finally {
    $("upd-submit").disabled = false;
  }
}


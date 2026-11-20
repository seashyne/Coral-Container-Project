// Event bindings, keyboard shortcuts, and application startup bootstrap for Coral Desktop.

$("rail-btn").addEventListener("click", toggleRail);
$("theme-btn").addEventListener("click", cycleTheme);
$("gear-btn").addEventListener("click", () => switchView("settings"));
$("set-theme").addEventListener("change", (e) => applyTheme(e.target.value));

document.querySelectorAll(".nav-item").forEach((b) =>
  b.addEventListener("click", () => switchView(b.dataset.view)),
);

$("vm-btn").addEventListener("click", () =>
  state.connected ? disconnect() : connect(),
);

$("search").addEventListener("input", (e) => {
  state.search = e.target.value;
  if (state.view === "containers") renderContainersTable();
  else if (state.view === "images") {
    renderBaseCatalog();
    renderInstalledImages(state.images);
  } else if (state.view === "volumes") {
    renderVolumesTable();
  } else if (state.view === "pods") {
    renderPodsTable();
  }
});

// Containers toolbar events (delegated)
$("ctabs").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-filter]");
  if (!btn) return;
  state.filter = btn.dataset.filter;
  renderContainerToolbar();
  renderContainersTable();
});

$("cbulk").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-bulk]");
  if (btn) bulkContainerAction(btn.dataset.bulk);
});

$("cright").addEventListener("click", (e) => {
  if (e.target.closest("#compose-btn")) openComposeModal();
  else if (e.target.closest("#run-btn")) openRunModal();
  else if (e.target.closest("#refresh-containers")) loadContainers(false);
});

$("containers-table").addEventListener("click", (e) => {
  const ckAll = e.target.closest("[data-ck-all]");
  if (ckAll) {
    const visible = filteredContainers();
    const allSel = visible.every((r) => state.selectedIds.has(r.id));
    if (allSel) visible.forEach((r) => state.selectedIds.delete(r.id));
    else visible.forEach((r) => state.selectedIds.add(r.id));
    renderContainerToolbar();
    renderContainersTable();
    return;
  }
  const ck = e.target.closest("[data-ck]");
  if (ck) {
    const id = ck.dataset.ck;
    if (state.selectedIds.has(id)) state.selectedIds.delete(id);
    else state.selectedIds.add(id);
    renderContainerToolbar();
    renderContainersTable();
    return;
  }
  const menuBtn = e.target.closest("button[data-menu-id]");
  if (menuBtn) {
    e.stopPropagation();
    openRowMenu(menuBtn, menuBtn.dataset.menuId);
    return;
  }
  const actEl = e.target.closest("[data-act]");
  if (actEl) containerAction(actEl.dataset.act, actEl.dataset.id);
});

$("row-menu").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-menu-act]");
  if (btn && state.menuTarget) {
    containerAction(btn.dataset.menuAct, state.menuTarget);
  }
});

// Images view events
$("add-image-btn").addEventListener("click", () => {
  $("image-modal").classList.remove("hidden");
  $("oci-image-ref").focus();
});
$("refresh-images").addEventListener("click", () => loadImages(false));

$("base-catalog").addEventListener("click", (e) => {
  const pullBtn = e.target.closest("button[data-base-pull]");
  if (pullBtn) {
    pullOciImage(pullBtn.dataset.basePull);
    return;
  }
  const runBtn = e.target.closest("button[data-base-run]");
  if (runBtn) {
    const item = BASE_IMAGES.find((b) => b.id === runBtn.dataset.baseRun);
    if (item) {
      openRunModal({
        slug: item.id,
        image: runBtn.dataset.runTag || item.ref,
        command: item.defaultCmd,
        args: item.defaultArgs,
      });
    }
  }
});

$("images-table").addEventListener("click", (e) => {
  const remBtn = e.target.closest("button[data-img-remove]");
  if (remBtn) {
    removeImage(remBtn.dataset.imgRemove);
    return;
  }
  const runEl = e.target.closest("[data-img-run]");
  if (runEl) {
    const tag = runEl.dataset.imgRun;
    const base = BASE_IMAGES.find((b) => b.matchTags.includes(tag));
    openRunModal({
      slug: base ? base.id : tag,
      image: tag,
      command: base ? base.defaultCmd : "/bin/sh",
      args: base ? base.defaultArgs : '-c "cat /etc/os-release || uname -a"',
    });
  }
});

$("oci-pull-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const ref = $("oci-image-ref").value.trim();
  if (!ref) return;
  $("oci-pull-submit").disabled = true;
  $("image-modal").classList.add("hidden");
  $("oci-image-ref").value = "";
  try {
    await pullOciImage(ref);
  } finally {
    $("oci-pull-submit").disabled = false;
  }
});

$("local-rootfs-form").addEventListener("submit", registerLocalImage);
$("dockerfile-build-form").addEventListener("submit", buildDockerfileImage);
$("archive-load-form").addEventListener("submit", loadImageArchive);
$("img-tag-form").addEventListener("submit", submitTagImage);
$("reg-login-form").addEventListener("submit", submitRegistryLogin);
$("reg-push-form").addEventListener("submit", submitPushImage);
$("commit-btn").addEventListener("click", commitContainer);
$("compose-up-btn").addEventListener("click", submitComposeUp);
$("compose-down-btn").addEventListener("click", submitComposeDown);

// Pods view events
$("refresh-pods").addEventListener("click", () => loadPods(false));
$("pods-table").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-pod-act]");
  if (btn) podAction(btn.dataset.podAct, btn.dataset.id);
});

// Volumes & System Prune view events
$("refresh-volumes").addEventListener("click", () => loadVolumes(false));
$("prune-btn").addEventListener("click", openPruneModal);
$("prune-submit").addEventListener("click", submitSystemPrune);
$("vol-create-form").addEventListener("submit", createVolumeSubmit);
$("volumes-table").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-vol-remove]");
  if (btn) removeVolume(btn.dataset.volRemove);
});

// Next-Gen Wasm & KVM Sandbox view events
$("kvm-probe-btn").addEventListener("click", probeKvmHardware);
$("wasm-run-form").addEventListener("submit", submitWasmPlayground);
$("sandbox-form").addEventListener("submit", submitAiSandbox);
$("vm-setup-kernel-btn").addEventListener("click", setupMicroVmKernel);
$("vm-boot-form").addEventListener("submit", submitVmBoot);
$("sec-sign-btn").addEventListener("click", () => runImageSecurityAction("sign"));
$("sec-verify-btn").addEventListener("click", () => runImageSecurityAction("verify"));
$("sec-sbom-btn").addEventListener("click", () => runImageSecurityAction("sbom"));
$("wasm-serve-target").addEventListener("input", updateWasmServePreview);
$("wasm-serve-addr").addEventListener("input", updateWasmServePreview);
$("wasm-serve-copy-btn").addEventListener("click", copyWasmServeCmd);

// Modals & drawer
document.querySelectorAll("[data-close]").forEach((btn) =>
  btn.addEventListener("click", () => $(btn.dataset.close).classList.add("hidden")),
);
$("run-modal").addEventListener("click", (e) => {
  if (e.target === $("run-modal")) $("run-modal").classList.add("hidden");
});
$("image-modal").addEventListener("click", (e) => {
  if (e.target === $("image-modal")) $("image-modal").classList.add("hidden");
});
$("compose-modal").addEventListener("click", (e) => {
  if (e.target === $("compose-modal")) $("compose-modal").classList.add("hidden");
});
$("prune-modal").addEventListener("click", (e) => {
  if (e.target === $("prune-modal")) $("prune-modal").classList.add("hidden");
});
$("run-submit").addEventListener("click", submitRun);
$("detail-close").addEventListener("click", closeDetail);
$("drawer-backdrop").addEventListener("click", closeDetail);

// Tabbed detail drawer, Live Logs controls & Built-in Web Terminal
initDetailTabs();
$("log-autoscroll").addEventListener("change", (e) => {
  state.logAutoScroll = Boolean(e.target.checked);
});
$("log-refresh").addEventListener("click", () => refreshDetail(false));
$("pty-connect-btn").addEventListener("click", toggleLivePty);
$("pty-ctrl-c").addEventListener("click", sendPtyCtrlC);
$("term-output").addEventListener("keydown", handleRawPtyScreenKeydown);
$("term-run").addEventListener("click", () => runTerminalCommand());
$("term-clear").addEventListener("click", clearTerminal);
$("term-input").addEventListener("keydown", handleTerminalKeydown);
document.querySelectorAll("[data-term-quick]").forEach((btn) =>
  btn.addEventListener("click", () => runTerminalCommand(btn.dataset.termQuick)),
);

// Settings
$("save-settings").addEventListener("click", () => saveSettings(false));
$("save-connect").addEventListener("click", () => saveSettings(true));
$("set-mode").addEventListener("change", () => {
  $("mode-hint").textContent = MODE_HINTS[$("set-mode").value] || "";
});

// File / folder dialogs
$("img-pick").addEventListener("click", async () => {
  const dir = await dialog.open({ directory: true });
  if (dir) $("img-rootfs").value = dir;
});
document.querySelectorAll(".pick-dir").forEach((b) =>
  b.addEventListener("click", async () => {
    const dir = await dialog.open({ directory: true });
    if (dir) $(b.dataset.target).value = dir;
  }),
);
document.querySelectorAll(".pick").forEach((b) =>
  b.addEventListener("click", async () => {
    const file = await dialog.open({ multiple: false });
    if (file) $(b.dataset.target).value = file;
  }),
);

// Global shortcuts & menu dismiss
document.addEventListener("click", (e) => {
  if (!e.target.closest("#row-menu") && !e.target.closest("button[data-menu-id]")) {
    closeRowMenu();
  }
});

document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    $("search").focus();
    $("search").select();
  } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "b") {
    e.preventDefault();
    toggleRail();
  } else if (e.key === "Escape") {
    if (!$("row-menu").classList.contains("hidden")) closeRowMenu();
    else if (!$("run-modal").classList.contains("hidden")) $("run-modal").classList.add("hidden");
    else if (!$("image-modal").classList.contains("hidden")) $("image-modal").classList.add("hidden");
    else if (!$("compose-modal").classList.contains("hidden")) $("compose-modal").classList.add("hidden");
    else if (!$("prune-modal").classList.contains("hidden")) $("prune-modal").classList.add("hidden");
    else if (state.detailId) closeDetail();
    else if (document.activeElement === $("search")) {
      $("search").value = "";
      state.search = "";
      $("search").blur();
      switchView(state.view);
    }
  }
});

window
  .matchMedia("(prefers-color-scheme: light)")
  .addEventListener("change", () => applyTheme(localStorage.getItem("coral.theme") || "auto"));

// ---------- init ----------

(async function init() {
  initIcons();
  applyTheme(localStorage.getItem("coral.theme") || "auto");
  if (localStorage.getItem("coral.rail") === "1") document.body.classList.add("rail");

  renderContainerToolbar();
  renderEmptyViews();
  updateChrome();

  await guard(async () => {
    const info = await invoke("connection_status");
    if (info.connected) {
      setConnected(true, info);
      await refreshAll(true);
    }
  });
})();

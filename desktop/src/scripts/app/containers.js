// Containers view event bindings (delegated table, toolbar, bulk and row menu).
// ---------- Containers View Events (Delegated) ----------

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

$("containers-table").addEventListener("click", async (e) => {
  const welcomeAct = e.target.closest("[data-welcome-act]");
  if (welcomeAct) {
    const act = welcomeAct.dataset.welcomeAct;
    if (act === "connect") connect();
    else if (act === "settings") switchView("settings");
    else if (act === "browse-compose") browseComposeFile(true);
    else if (act === "custom-run") openRunModal();
    else if (act === "browse-images") {
      switchView("images");
      selectImagesTab("discover");
    }
    return;
  }

  const starter = e.target.closest("[data-starter-id]");
  if (starter) {
    launchQuickStarter(starter.dataset.starterId);
    return;
  }

  const portBtn = e.target.closest("[data-open-port]");
  if (portBtn) {
    e.stopPropagation();
    openMappedPort(portBtn.dataset.openPort);
    return;
  }

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

  const ckStack = e.target.closest("[data-ck-stack]");
  if (ckStack) {
    e.stopPropagation();
    const proj = ckStack.dataset.ckStack;
    const members = state.containers.filter((c) => c.labels?.["coral.compose.project"] === proj);
    const allSel = members.every((m) => state.selectedIds.has(m.id));
    if (allSel) members.forEach((m) => state.selectedIds.delete(m.id));
    else members.forEach((m) => state.selectedIds.add(m.id));
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

  const stackAct = e.target.closest("[data-stack-act]");
  if (stackAct) {
    e.stopPropagation();
    stackAction(stackAct.dataset.stackAct, stackAct.dataset.stack);
    return;
  }

  const stackToggle = e.target.closest("[data-stack-toggle]");
  if (stackToggle && !e.target.closest(".actions") && !e.target.closest(".ck")) {
    const proj = stackToggle.dataset.stackToggle;
    if (state.collapsedStacks.has(proj)) state.collapsedStacks.delete(proj);
    else state.collapsedStacks.add(proj);
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

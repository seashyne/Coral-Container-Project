// Containers view: welcome hub, toolbar, table rendering and filtering.
function renderContainersEmpty() {
  if (!state.connected) {
    const endpoint = state.settings?.endpoint || "127.0.0.1:50051";
    const mode = state.settings?.mode || "local";
    $("containers-table").innerHTML = emptyStateHtml(
      "cube",
      t("containers.disconnectedTitle"),
      t("containers.disconnectedDetail", { mode, endpoint }),
      `<button type="button" class="btn primary" data-welcome-act="connect">${esc(t("containers.connectEngine"))}</button>
       <button type="button" class="btn" data-welcome-act="settings">${esc(t("containers.connectionSettings"))}</button>`,
    );
    return;
  }

  const cards = QUICK_STARTERS.map(
    (s) => `
      <button type="button" class="starter-card" data-starter-id="${esc(s.id)}">
        <span class="tile ${esc(s.tileClass)}">${esc(s.monogram)}</span>
        <span class="starter-meta">
          <strong>${esc(t(s.titleKey))}</strong>
          <small>${esc(t(s.subtitleKey))}</small>
        </span>
        <span class="starter-go">${ICONS.play}<span>${esc(t("common.run"))}</span></span>
      </button>`,
  ).join("");

  $("containers-table").innerHTML = `
    <div class="welcome-hub">
      <div class="welcome-head">
        <div class="welcome-icon">${ICONS.cube}</div>
        <div>
          <h3>${esc(t("containers.welcomeTitle"))}</h3>
          <p class="muted">${esc(t("containers.welcomeSub"))}</p>
        </div>
      </div>

      <div class="starter-grid">${cards}</div>

      <div class="welcome-actions">
        <button type="button" class="btn primary" data-welcome-act="browse-compose">${ICONS.folder}<span>${esc(t("containers.openCompose"))}</span></button>
        <button type="button" class="btn" data-welcome-act="custom-run">${ICONS.plus}<span>${esc(t("containers.customContainer"))}</span></button>
        <button type="button" class="btn" data-welcome-act="browse-images">${ICONS.image}<span>${esc(t("containers.browseImages"))}</span></button>
      </div>
    </div>`;
}

function renderContainerToolbar() {
  const total = state.containers.length;
  const running = state.containers.filter((c) => c.state === "running").length;
  const exited = state.containers.filter((c) => c.state !== "running").length;

  const tabs = [
    { key: "all", label: t("containers.filterAll"), count: total, dot: "" },
    { key: "running", label: t("containers.filterRunning"), count: running, dot: "moss" },
    { key: "exited", label: t("containers.filterStopped"), count: exited, dot: "" },
  ];
  $("ctabs").innerHTML = tabs
    .map(
      (tab) => `
      <button type="button" class="ltab" data-filter="${tab.key}" aria-pressed="${state.filter === tab.key}">
        ${tab.dot ? `<i class="${tab.dot}"></i>` : ""}
        <span>${esc(tab.label)}</span>
        <span class="n">${tab.count}</span>
      </button>`,
    )
    .join("");

  const selCount = state.selectedIds.size;
  const bulk = $("cbulk");
  if (selCount > 0) {
    bulk.classList.remove("hidden");
    bulk.innerHTML = `
      <span class="bcount">
        <b>${selCount}</b> ${esc(t("common.selected"))}
        <button type="button" class="x" data-bulk="clear" title="${esc(t("common.clearSelection"))}" aria-label="${esc(t("common.clearSelection"))}">${ICONS.x}</button>
      </span>
      <button type="button" class="btn sm" data-bulk="start">${esc(t("common.start"))}</button>
      <button type="button" class="btn sm" data-bulk="stop">${esc(t("common.stop"))}</button>
      <button type="button" class="btn sm danger" data-bulk="remove">${esc(t("common.remove"))}</button>`;
  } else {
    bulk.classList.add("hidden");
    bulk.innerHTML = "";
  }

  $("cright").innerHTML = `
    <button id="compose-btn" type="button" class="btn">${ICONS.layers}<span class="hlbl">${esc(t("containers.btnCompose"))}</span></button>
    <button id="run-btn" type="button" class="btn primary">${ICONS.plus}<span class="hlbl">${esc(t("containers.btnRun"))}</span></button>
    <button id="refresh-containers" type="button" class="btn ic" title="${esc(t("common.refresh"))}" aria-label="${esc(t("common.refresh"))}">${ICONS.refresh}</button>`;
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
    if (state.filter === "exited" && c.state === "running") return false;
    if (!q) return true;
    const stack = (c.labels?.["coral.compose.project"] || "").toLowerCase();
    return (
      c.name.toLowerCase().includes(q) ||
      c.id.toLowerCase().includes(q) ||
      c.image.toLowerCase().includes(q) ||
      c.state.toLowerCase().includes(q) ||
      stack.includes(q)
    );
  });
}

function containerRowHtml(c, treeInfo = null) {
  const chosen = state.selectedIds.has(c.id);
  const pillClass = c.state === "running" ? "running" : c.state === "unknown" ? "failed" : "";
  const mono = (c.name || c.id).slice(0, 2).toUpperCase();
  const health = c.health || c.labels?.["coral.health.status"] || "";
  const healthHtml =
    health && health !== "none"
      ? `<span class="health-badge ${esc(health)}">${esc(tHealth(health))}</span>`
      : "";

  const ports = parseMappedPorts(c.labels?.["coral.ports"]);
  const portsHtml = ports.length
    ? ports
        .map((p) =>
          p.hostPort && c.state === "running"
            ? `<button type="button" class="port-chip" data-open-port="${esc(p.hostPort)}" title="${esc(t("containers.openBrowserTip", { port: p.hostPort }))}">${esc(p.label)} ${ICONS.external}</button>`
            : `<span class="port-chip static">${esc(p.label)}</span>`,
        )
        .join(" ")
    : '<span class="muted">—</span>';

  const treePrefix = treeInfo
    ? `<span class="tree ${treeInfo.isLast ? "last" : ""}" aria-hidden="true"></span>`
    : "";

  return `
    <tr class="row ${treeInfo ? "member" : ""} ${chosen ? "chosen" : ""}" data-cid="${esc(c.id)}">
      <td class="ck">
        <div class="ckbox" data-ck="${esc(c.id)}" role="checkbox" aria-checked="${chosen}" tabindex="0">
          <span class="cb">${ICONS.check}</span>
        </div>
      </td>
      <td data-label="${esc(t("containers.colName"))}">
        <div class="name">
          ${treePrefix}
          <span class="tile cmp">${esc(mono)}</span>
          <div>
            <button type="button" class="nm-btn" data-act="logs" data-id="${esc(c.id)}" title="${esc(c.id)}">${esc(c.name)}</button>
          </div>
        </div>
      </td>
      <td class="mono" data-label="${esc(t("containers.colImage"))}">${esc(c.image)}</td>
      <td data-label="${esc(t("containers.colState"))}"><span class="pill ${pillClass}">${esc(tState(c.state))}</span>${healthHtml}</td>
      <td data-label="${esc(t("containers.colPorts"))}">${portsHtml}</td>
      <td class="muted" data-label="${esc(t("containers.colCreated"))}">${esc(fmtTime(c.createdAt))}</td>
      <td class="actions">
        <div class="acts-row">
          ${
            c.state === "running"
              ? `<button type="button" class="btn ic" data-act="term" data-id="${esc(c.id)}" title="${esc(t("containers.openTerminal"))}" aria-label="${esc(t("containers.openTerminal"))}">${ICONS.terminal}</button>
                 <button type="button" class="btn ic halt" data-act="stop" data-id="${esc(c.id)}" title="${esc(t("containers.stopContainer"))}" aria-label="${esc(t("containers.stopContainer"))}">${ICONS.square}</button>`
              : `<button type="button" class="btn ic go" data-act="start" data-id="${esc(c.id)}" title="${esc(t("containers.startContainer"))}" aria-label="${esc(t("containers.startContainer"))}">${ICONS.play}</button>
                 <button type="button" class="btn ic" data-act="logs" data-id="${esc(c.id)}" title="${esc(t("containers.viewLogs"))}" aria-label="${esc(t("containers.viewLogs"))}">${ICONS.logs}</button>`
          }
          <button type="button" class="btn ic danger" data-act="remove" data-id="${esc(c.id)}" title="${esc(t("containers.removeContainer"))}" aria-label="${esc(t("containers.removeContainer"))}">${ICONS.trash}</button>
          <button type="button" class="btn ic" data-menu-id="${esc(c.id)}" title="${esc(t("containers.moreActions"))}" aria-label="${esc(t("containers.moreActions"))}">${ICONS.more}</button>
        </div>
      </td>
    </tr>`;
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
        t("containers.noMatchingTitle"),
        t("containers.noMatchingSub"),
      );
    }
    return;
  }

  const allSelected = rows.length > 0 && rows.every((r) => state.selectedIds.has(r.id));
  const someSelected = !allSelected && rows.some((r) => state.selectedIds.has(r.id));
  const headCbClass = allSelected ? "cb all" : someSelected ? "cb some" : "cb";

  const stacks = new Map();
  const standalone = [];
  for (const c of rows) {
    const proj = c.labels?.["coral.compose.project"];
    if (proj) {
      if (!stacks.has(proj)) stacks.set(proj, []);
      stacks.get(proj).push(c);
    } else {
      standalone.push(c);
    }
  }

  let bodyHtml = "";
  for (const [proj, members] of stacks.entries()) {
    const collapsed = state.collapsedStacks.has(proj);
    const runningN = members.filter((m) => m.state === "running").length;
    const allStackSelected = members.every((m) => state.selectedIds.has(m.id));
    bodyHtml += `
      <tr class="group ${collapsed ? "collapsed" : ""}" data-stack-toggle="${esc(proj)}">
        <td class="ck">
          <div class="ckbox" data-ck-stack="${esc(proj)}" role="checkbox" aria-checked="${allStackSelected}" tabindex="0">
            <span class="cb ${allStackSelected ? "all" : ""}">${ICONS.check}</span>
          </div>
        </td>
        <td colspan="2">
          <div class="gname">
            <span class="chev">${ICONS.chevron}</span>
            <span class="tile cmp">${ICONS.layers}</span>
            <div>
              <b>${esc(proj)}</b>
              <div class="gsub">${esc(t("containers.stackServices", { count: members.length }))}</div>
            </div>
          </div>
        </td>
        <td><span class="pill ${runningN > 0 ? "running" : ""}">${esc(t("containers.stackRunning", { running: runningN, total: members.length }))}</span></td>
        <td class="muted">—</td>
        <td class="muted">—</td>
        <td class="actions">
          <div class="acts-row">
            ${
              runningN > 0
                ? `<button type="button" class="btn ic halt" data-stack-act="stop" data-stack="${esc(proj)}" title="${esc(t("containers.stopStack"))}" aria-label="${esc(t("containers.stopStack"))}">${ICONS.square}</button>`
                : ""
            }
            <button type="button" class="btn ic danger" data-stack-act="down" data-stack="${esc(proj)}" title="${esc(t("containers.removeStack"))}" aria-label="${esc(t("containers.removeStack"))}">${ICONS.trash}</button>
          </div>
        </td>
      </tr>`;
    if (!collapsed) {
      members.forEach((m, idx) => {
        bodyHtml += containerRowHtml(m, { isLast: idx === members.length - 1 });
      });
    }
  }

  for (const c of standalone) {
    bodyHtml += containerRowHtml(c, null);
  }

  $("containers-table").innerHTML = `
    <div class="responsive-table-wrap" tabindex="0" role="region" aria-labelledby="containers-table-caption">
      <table>
        <caption id="containers-table-caption" class="visually-hidden">${esc(t("containers.title"))}</caption>
        <thead>
          <tr>
            <th scope="col" class="ck">
              <div class="ckbox" data-ck-all="1" role="checkbox" aria-checked="${allSelected}" tabindex="0">
                <span class="${headCbClass}">${ICONS.check}</span>
              </div>
            </th>
            <th scope="col">${esc(t("containers.colName"))}</th>
            <th scope="col">${esc(t("containers.colImage"))}</th>
            <th scope="col">${esc(t("containers.colState"))}</th>
            <th scope="col">${esc(t("containers.colPorts"))}</th>
            <th scope="col">${esc(t("containers.colCreated"))}</th>
            <th scope="col" class="r">${esc(t("containers.colActions"))}</th>
          </tr>
        </thead>
        <tbody>${bodyHtml}</tbody>
      </table>
    </div>`;
}


// Pod sandboxes view: listing, filtering, stopping, and removing CRI pods.

function renderPodsEmpty() {
  $("pods-table").innerHTML = emptyStateHtml(
    "hexagon",
    state.connected ? t("pods.emptyTitle") : t("images.disconnectedTitle"),
    state.connected ? t("pods.emptySub") : t("pods.disconnectedSub"),
  );
}

async function loadPods(quiet) {
  try {
    const rows = await invoke("list_pods");
    state.pods = rows;
    const json = JSON.stringify(rows);
    if (json !== state.lastPods || state.view === "pods") {
      state.lastPods = json;
      renderPodsTable();
    }
  } catch (e) {
    if (!quiet) toast(String(e), true);
  }
}

function renderPodsTable() {
  if (!state.connected) {
    renderPodsEmpty();
    return;
  }
  const q = state.search.trim().toLowerCase();
  const rows = state.pods.filter((p) => {
    if (!q) return true;
    return (
      p.name.toLowerCase().includes(q) ||
      p.id.toLowerCase().includes(q) ||
      p.state.toLowerCase().includes(q)
    );
  });
  if (!rows.length) {
    if (!state.pods.length) renderPodsEmpty();
    else {
      $("pods-table").innerHTML = emptyStateHtml(
        "search",
        t("pods.noMatchTitle"),
        t("pods.noMatchSub"),
      );
    }
    return;
  }
  const body = rows
    .map((p) => `
      <tr class="row">
        <td data-label="${esc(t("pods.colPod"))}">
          <div class="name">
            <span class="tile cmp">${ICONS.hexagon}</span>
            <div>
              <div class="nm" title="${esc(p.id)}">${esc(p.name)}</div>
            </div>
          </div>
        </td>
        <td data-label="${esc(t("pods.colState"))}"><span class="pill ${p.state === "ready" ? "ready" : ""}">${esc(tState(p.state))}</span></td>
        <td class="muted" data-label="${esc(t("pods.colCreated"))}">${esc(fmtTime(p.createdAt))}</td>
        <td class="actions">
          <div class="acts-row">
            ${
              p.state === "ready"
                ? `<button type="button" class="btn ic halt" data-pod-act="stop" data-id="${esc(p.id)}" title="${esc(t("pods.stopTitle"))}" aria-label="${esc(t("pods.stopTitle"))}">${ICONS.square}</button>`
                : ""
            }
            <button type="button" class="btn ic danger" data-pod-act="remove" data-id="${esc(p.id)}" title="${esc(t("pods.removeTitle"))}" aria-label="${esc(t("pods.removeTitle"))}">${ICONS.trash}</button>
          </div>
        </td>
      </tr>`)
    .join("");
  $("pods-table").innerHTML = `
    <div class="responsive-table-wrap" tabindex="0" role="region" aria-label="${esc(t("pods.title"))}">
      <table>
        <thead>
          <tr>
            <th scope="col">${esc(t("pods.colPod"))}</th>
            <th scope="col">${esc(t("pods.colState"))}</th>
            <th scope="col">${esc(t("pods.colCreated"))}</th>
            <th scope="col" class="r">${esc(t("pods.colActions"))}</th>
          </tr>
        </thead>
        <tbody>${body}</tbody>
      </table>
    </div>`;
}

async function podAction(act, id) {
  await guard(async () => {
    await invoke(act === "stop" ? "stop_pod" : "remove_pod", { id });
    toast(t(act === "stop" ? "toast.podStopped" : "toast.podRemoved"));
    await loadPods(false);
    updateChrome();
  });
}

// Pod sandboxes view: listing, filtering, stopping, and removing CRI pods.

function renderPodsEmpty() {
  $("pods-table").innerHTML = emptyStateHtml(
    "hexagon",
    state.connected ? "No pod sandboxes" : "Daemon disconnected",
    state.connected
      ? "Pod sandboxes appear automatically when containers run."
      : "Connect to coral-daemon to inspect pod sandboxes.",
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
    else $("pods-table").innerHTML = emptyStateHtml("search", "No matching pods", "Try adjusting your search query.");
    return;
  }
  const body = rows
    .map((p) => `
      <tr class="row">
        <td>
          <div class="name">
            <span class="tile">${ICONS.hexagon}</span>
            <div>
              <div class="nm">${esc(p.name)}</div>
              <div class="sub-name mono" title="${esc(p.id)}">${esc(shortId(p.id))}</div>
            </div>
          </div>
        </td>
        <td><span class="pill ${p.state === "ready" ? "ready" : ""}">${esc(p.state)}</span></td>
        <td class="muted">${esc(fmtTime(p.createdAt))}</td>
        <td class="actions">
          <div class="acts-row">
            ${
              p.state === "ready"
                ? `<button type="button" class="btn ic halt" data-pod-act="stop" data-id="${esc(p.id)}" title="Stop pod" aria-label="Stop pod">${ICONS.square}</button>`
                : ""
            }
            <button type="button" class="btn ic danger" data-pod-act="remove" data-id="${esc(p.id)}" title="Remove pod" aria-label="Remove pod">${ICONS.trash}</button>
          </div>
        </td>
      </tr>`)
    .join("");
  $("pods-table").innerHTML = `
    <table>
      <thead><tr><th>Pod sandbox</th><th>State</th><th>Created</th><th class="r">Actions</th></tr></thead>
      <tbody>${body}</tbody>
    </table>`;
}

async function podAction(act, id) {
  await guard(async () => {
    await invoke(act === "stop" ? "stop_pod" : "remove_pod", { id });
    toast(act === "stop" ? "Pod stopped" : "Pod removed");
    await loadPods(false);
    updateChrome();
  });
}

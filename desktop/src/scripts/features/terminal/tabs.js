// Terminal panel: detail tabs, ANSI palette and PTY event listener setup.
const DETAIL_TABS = [
  { key: "logs", tabId: "dtab-logs", panelId: "dpane-logs" },
  { key: "term", tabId: "dtab-term", panelId: "dpane-term" },
  { key: "metrics", tabId: "dtab-metrics", panelId: "dpane-metrics" },
];

const ANSI_COLORS = {
  30: "#52525b", 31: "#f87171", 32: "#4ade80", 33: "#facc15",
  34: "#60a5fa", 35: "#c084fc", 36: "#22d3ee", 37: "#e4e4e7",
  90: "#71717a", 91: "#fca5a5", 92: "#86efac", 93: "#fde047",
  94: "#93c5fd", 95: "#d8b4fe", 96: "#67e8f9", 97: "#ffffff",
};

let ptyListenersReady = false;

function switchDetailTab(tabKey, focusTab = false) {
  state.detailTab = tabKey;
  for (const item of DETAIL_TABS) {
    const tab = $(item.tabId);
    const panel = $(item.panelId);
    if (!tab || !panel) continue;
    const active = item.key === tabKey;
    tab.setAttribute("aria-selected", active ? "true" : "false");
    tab.toggleAttribute("focusgroupstart", active);
    tab.tabIndex = active ? 0 : -1;
    if (active) {
      panel.removeAttribute("hidden");
      if (focusTab) tab.focus();
      tab.scrollIntoView({ block: "nearest", inline: "nearest" });
    } else {
      panel.setAttribute("hidden", "until-found");
    }
  }
  if (tabKey === "term" && state.detailId) {
    renderTerminal(state.detailId);
    const row = state.containers.find((c) => c.id === state.detailId);
    const sess = getTermSession(state.detailId);
    if (row?.state === "running" && !sess.ptySessionId && !sess.ptyConnecting) {
      connectLivePty(state.detailId);
    } else if (!focusTab && $("term-input")) {
      $("term-input").focus();
    }
  }
}

function initDetailTabs() {
  const tablist = $("detail-tablist");
  if (!tablist) return;

  tablist.addEventListener("click", (e) => {
    const btn = e.target.closest('[role="tab"]');
    if (!btn || !btn.dataset.dtab) return;
    switchDetailTab(btn.dataset.dtab, false);
  });

  tablist.addEventListener("keydown", (e) => {
    const tabs = Array.from(tablist.querySelectorAll('[role="tab"]'));
    const idx = tabs.indexOf(document.activeElement);
    if (idx < 0) return;
    let next = -1;
    if (e.key === "ArrowRight") next = (idx + 1) % tabs.length;
    else if (e.key === "ArrowLeft") next = (idx - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = tabs.length - 1;
    if (next >= 0) {
      e.preventDefault();
      switchDetailTab(tabs[next].dataset.dtab, true);
    }
  });

  for (const item of DETAIL_TABS) {
    const panel = $(item.panelId);
    if (panel) {
      panel.addEventListener("beforematch", () => switchDetailTab(item.key, false));
    }
  }
}

function ensurePtyEventListeners() {
  if (ptyListenersReady) return;
  const ev = window.__TAURI__?.event;
  if (!ev || typeof ev.listen !== "function") return;
  ptyListenersReady = true;

  ev.listen("pty-output", (event) => {
    const { sessionId, data } = event.payload || {};
    const entry = Object.entries(state.termSessions).find(([, s]) => s.ptySessionId === sessionId);
    if (!entry) return;
    const [cid, sess] = entry;
    appendPtyChunk(sess, data || "");
    if (state.detailId === cid) renderTerminal(cid);
  });

  ev.listen("pty-exit", (event) => {
    const { sessionId, exitCode } = event.payload || {};
    const entry = Object.entries(state.termSessions).find(([, s]) => s.ptySessionId === sessionId);
    if (!entry) return;
    const [cid, sess] = entry;
    sess.ptySessionId = null;
    sess.ptyConnecting = false;
    sess.output += `\n[PTY session closed (exit ${exitCode ?? 0})]\n`;
    if (state.detailId === cid) renderTerminal(cid);
  });
}


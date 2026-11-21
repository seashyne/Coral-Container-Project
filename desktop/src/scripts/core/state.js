// Tauri IPC bridge, shared application state, accessible dialogs/tabs utilities, and formatting.

const invoke =
  window.__TAURI__?.core?.invoke ||
  (async () => {
    throw new Error("Coral Desktop requires the Tauri runtime");
  });
const dialog = window.__TAURI__?.dialog || {
  open: async () => null,
  save: async () => null,
};

const $ = (id) => document.getElementById(id);

const state = {
  connected: false,
  connInfo: null,
  view: "containers",
  imagesTab: "local",
  nextgenTab: "sandbox",
  settingsTab: "general",
  imageModalTab: "pull",
  settings: null,
  autoConnect: localStorage.getItem("coral.autoConnect") !== "0",
  containers: [],
  pods: [],
  images: [],
  volumes: [],
  diskUsage: null,
  filter: "all",
  search: "",
  selectedIds: new Set(),
  collapsedStacks: new Set(),
  pulling: new Set(),
  detailId: null,
  detailTab: "logs",
  logAutoScroll: true,
  baseLog: "",
  execLog: "",
  termSessions: {},
  metricsHistory: {},
  lastContainers: "",
  lastPods: "",
  lastImages: "",
  lastVolumes: "",
  pollTimer: null,
  menuTarget: null,
  runSeq: 1,
};

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));
}

function toast(msg, isError = false) {
  const el = document.createElement("div");
  el.className = "toast" + (isError ? " error" : "");
  el.setAttribute("role", isError ? "alert" : "status");
  el.textContent = msg;
  $("toasts").appendChild(el);
  setTimeout(() => el.remove(), isError ? 7000 : 3800);
}

function fmtTime(nanos) {
  if (!nanos) return "—";
  const locale = typeof getLang === "function" && getLang() === "th" ? "th-TH" : undefined;
  return new Date(nanos / 1e6).toLocaleString(locale);
}

function fmtBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let val = n;
  let i = 0;
  while (val >= 1024 && i < units.length - 1) {
    val /= 1024;
    i += 1;
  }
  return `${val >= 100 || i === 0 ? val.toFixed(0) : val.toFixed(1)} ${units[i]}`;
}

function shortId(id) {
  return id.length > 18 ? id.slice(0, 18) + "…" : id;
}

function emptyStateHtml(iconName, title, sub, actionsHtml = "") {
  return `
    <div class="empty">
      <div>
        <div class="art">${ICONS[iconName] || ICONS.cube}</div>
        <h2>${esc(title)}</h2>
        <p>${esc(sub)}</p>
        ${actionsHtml ? `<div class="empty-actions">${actionsHtml}</div>` : ""}
      </div>
    </div>`;
}

async function guard(fn) {
  try {
    await fn();
  } catch (e) {
    toast(String(e), true);
    if (String(e).includes("not connected")) setConnected(false);
  }
}

// ---------- Native <dialog> helpers + closedby="any" fallback ----------

function openDialog(id) {
  const dlg = $(id);
  if (!dlg) return;
  if (!dlg.open) {
    dlg.returnValue = "";
    dlg.showModal();
  }
}

function closeDialog(id) {
  const dlg = $(id);
  if (dlg && dlg.open) dlg.close();
}

function initDialogs() {
  for (const dlg of document.querySelectorAll("dialog")) {
    dlg.addEventListener("close", () => {
      dlg.returnValue = "";
    });
  }
  // Light-dismiss fallback where HTMLDialogElement.prototype.closedBy is unavailable (e.g. WebKitGTK).
  if (!("closedBy" in HTMLDialogElement.prototype)) {
    const outside = (e, dlg) => {
      const r = dlg.getBoundingClientRect();
      return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
    };
    for (const dlg of document.querySelectorAll('dialog[closedby="any"]')) {
      let pressedOutside = false;
      dlg.addEventListener("pointerdown", (e) => {
        pressedOutside = outside(e, dlg);
      });
      dlg.addEventListener("pointerup", (e) => {
        if (pressedOutside && outside(e, dlg)) dlg.close();
        pressedOutside = false;
      });
    }
  }
}

// ---------- Shared Accessible Tablist Controller ----------

function setupTabGroup(tablistId, onSelect = null) {
  const tablist = $(tablistId);
  if (!tablist) return () => {};
  const tabs = Array.from(tablist.querySelectorAll('[role="tab"]'));

  function selectKey(key, focusTab = false) {
    const selected = tabs.find((t) => t.dataset.tab === key) || tabs[0];
    if (!selected) return;
    for (const tab of tabs) {
      const active = tab === selected;
      const panel = tab.dataset.panel ? $(tab.dataset.panel) : null;
      tab.setAttribute("aria-selected", active ? "true" : "false");
      tab.tabIndex = active ? 0 : -1;
      if (panel) {
        if (active) {
          panel.removeAttribute("hidden");
        } else {
          panel.setAttribute("hidden", "until-found");
        }
      }
    }
    if (focusTab) selected.focus();
    selected.scrollIntoView({ block: "nearest", inline: "nearest" });
    if (onSelect) onSelect(selected.dataset.tab);
  }

  tablist.addEventListener("click", (e) => {
    const btn = e.target.closest('[role="tab"]');
    if (btn && btn.dataset.tab) selectKey(btn.dataset.tab, false);
  });

  tablist.addEventListener("keydown", (e) => {
    const idx = tabs.indexOf(document.activeElement);
    if (idx < 0) return;
    let next = -1;
    if (e.key === "ArrowRight") next = (idx + 1) % tabs.length;
    else if (e.key === "ArrowLeft") next = (idx - 1 + tabs.length) % tabs.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = tabs.length - 1;
    if (next >= 0) {
      e.preventDefault();
      selectKey(tabs[next].dataset.tab, true);
    }
  });

  for (const tab of tabs) {
    const panel = tab.dataset.panel ? $(tab.dataset.panel) : null;
    if (panel) {
      panel.addEventListener("beforematch", () => selectKey(tab.dataset.tab, false));
    }
  }

  return selectKey;
}

// ---------- Sidebar Rail ----------

function toggleRail() {
  document.body.classList.toggle("rail");
  const isRail = document.body.classList.contains("rail");
  localStorage.setItem("coral.rail", isRail ? "1" : "0");
  if ($("set-compact-rail")) $("set-compact-rail").checked = isRail;
}

// Tauri IPC bridge, global application state, formatting helpers, toast notifications, and theme/rail toggles.

const invoke = window.__TAURI__.core.invoke;
const dialog = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);

const state = {
  connected: false,
  connInfo: null,
  view: "containers",
  settings: null,
  containers: [],
  pods: [],
  images: [],
  volumes: [],
  diskUsage: null,
  filter: "all",
  search: "",
  selectedIds: new Set(),
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
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

function toast(msg, isError = false) {
  const el = document.createElement("div");
  el.className = "toast" + (isError ? " error" : "");
  el.textContent = msg;
  $("toasts").appendChild(el);
  setTimeout(() => el.remove(), isError ? 7000 : 3500);
}

function fmtTime(nanos) {
  if (!nanos) return "—";
  return new Date(nanos / 1e6).toLocaleString();
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
  return id.length > 22 ? id.slice(0, 22) + "…" : id;
}

async function guard(fn) {
  try {
    await fn();
  } catch (e) {
    toast(String(e), true);
    if (String(e).includes("not connected")) setConnected(false);
  }
}

function applyTheme(theme) {
  const value = theme === "dark" || theme === "light" ? theme : "auto";
  document.documentElement.dataset.theme = value;
  localStorage.setItem("coral.theme", value);
  if ($("set-theme")) $("set-theme").value = value;
  const effectiveLight =
    value === "light" ||
    (value === "auto" && window.matchMedia("(prefers-color-scheme: light)").matches);
  $("theme-btn").innerHTML = effectiveLight ? ICONS.moon : ICONS.sun;
  $("theme-btn").title = `Theme: ${value}`;
}

function cycleTheme() {
  const cur = localStorage.getItem("coral.theme") || "auto";
  const next = cur === "auto" ? "dark" : cur === "dark" ? "light" : "auto";
  applyTheme(next);
}

function toggleRail() {
  document.body.classList.toggle("rail");
  localStorage.setItem("coral.rail", document.body.classList.contains("rail") ? "1" : "0");
}

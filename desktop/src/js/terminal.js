// Tabbed Container Detail Drawer (accessible tabs + beforematch sync),
// True Full-Duplex Interactive PTY Streaming Terminal (CRI ExecStream + ANSI colors + raw keys),
// Stateful ExecSync Fallback, and Live Resource Metrics with real-time SVG Sparklines.

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

// ---------- True Full-Duplex PTY Streaming + ExecSync Fallback ----------

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

function getTermSession(id) {
  if (!state.termSessions[id]) {
    state.termSessions[id] = {
      cwd: "/",
      history: [],
      histIdx: -1,
      output: "Coral Terminal — Live PTY streaming for running containers, stateful Exec fallback for exited containers.\n",
      busy: false,
      ptySessionId: null,
      ptyConnecting: false,
    };
  }
  return state.termSessions[id];
}

function appendPtyChunk(sess, chunk) {
  // Handle clear-screen ESC[2J / ESC[H cleanly and normalize CRLF.
  if (chunk.includes("\x1b[2J") || chunk.includes("\x1b[H\x1b[J")) {
    const parts = chunk.split(/\x1b\[2J|\x1b\[H\x1b\[J/);
    sess.output = parts[parts.length - 1];
  } else {
    sess.output += chunk.replace(/\r\n/g, "\n");
  }
  // Keep terminal scrollback bounded (~64 KB) for snappy WebView rendering.
  if (sess.output.length > 65536) {
    sess.output = sess.output.slice(-65536);
  }
}

function renderAnsiToHtml(raw) {
  // Strip non-SGR CSI/OSC control sequences while turning SGR color codes (`\x1b[...m`) into spans.
  const cleaned = raw
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-9;?]*[A-LN-Za-z]/g, "");
  const parts = cleaned.split(/(\x1b\[[0-9;]*m)/g);
  let fg = null;
  let bold = false;
  let html = "";
  for (const part of parts) {
    const m = part.match(/^\x1b\[([0-9;]*)m$/);
    if (m) {
      const codes = (m[1] || "0").split(";").map((n) => parseInt(n || "0", 10));
      for (const code of codes) {
        if (code === 0) {
          fg = null;
          bold = false;
        } else if (code === 1) {
          bold = true;
        } else if (code === 22) {
          bold = false;
        } else if (ANSI_COLORS[code]) {
          fg = ANSI_COLORS[code];
        } else if (code === 39) {
          fg = null;
        }
      }
      continue;
    }
    if (!part) continue;
    const escaped = esc(part);
    if (fg || bold) {
      const style = `${fg ? `color:${fg};` : ""}${bold ? "font-weight:600;" : ""}`;
      html += `<span style="${style}">${escaped}</span>`;
    } else {
      html += escaped;
    }
  }
  return html;
}

function estimateTermDims() {
  const outEl = $("term-output");
  if (!outEl) return { cols: 80, rows: 24 };
  const cols = Math.max(40, Math.min(200, Math.floor((outEl.clientWidth - 36) / 7.6) || 80));
  const rows = Math.max(12, Math.min(80, Math.floor((outEl.clientHeight - 28) / 19) || 24));
  return { cols, rows };
}

async function connectLivePty(id) {
  if (!id) return;
  ensurePtyEventListeners();
  const sess = getTermSession(id);
  if (sess.ptySessionId || sess.ptyConnecting) return;

  sess.ptyConnecting = true;
  renderTerminal(id);
  try {
    const { cols, rows } = estimateTermDims();
    const sessionId = await invoke("pty_open", {
      containerId: id,
      cols,
      rows,
      cmd: ["/bin/sh"],
    });
    sess.ptySessionId = sessionId;
    sess.output += "\n[Connected to live Linux PTY (/bin/sh)]\n";
  } catch (e) {
    sess.output += `\n[Live PTY unavailable — using ExecSync fallback: ${String(e)}]\n`;
  } finally {
    sess.ptyConnecting = false;
    if (state.detailId === id) renderTerminal(id);
  }
}

async function disconnectLivePty(id) {
  if (!id) return;
  const sess = state.termSessions[id];
  if (!sess?.ptySessionId) return;
  const sid = sess.ptySessionId;
  sess.ptySessionId = null;
  try {
    await invoke("pty_close", { sessionId: sid });
  } catch (_) {
    // ignore close race
  }
  if (state.detailId === id) renderTerminal(id);
}

async function toggleLivePty() {
  const id = state.detailId;
  if (!id) return;
  const sess = getTermSession(id);
  if (sess.ptySessionId) {
    await disconnectLivePty(id);
  } else {
    await connectLivePty(id);
  }
}

async function sendPtyCtrlC() {
  const id = state.detailId;
  if (!id) return;
  const sess = getTermSession(id);
  if (!sess.ptySessionId) return;
  await invoke("pty_write", { sessionId: sess.ptySessionId, data: "\x03" });
}

function handleRawPtyScreenKeydown(e) {
  const id = state.detailId;
  if (!id) return;
  const sess = getTermSession(id);
  if (!sess.ptySessionId) return;

  let seq = null;
  if (e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) {
    const code = e.key.toLowerCase().charCodeAt(0) - 96;
    if (code >= 1 && code <= 26) seq = String.fromCharCode(code);
  } else if (e.key === "Enter") seq = "\r";
  else if (e.key === "Backspace") seq = "\x7f";
  else if (e.key === "Tab") seq = "\t";
  else if (e.key === "Escape") seq = "\x1b";
  else if (e.key === "ArrowUp") seq = "\x1b[A";
  else if (e.key === "ArrowDown") seq = "\x1b[B";
  else if (e.key === "ArrowRight") seq = "\x1b[C";
  else if (e.key === "ArrowLeft") seq = "\x1b[D";
  else if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key.length === 1) seq = e.key;

  if (seq !== null) {
    e.preventDefault();
    e.stopPropagation();
    invoke("pty_write", { sessionId: sess.ptySessionId, data: seq }).catch(() => {});
  }
}

function shellSingleQuote(str) {
  return "'" + String(str).replace(/'/g, "'\\''") + "'";
}

function renderTerminal(id) {
  const sess = getTermSession(id);
  const promptEl = $("term-prompt");
  const outEl = $("term-output");
  const badge = $("pty-status-badge");
  const btn = $("pty-connect-btn");
  const live = Boolean(sess.ptySessionId);

  if (badge) {
    badge.textContent = sess.ptyConnecting
      ? "Connecting PTY…"
      : live
        ? "Live PTY"
        : "Exec Mode";
    badge.className = "live-badge" + (live ? " live" : "");
  }
  if (btn) {
    btn.textContent = live ? "Disconnect" : "Connect PTY";
    btn.classList.toggle("primary", !live);
    btn.disabled = sess.ptyConnecting;
  }
  if ($("pty-ctrl-c")) {
    $("pty-ctrl-c").disabled = !live;
  }
  if (promptEl) {
    const shortCwd = sess.cwd.length > 22 ? "…" + sess.cwd.slice(-21) : sess.cwd;
    promptEl.textContent = live ? "pty#" : `root:${shortCwd}#`;
  }
  if (outEl) {
    outEl.innerHTML = renderAnsiToHtml(sess.output);
    outEl.scrollTop = outEl.scrollHeight;
  }
}

function clearTerminal() {
  if (!state.detailId) return;
  const sess = getTermSession(state.detailId);
  sess.output = "";
  renderTerminal(state.detailId);
}

async function runTerminalCommand(explicitCmd = null) {
  const id = state.detailId;
  if (!id) return;
  const input = $("term-input");
  const raw = explicitCmd !== null ? String(explicitCmd).trim() : input.value.trim();
  if (!raw) return;

  const sess = getTermSession(id);
  if (explicitCmd === null) input.value = "";

  if (raw === "clear" && !sess.ptySessionId) {
    clearTerminal();
    return;
  }

  if (sess.history[sess.history.length - 1] !== raw) {
    sess.history.push(raw);
    if (sess.history.length > 100) sess.history.shift();
  }
  sess.histIdx = sess.history.length;

  // 1. Live PTY mode: write command directly to the streaming Linux PTY!
  if (sess.ptySessionId) {
    try {
      await invoke("pty_write", { sessionId: sess.ptySessionId, data: `${raw}\r` });
    } catch (e) {
      toast(String(e), true);
    }
    return;
  }

  // 2. ExecSync fallback mode (exited containers or detached PTY).
  if (sess.busy) return;
  sess.output += `\nroot:${sess.cwd}# ${raw}\n`;
  sess.busy = true;
  if ($("term-run")) $("term-run").disabled = true;
  renderTerminal(id);

  const marker = "__CORAL_PWD__";
  const wrapped =
    `cd ${shellSingleQuote(sess.cwd)} 2>/dev/null || cd /; ` +
    `${raw}; __coral_ec=$?; ` +
    `printf '\\n${marker}%s\\n' "$(pwd)"; ` +
    `exit $__coral_ec`;

  try {
    const res = await invoke("exec_container", {
      id,
      argv: ["/bin/sh", "-c", wrapped],
    });
    let stdout = res.stdout || "";
    const markerIdx = stdout.lastIndexOf(marker);
    if (markerIdx >= 0) {
      const after = stdout.slice(markerIdx + marker.length).trim();
      const firstLine = after.split(/\r?\n/)[0]?.trim();
      if (firstLine) sess.cwd = firstLine;
      stdout = stdout.slice(0, markerIdx).replace(/\n$/, "");
    }
    if (stdout) sess.output += stdout + (stdout.endsWith("\n") ? "" : "\n");
    if (res.stderr) sess.output += `[stderr] ${res.stderr}` + (res.stderr.endsWith("\n") ? "" : "\n");
    if (res.exitCode !== 0) sess.output += `[exit ${res.exitCode}]\n`;
  } catch (e) {
    sess.output += `[error] ${String(e)}\n`;
  } finally {
    sess.busy = false;
    if ($("term-run")) $("term-run").disabled = false;
    renderTerminal(id);
    if (state.detailTab === "term" && input) input.focus();
  }
}

function handleTerminalKeydown(e) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "l") {
    e.preventDefault();
    clearTerminal();
    return;
  }
  if (e.key === "Enter" && !e.isComposing) {
    e.preventDefault();
    runTerminalCommand();
    return;
  }
  if (!state.detailId) return;
  const sess = getTermSession(state.detailId);
  if (!sess.history.length) return;
  if (e.key === "ArrowUp") {
    e.preventDefault();
    sess.histIdx = Math.max(0, sess.histIdx - 1);
    $("term-input").value = sess.history[sess.histIdx] || "";
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    sess.histIdx = Math.min(sess.history.length, sess.histIdx + 1);
    $("term-input").value = sess.histIdx < sess.history.length ? sess.history[sess.histIdx] : "";
  }
}

// ---------- Live Resource Metrics & SVG Sparklines ----------

function recordMetricsSample(id, metrics) {
  if (!metrics) return;
  if (!state.metricsHistory[id]) state.metricsHistory[id] = [];
  const list = state.metricsHistory[id];
  const prev = list[list.length - 1];
  const cpuDeltaMs = prev
    ? Math.max(0, (metrics.cpuUsageUsec - prev.cpuUsageUsec) / 1000)
    : metrics.cpuUsageUsec / 1000;
  list.push({
    ts: Date.now(),
    memBytes: metrics.memoryBytes || 0,
    cpuUsageUsec: metrics.cpuUsageUsec || 0,
    cpuDeltaMs,
  });
  if (list.length > 24) list.shift();
}

function buildSparklineSvg(values, strokeColor, fillColor) {
  const w = 220;
  const h = 50;
  if (!values.length) {
    return `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-hidden="true">
      <line x1="0" y1="${h - 6}" x2="${w}" y2="${h - 6}" stroke="var(--line)" stroke-dasharray="3 3" stroke-width="1" />
    </svg>`;
  }
  const maxVal = Math.max(...values, 1);
  const pts = values.map((v, i) => {
    const x = values.length === 1 ? w : (i / (values.length - 1)) * w;
    const y = h - 4 - (v / maxVal) * (h - 10);
    return [x.toFixed(1), y.toFixed(1)];
  });
  const line = pts.map(([x, y]) => `${x},${y}`).join(" ");
  const area = `0,${h} ${line} ${w},${h}`;
  return `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-hidden="true">
    <polygon points="${area}" fill="${fillColor}" />
    <polyline points="${line}" fill="none" stroke="${strokeColor}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" />
  </svg>`;
}

function fmtUptime(secs) {
  const s = Number(secs) || 0;
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function renderMetricsPanel(id, d) {
  const cardsEl = $("metrics-cards");
  const memWrap = $("spark-mem-svg");
  const cpuWrap = $("spark-cpu-svg");
  if (!cardsEl || !memWrap || !cpuWrap) return;

  const m = d.metrics;
  if (m) recordMetricsSample(id, m);
  const hist = state.metricsHistory[id] || [];

  const memText = m ? fmtBytes(m.memoryBytes) : "—";
  const memSub = m && m.memoryLimitBytes > 0 ? `Limit: ${fmtBytes(m.memoryLimitBytes)}` : "No memory limit";
  const cpuMs = m ? `${(m.cpuUsageUsec / 1000).toFixed(1)} ms` : "—";
  const pidsText = m ? String(m.pidsCurrent) : "—";
  const uptimeText = m ? fmtUptime(m.uptimeSecs) : "—";
  const srcText = m ? `PID ${m.pid} • ${m.source}` : "Container not running";

  cardsEl.innerHTML = `
    <div class="metric-card"><span class="mk">Memory</span><span class="mv">${esc(memText)}</span><span class="ms">${esc(memSub)}</span></div>
    <div class="metric-card"><span class="mk">CPU Time</span><span class="mv">${esc(cpuMs)}</span><span class="ms">${esc(srcText)}</span></div>
    <div class="metric-card"><span class="mk">PIDs / Threads</span><span class="mv">${esc(pidsText)}</span><span class="ms">Restart: ${esc(d.restartPolicy || "no")} (${esc(d.restartCount || 0)})</span></div>
    <div class="metric-card"><span class="mk">Uptime</span><span class="mv">${esc(uptimeText)}</span><span class="ms">Health: ${esc(d.healthStatus || "none")}</span></div>`;

  $("spark-mem-val").textContent = memText;
  $("spark-cpu-val").textContent = cpuMs;
  memWrap.innerHTML = buildSparklineSvg(
    hist.map((p) => p.memBytes),
    "var(--blue-light)",
    "color-mix(in srgb, var(--blue) 18%, transparent)",
  );
  cpuWrap.innerHTML = buildSparklineSvg(
    hist.map((p) => p.cpuDeltaMs),
    "var(--moss)",
    "color-mix(in srgb, var(--moss) 18%, transparent)",
  );
}

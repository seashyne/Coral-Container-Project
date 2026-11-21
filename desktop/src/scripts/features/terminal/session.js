// Terminal panel: PTY session handling, ANSI rendering and live connect/disconnect.
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
  if (chunk.includes("\x1b[2J") || chunk.includes("\x1b[H\x1b[J")) {
    const parts = chunk.split(/\x1b\[2J|\x1b\[H\x1b\[J/);
    sess.output = parts[parts.length - 1];
  } else {
    sess.output += chunk.replace(/\r\n/g, "\n");
  }
  if (sess.output.length > 65536) {
    sess.output = sess.output.slice(-65536);
  }
}

function renderAnsiToHtml(raw) {
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

window.addEventListener("resize", () => {
  const id = state.detailId;
  if (!id || state.detailTab !== "term") return;
  const sess = state.termSessions[id];
  if (!sess?.ptySessionId) return;
  const { cols, rows } = estimateTermDims();
  invoke("pty_resize", { sessionId: sess.ptySessionId, cols, rows }).catch(() => {});
});

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

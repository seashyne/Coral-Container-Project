// Terminal panel: raw screen keys, control-C and terminal rendering.

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
      ? t("drawer.connectingPty")
      : live
        ? t("drawer.livePty")
        : t("drawer.execMode");
    badge.className = "live-badge" + (live ? " live" : "");
  }
  if (btn) {
    btn.textContent = t(live ? "drawer.disconnectPty" : "drawer.connectPty");
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

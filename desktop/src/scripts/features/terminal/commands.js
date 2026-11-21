// Terminal panel: command execution and terminal keyboard handling.

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

  if (sess.ptySessionId) {
    try {
      await invoke("pty_write", { sessionId: sess.ptySessionId, data: `${raw}\r` });
    } catch (e) {
      toast(String(e), true);
    }
    return;
  }

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


// Modal dialogs, compose file browser, drawer and terminal panel bindings.
// ---------- Modals, Compose File Browser & Dialog Closers ----------

document.querySelectorAll("[data-close-dialog]").forEach((btn) =>
  btn.addEventListener("click", () => closeDialog(btn.dataset.closeDialog)),
);

$("run-submit").addEventListener("click", submitRun);
document.querySelectorAll("[data-run-preset]").forEach((btn) =>
  btn.addEventListener("click", () => {
    const key = btn.dataset.runPreset;
    const preset = RUN_PRESETS[key];
    if (preset) applyRunPreset(preset, key);
  }),
);
document.querySelectorAll("[data-run-mem]").forEach((btn) =>
  btn.addEventListener("click", () => {
    $("run-memory").value = btn.dataset.runMem;
    syncRunMemoryPills();
  }),
);
$("run-memory")?.addEventListener("input", syncRunMemoryPills);

$("compose-browse-btn").addEventListener("click", () => browseComposeFile(false));
$("compose-template-select")?.addEventListener("change", (e) => {
  const tpl = COMPOSE_TEMPLATES[e.target.value];
  if (tpl) $("compose-yaml").value = tpl;
});
$("compose-up-btn").addEventListener("click", submitComposeUp);
$("compose-down-btn").addEventListener("click", submitComposeDown);

$("commit-btn").addEventListener("click", commitContainer);
$("detail-close").addEventListener("click", closeDetail);
$("drawer-backdrop").addEventListener("click", closeDetail);

initDetailTabs();
$("log-autoscroll").addEventListener("change", (e) => {
  state.logAutoScroll = Boolean(e.target.checked);
});
$("log-refresh").addEventListener("click", () => refreshDetail(false));
$("pty-connect-btn").addEventListener("click", toggleLivePty);
$("pty-ctrl-c").addEventListener("click", sendPtyCtrlC);
$("term-output").addEventListener("keydown", handleRawPtyScreenKeydown);
$("term-run").addEventListener("click", () => runTerminalCommand());
$("term-clear").addEventListener("click", clearTerminal);
$("term-input").addEventListener("keydown", handleTerminalKeydown);
document.querySelectorAll("[data-term-quick]").forEach((btn) =>
  btn.addEventListener("click", () => runTerminalCommand(btn.dataset.termQuick)),
);

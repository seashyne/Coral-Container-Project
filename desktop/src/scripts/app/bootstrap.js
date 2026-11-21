// Bootstrap: icons, dialogs, theme, i18n, first render and auto-connect.
// ---------- Bootstrap & Auto-Connect ----------

(async function init() {
  initIcons();
  initDialogs();
  initThemePopover();
  applyI18nToDom();
  syncThemeControls();
  applyPalette(localStorage.getItem("coral.palette") || "coral");
  applyDensity(localStorage.getItem("coral.density") || "comfortable");
  if (localStorage.getItem("coral.rail") === "1") document.body.classList.add("rail");

  updateModeHint();
  updateSearchPlaceholder(state.view);
  renderContainerToolbar();
  renderEmptyViews();
  updateChrome();

  await guard(async () => {
    const info = await invoke("connection_status");
    if (info.connected) {
      setConnected(true, info);
      await refreshAll(true);
    } else if (state.autoConnect) {
      await connect(true);
    }
  });
})();

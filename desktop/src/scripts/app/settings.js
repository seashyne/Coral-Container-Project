// Settings view event bindings.
// ---------- Settings View Events ----------

const selectSettingsTab = setupTabGroup("settings-tablist", (tabKey) => {
  state.settingsTab = tabKey;
});

$("save-settings").addEventListener("click", () => saveSettings(false));
$("save-adv-settings")?.addEventListener("click", () => saveSettings(false));
$("save-connect").addEventListener("click", () => saveSettings(true));
$("settings-conn-btn").addEventListener("click", () =>
  state.connected ? disconnect() : connect(),
);
$("set-mode")?.addEventListener("change", updateModeHint);
$("set-compact-rail")?.addEventListener("change", toggleRail);

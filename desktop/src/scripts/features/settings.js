// Settings view: Connection settings, Theme/Palette/Density controls, Language switcher, and Advanced paths.

function updateModeHint() {
  const mode = $("set-mode")?.value || state.settings?.mode || "local";
  const hintKey = MODE_HINT_KEYS[mode];
  if ($("mode-hint")) {
    $("mode-hint").textContent = hintKey ? t(hintKey) : "";
  }
}

async function loadSettingsForm() {
  await guard(async () => {
    const s = await invoke("get_settings");
    state.settings = s;
    if ($("set-mode")) $("set-mode").value = s.mode;
    if ($("set-endpoint")) $("set-endpoint").value = s.endpoint;
    if ($("set-daemon-bin")) $("set-daemon-bin").value = s.daemonBin;
    if ($("set-coral-bin")) $("set-coral-bin").value = s.coralBin;
    if ($("set-rootfs")) $("set-rootfs").value = s.rootfs;
    if ($("set-images-dir")) $("set-images-dir").value = s.imagesDir || "";
    if ($("set-state-dir")) $("set-state-dir").value = s.stateDir;
    if ($("set-wsl-distro")) $("set-wsl-distro").value = s.wslDistro;
    if ($("set-auth-token")) $("set-auth-token").value = s.authToken || "";
    if ($("set-docker-sock")) {
      const base = (s.stateDir || "").trim() || "~/.local/state/coral/daemon";
      $("set-docker-sock").value = `unix://${base.replace(/\/+$/, "")}/docker.sock`;
    }
    updateModeHint();

    syncThemeControls();
    applyPalette(localStorage.getItem("coral.palette") || "coral");
    applyDensity(localStorage.getItem("coral.density") || "comfortable");
    if ($("set-lang")) $("set-lang").value = getLang();
    if ($("set-auto-connect")) $("set-auto-connect").checked = localStorage.getItem("coral.autoConnect") !== "0";
    if ($("set-compact-rail")) $("set-compact-rail").checked = localStorage.getItem("coral.rail") === "1";
  });
}

function collectSettings() {
  return {
    mode: $("set-mode")?.value || "local",
    endpoint: ($("set-endpoint")?.value || "127.0.0.1:50051").trim(),
    daemonBin: ($("set-daemon-bin")?.value || "coral-daemon").trim(),
    coralBin: ($("set-coral-bin")?.value || "coral").trim(),
    rootfs: ($("set-rootfs")?.value || "").trim(),
    imagesDir: ($("set-images-dir")?.value || "").trim(),
    stateDir: ($("set-state-dir")?.value || "").trim(),
    wslDistro: ($("set-wsl-distro")?.value || "").trim(),
    authToken: ($("set-auth-token")?.value || "").trim(),
  };
}

async function saveSettings(andConnect) {
  await guard(async () => {
    const newSettings = collectSettings();
    await invoke("save_settings", { newSettings });
    state.settings = newSettings;
    const autoConn = Boolean($("set-auto-connect")?.checked);
    state.autoConnect = autoConn;
    localStorage.setItem("coral.autoConnect", autoConn ? "1" : "0");
    toast(t("toast.settingsSaved"));
    setConnected(false);
    renderEmptyViews();
    if (andConnect) await connect();
  });
}

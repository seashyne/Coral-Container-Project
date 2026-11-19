// Settings view form loading, collection, and persistence.

async function loadSettingsForm() {
  await guard(async () => {
    const s = await invoke("get_settings");
    state.settings = s;
    $("set-mode").value = s.mode;
    $("set-endpoint").value = s.endpoint;
    $("set-daemon-bin").value = s.daemonBin;
    $("set-coral-bin").value = s.coralBin;
    $("set-rootfs").value = s.rootfs;
    $("set-images-dir").value = s.imagesDir || "";
    $("set-state-dir").value = s.stateDir;
    $("set-wsl-distro").value = s.wslDistro;
    $("mode-hint").textContent = MODE_HINTS[s.mode] || "";
    $("set-theme").value = localStorage.getItem("coral.theme") || "auto";
  });
}

function collectSettings() {
  return {
    mode: $("set-mode").value,
    endpoint: $("set-endpoint").value.trim(),
    daemonBin: $("set-daemon-bin").value.trim(),
    coralBin: $("set-coral-bin").value.trim(),
    rootfs: $("set-rootfs").value.trim(),
    imagesDir: $("set-images-dir").value.trim(),
    stateDir: $("set-state-dir").value.trim(),
    wslDistro: $("set-wsl-distro").value.trim(),
  };
}

async function saveSettings(andConnect) {
  await guard(async () => {
    await invoke("save_settings", { newSettings: collectSettings() });
    toast("Settings saved");
    setConnected(false);
    renderEmptyViews();
    if (andConnect) await connect();
  });
}

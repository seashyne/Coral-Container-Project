// Application shell bindings: topbar chrome, navigation, engine toggle and global search.
function syncSearchClearButton() {
  const btn = $("search-clear");
  if (btn) btn.classList.toggle("hidden", !$("search")?.value);
}

function openRunModalFromCatalog(baseItem, overrideTag = null) {
  openRunModal({
    slug: baseItem.id,
    image: overrideTag || baseItem.ref,
    command: baseItem.defaultCmd,
    args: baseItem.defaultArgs,
    ports: baseItem.defaultPorts,
    volumes: baseItem.defaultVolumes,
    env: baseItem.defaultEnv,
    memoryMb: baseItem.defaultMemoryMb,
  });
}

$("rail-btn").addEventListener("click", toggleRail);
$("gear-btn").addEventListener("click", () => switchView("settings"));
$("theme-pop-settings")?.addEventListener("click", () => {
  closeThemePopover();
  switchView("settings");
});

$("set-theme")?.addEventListener("change", (e) => applyTheme(e.target.value));
$("set-lang")?.addEventListener("change", (e) => setLang(e.target.value, true));
$("set-density")?.addEventListener("change", (e) => applyDensity(e.target.value));
document.querySelectorAll("[data-theme-pick]").forEach((btn) =>
  btn.addEventListener("click", () => applyTheme(btn.dataset.themePick)),
);
document.querySelectorAll("[data-palette-pick]").forEach((btn) =>
  btn.addEventListener("click", () => applyPalette(btn.dataset.palettePick)),
);
document.querySelectorAll("[data-density-pick]").forEach((btn) =>
  btn.addEventListener("click", () => applyDensity(btn.dataset.densityPick)),
);
document.querySelectorAll("[data-lang-pick]").forEach((btn) =>
  btn.addEventListener("click", () => setLang(btn.dataset.langPick, true)),
);

document.querySelectorAll(".nav-item").forEach((b) =>
  b.addEventListener("click", () => switchView(b.dataset.view)),
);

$("vm-btn").addEventListener("click", () =>
  state.connected ? disconnect() : connect(),
);

function rerenderActiveSearch() {
  if (state.view === "containers") renderContainersTable();
  else if (state.view === "images") {
    renderBaseCatalog();
    renderInstalledImages(state.images);
  } else if (state.view === "volumes") {
    renderVolumesTable();
  } else if (state.view === "pods") {
    renderPodsTable();
  }
}

$("search").addEventListener("input", (e) => {
  state.search = e.target.value;
  syncSearchClearButton();
  rerenderActiveSearch();
});

$("search-clear")?.addEventListener("click", () => {
  $("search").value = "";
  state.search = "";
  syncSearchClearButton();
  rerenderActiveSearch();
  $("search").focus();
});

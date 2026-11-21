// Theme Engine: Color Scheme (Auto / Dark / Light with <meta name="color-scheme"> sync),
// Accent Palette Presets (Coral / Ocean / Emerald / Indigo / Amber), UI Density, and Popover wiring.

const THEME_STORAGE_KEY = "coral.theme";
const PALETTE_STORAGE_KEY = "coral.palette";
const DENSITY_STORAGE_KEY = "coral.density";
const VALID_PALETTES = new Set(["coral", "ocean", "emerald", "indigo", "amber"]);
const darkSchemeMedia = window.matchMedia("(prefers-color-scheme: dark)");

function getStoredThemeOverride() {
  try {
    const val = localStorage.getItem(THEME_STORAGE_KEY);
    return val === "dark" || val === "light" ? val : null;
  } catch {
    return null;
  }
}

function getSystemScheme() {
  return darkSchemeMedia.matches ? "dark" : "light";
}

function syncThemeControls() {
  const override = getStoredThemeOverride();
  const rendered = override ?? getSystemScheme();
  const modeValue = override ?? "auto";

  const meta = document.querySelector('meta[name="color-scheme"]');
  if (meta) {
    meta.content = override ?? "light dark";
  }
  document.documentElement.dataset.theme = modeValue;

  const btn = $("theme-btn");
  const iconSlot = $("theme-btn-icon");
  if (iconSlot) {
    iconSlot.innerHTML = rendered === "light" ? ICONS.moon : ICONS.sun;
  }
  if (btn) {
    const modeLabel =
      modeValue === "auto"
        ? t("settings.themeAuto", {
            scheme: t(
              getSystemScheme() === "dark" ? "settings.themeAutoDark" : "settings.themeAutoLight",
            ),
          })
        : t(modeValue === "dark" ? "settings.themeDark" : "settings.themeLight");
    btn.title = `${t("topbar.themeMenu")} (${modeLabel})`;
  }

  document.querySelectorAll("[data-theme-pick]").forEach((opt) => {
    opt.setAttribute("aria-pressed", opt.dataset.themePick === modeValue ? "true" : "false");
  });

  const sel = $("set-theme");
  if (sel) {
    sel.value = modeValue;
    const autoOpt = sel.querySelector('option[value="auto"]');
    if (autoOpt) {
      autoOpt.textContent = t("settings.themeAuto", {
        scheme: t(
          getSystemScheme() === "dark" ? "settings.themeAutoDark" : "settings.themeAutoLight",
        ),
      });
    }
  }
}

function applyTheme(theme) {
  try {
    if (theme === "dark" || theme === "light") {
      localStorage.setItem(THEME_STORAGE_KEY, theme);
    } else {
      localStorage.removeItem(THEME_STORAGE_KEY);
    }
  } catch {
    // Ignore storage write error
  }
  syncThemeControls();
}

function applyPalette(palette) {
  const valid = VALID_PALETTES.has(palette) ? palette : "coral";
  document.documentElement.dataset.palette = valid;
  try {
    localStorage.setItem(PALETTE_STORAGE_KEY, valid);
  } catch {
    // Ignore storage error
  }
  document.querySelectorAll("[data-palette-pick]").forEach((btn) => {
    btn.setAttribute("aria-pressed", btn.dataset.palettePick === valid ? "true" : "false");
  });
}

function applyDensity(density) {
  const valid = density === "compact" ? "compact" : "comfortable";
  document.documentElement.dataset.density = valid;
  try {
    localStorage.setItem(DENSITY_STORAGE_KEY, valid);
  } catch {
    // Ignore storage error
  }
  const sel = $("set-density");
  if (sel) sel.value = valid;
  document.querySelectorAll("[data-density-pick]").forEach((btn) => {
    btn.setAttribute("aria-pressed", btn.dataset.densityPick === valid ? "true" : "false");
  });
}

function closeThemePopover() {
  const pop = $("theme-popover");
  if (!pop) return;
  if (Object.hasOwn(HTMLElement.prototype, "popover") && typeof pop.hidePopover === "function") {
    if (pop.matches(":popover-open")) pop.hidePopover();
  } else {
    pop.classList.remove("is-open");
  }
}

function initThemePopover() {
  const pop = $("theme-popover");
  const trigger = $("theme-btn");
  if (!pop || !trigger) return;

  if (!Object.hasOwn(HTMLElement.prototype, "popover")) {
    document.documentElement.classList.add("no-popover");
    trigger.addEventListener("click", (e) => {
      e.stopPropagation();
      pop.classList.toggle("is-open");
    });
    document.addEventListener("click", (e) => {
      if (!pop.contains(e.target) && !trigger.contains(e.target)) {
        pop.classList.remove("is-open");
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && pop.classList.contains("is-open")) {
        pop.classList.remove("is-open");
        trigger.focus();
      }
    });
  }
}

// Never retroactively clear an explicit pin when OS changes to match it; only refresh UI hints.
darkSchemeMedia.addEventListener("change", () => syncThemeControls());

window.addEventListener("storage", (e) => {
  if (e.key === THEME_STORAGE_KEY) syncThemeControls();
  if (e.key === PALETTE_STORAGE_KEY) applyPalette(e.newValue || "coral");
  if (e.key === DENSITY_STORAGE_KEY) applyDensity(e.newValue || "comfortable");
});

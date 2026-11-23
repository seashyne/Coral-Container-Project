// Multi-language Internationalization (i18n) controller for Coral Desktop (8 languages).

const LANG_STORAGE_KEY = "coral.lang";

const CORAL_LANGUAGES = [
  { code: "en", short: "EN", native: "English" },
  { code: "th", short: "ไทย", native: "ไทย" },
  { code: "zh", short: "中文", native: "简体中文" },
  { code: "ja", short: "日本語", native: "日本語" },
  { code: "ko", short: "한국어", native: "한국어" },
  { code: "vi", short: "VI", native: "Tiếng Việt" },
  { code: "es", short: "ES", native: "Español" },
  { code: "de", short: "DE", native: "Deutsch" },
];

const SUPPORTED_LANG_CODES = new Set(CORAL_LANGUAGES.map((l) => l.code));

function normalizeLang(raw) {
  if (!raw || typeof raw !== "string") return null;
  const base = raw.trim().toLowerCase().split(/[-_]/)[0];
  return SUPPORTED_LANG_CODES.has(base) ? base : null;
}

function getInitialLang() {
  try {
    const saved = normalizeLang(localStorage.getItem(LANG_STORAGE_KEY));
    if (saved) return saved;
  } catch {
    // Ignore storage read error
  }
  const prefs = Array.isArray(navigator.languages) && navigator.languages.length
    ? navigator.languages
    : [navigator.language];
  for (const tag of prefs) {
    const matched = normalizeLang(tag);
    if (matched) return matched;
  }
  return "en";
}

let currentLang = getInitialLang();

function getLang() {
  return currentLang;
}

function getLangMeta(code) {
  return CORAL_LANGUAGES.find((l) => l.code === code) || CORAL_LANGUAGES[0];
}

function t(key, vars = null) {
  const dicts = window.CORAL_DESKTOP_LOCALES || {};
  const active = dicts[currentLang] || {};
  const fallback = dicts.en || {};
  let text = active[key] ?? fallback[key] ?? key;
  if (vars && typeof text === "string") {
    for (const [k, v] of Object.entries(vars)) {
      text = text.replaceAll(`{${k}}`, String(v ?? ""));
    }
  }
  return text;
}

function tState(rawState) {
  const s = String(rawState || "").toLowerCase();
  return t(`state.${s}`) !== `state.${s}` ? t(`state.${s}`) : rawState;
}

function tHealth(rawHealth) {
  const h = String(rawHealth || "").toLowerCase();
  if (!h || h === "none") return t("common.none");
  return t(`health.${h}`) !== `health.${h}` ? t(`health.${h}`) : rawHealth;
}

function closeLangPopover() {
  const pop = $("lang-popover");
  if (!pop) return;
  if (Object.hasOwn(HTMLElement.prototype, "popover") && typeof pop.hidePopover === "function") {
    if (pop.matches(":popover-open")) pop.hidePopover();
  } else {
    pop.classList.remove("is-open");
  }
}

function initLangPopover() {
  const pop = $("lang-popover");
  const trigger = $("lang-btn");
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

function applyI18nToDom() {
  document.documentElement.lang = currentLang;

  document.querySelectorAll("[data-i18n]").forEach((el) => {
    el.textContent = t(el.dataset.i18n);
  });
  document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
    el.placeholder = t(el.dataset.i18nPlaceholder);
  });
  document.querySelectorAll("[data-i18n-title]").forEach((el) => {
    el.title = t(el.dataset.i18nTitle);
  });
  document.querySelectorAll("[data-i18n-aria]").forEach((el) => {
    el.setAttribute("aria-label", t(el.dataset.i18nAria));
  });

  const langLabel = $("lang-label");
  if (langLabel) {
    langLabel.textContent = getLangMeta(currentLang).short;
  }
  const setLangSel = $("set-lang");
  if (setLangSel) {
    setLangSel.value = currentLang;
  }
  document.querySelectorAll("[data-lang-pick]").forEach((btn) => {
    btn.setAttribute("aria-pressed", btn.dataset.langPick === currentLang ? "true" : "false");
  });
}

function setLang(lang, rerender = true) {
  currentLang = normalizeLang(lang) || "en";
  try {
    localStorage.setItem(LANG_STORAGE_KEY, currentLang);
  } catch {
    // Ignore storage write error
  }
  applyI18nToDom();
  closeLangPopover();
  if (rerender) {
    if (typeof syncThemeControls === "function") syncThemeControls();
    if (typeof updateModeHint === "function") updateModeHint();
    if (typeof updateSearchPlaceholder === "function") updateSearchPlaceholder(state.view);
    if (typeof updateChrome === "function") updateChrome();
    if (typeof renderContainerToolbar === "function") renderContainerToolbar();
    if (typeof renderContainersTable === "function") renderContainersTable();
    if (typeof renderBaseCatalog === "function") renderBaseCatalog();
    if (typeof renderInstalledImages === "function") renderInstalledImages(state.images);
    if (typeof updatePullStatusBanner === "function") updatePullStatusBanner();
    if (typeof renderDiskUsage === "function") renderDiskUsage();
    if (typeof renderVolumesTable === "function") renderVolumesTable();
    if (typeof renderPodsTable === "function") renderPodsTable();
    if (state.detailId && typeof refreshDetail === "function") refreshDetail(true);
  }
}

function toggleLang() {
  const idx = CORAL_LANGUAGES.findIndex((l) => l.code === currentLang);
  const next = CORAL_LANGUAGES[(idx + 1) % CORAL_LANGUAGES.length];
  setLang(next.code, true);
}

// Bilingual Internationalization (i18n) controller for Coral Desktop (EN / TH).

const LANG_STORAGE_KEY = "coral.lang";

function getInitialLang() {
  try {
    const saved = localStorage.getItem(LANG_STORAGE_KEY);
    if (saved === "th" || saved === "en") return saved;
  } catch {
    // Ignore storage read error
  }
  return navigator.language && navigator.language.toLowerCase().startsWith("th") ? "th" : "en";
}

let currentLang = getInitialLang();

function getLang() {
  return currentLang;
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
    langLabel.textContent = currentLang === "th" ? "ไทย" : "EN";
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
  currentLang = lang === "th" ? "th" : "en";
  try {
    localStorage.setItem(LANG_STORAGE_KEY, currentLang);
  } catch {
    // Ignore storage write error
  }
  applyI18nToDom();
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
  setLang(currentLang === "th" ? "en" : "th", true);
}

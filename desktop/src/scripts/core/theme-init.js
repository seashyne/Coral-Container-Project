// Synchronous pre-paint bootstrap placed right after <meta name="color-scheme">
// to prevent flash of un-themed content (FOUC) under strict CSP (`script-src 'self'`).
(() => {
  try {
    const theme = localStorage.getItem("coral.theme");
    const meta = document.querySelector('meta[name="color-scheme"]');
    if (theme === "dark" || theme === "light") {
      document.documentElement.dataset.theme = theme;
      if (meta) meta.content = theme;
    } else {
      document.documentElement.dataset.theme = "auto";
      if (meta) meta.content = "light dark";
    }

    const palette = localStorage.getItem("coral.palette");
    if (
      palette === "coral" ||
      palette === "ocean" ||
      palette === "emerald" ||
      palette === "indigo" ||
      palette === "amber"
    ) {
      document.documentElement.dataset.palette = palette;
    } else {
      document.documentElement.dataset.palette = "coral";
    }

    const density = localStorage.getItem("coral.density");
    if (density === "compact" || density === "comfortable") {
      document.documentElement.dataset.density = density;
    } else {
      document.documentElement.dataset.density = "comfortable";
    }

    const savedLang = localStorage.getItem("coral.lang");
    const detected =
      savedLang === "th" || savedLang === "en"
        ? savedLang
        : navigator.language && navigator.language.toLowerCase().startsWith("th")
          ? "th"
          : "en";
    document.documentElement.lang = detected;
  } catch {
    // Ignore storage access errors in restricted webviews.
  }
})();

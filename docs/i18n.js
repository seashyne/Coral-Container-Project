/**
 * Coral Container Project — 8-Language Dictionary Loader (en, th, zh, ja, ko, vi, es, de).
 * Merges base dictionaries and extra locale packs into window.CORAL_I18N.
 */
(() => {
  const enBase = Object.assign(
    {},
    window.CORAL_I18N_EN || {},
    window.CORAL_I18N_EN_FEATURES || {},
  );
  const thBase = Object.assign(
    {},
    window.CORAL_I18N_TH || {},
    window.CORAL_I18N_TH_FEATURES || {},
  );
  const extra = window.CORAL_I18N_EXTRA || {};
  const map = { en: enBase, th: thBase };
  for (const code of ["zh", "ja", "ko", "vi", "es", "de"]) {
    map[code] = Object.assign({}, enBase, extra[code] || {});
  }
  window.CORAL_I18N = map;
})();

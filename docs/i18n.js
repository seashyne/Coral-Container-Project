/**
 * Coral Container Project — Thai / English Bilingual Dictionary (loader)
 *
 * The dictionaries live in ./i18n/th.js and ./i18n/en.js. This file only
 * merges them into the single `window.CORAL_I18N` map that app.js consumes.
 */
window.CORAL_I18N = {
  th: window.CORAL_I18N_TH || {},
  en: window.CORAL_I18N_EN || {},
};

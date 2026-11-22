/**
 * Coral Container Project — Thai / English Bilingual Dictionary (loader)
 *
 * The dictionaries live in ./i18n/th.js, ./i18n/en.js and the platform-feature
 * dictionaries in ./i18n/features-th.js and ./i18n/features-en.js. This file
 * only merges them into the single `window.CORAL_I18N` map that app.js consumes.
 */
window.CORAL_I18N = {
  th: Object.assign(
    {},
    window.CORAL_I18N_TH || {},
    window.CORAL_I18N_TH_FEATURES || {},
  ),
  en: Object.assign(
    {},
    window.CORAL_I18N_EN || {},
    window.CORAL_I18N_EN_FEATURES || {},
  ),
};

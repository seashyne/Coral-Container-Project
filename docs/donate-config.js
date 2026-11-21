/**
 * Coral Container Project — Donation & Sponsorship Configuration
 *
 * Edit this single file to activate your real sponsorship links & PromptPay QR!
 * 1. Set `configured: true` and replace placeholder URLs / details below.
 * 2. For PromptPay QR, drop your image at `docs/assets/promptpay-qr.png`
 *    and set `promptpay.qrImage = "assets/promptpay-qr.png"`.
 */
window.CORAL_SPONSOR_CONFIG = {
  releaseVersion: "v2.3.0",
  repoSlug: "seashyne/Coral-Container-Project",

  promptpay: {
    configured: false,
    // Replace with "assets/promptpay-qr.png" once you add your QR image:
    qrImage: "",
    accountName: "Coral Container Project Maintainer",
    promptpayId: "0XX-XXX-XXXX (Edit docs/donate-config.js)",
  },

  githubSponsors: {
    configured: false,
    url: "https://github.com/sponsors/seashyne",
    handle: "@seashyne",
  },

  buyMeACoffee: {
    configured: true,
    url: "https://buymeacoffee.com/seashyne?new=1",
    handle: "buymeacoffee.com/seashyne",
  },
};

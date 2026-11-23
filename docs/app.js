(() => {
  "use strict";

  const STORAGE_THEME = "color-scheme";
  const STORAGE_LANG = "coral-docs-lang";

  /* -----------------------------------------------------------------------
     1. Dark Mode Two-State Toggle (follows dark-mode-toggle guidance)
     ----------------------------------------------------------------------- */
  const metaColorScheme = document.querySelector('meta[name="color-scheme"]');
  const darkMedia = window.matchMedia("(prefers-color-scheme: dark)");
  const themeBtn = document.getElementById("theme-toggle");

  function getStoredTheme() {
    try {
      const val = localStorage.getItem(STORAGE_THEME);
      return val === "light" || val === "dark" ? val : null;
    } catch {
      return null;
    }
  }

  function applyThemeMeta(stored) {
    if (metaColorScheme) {
      metaColorScheme.content = stored ?? "light dark";
    }
  }

  if (themeBtn) {
    themeBtn.addEventListener("click", () => {
      const systemScheme = darkMedia.matches ? "dark" : "light";
      const currentRendered = getStoredTheme() ?? systemScheme;
      const target = currentRendered === "dark" ? "light" : "dark";

      try {
        if (target === systemScheme) {
          // Reverting back to match system: remove stored override so we don't pin
          localStorage.removeItem(STORAGE_THEME);
          applyThemeMeta(null);
        } else {
          localStorage.setItem(STORAGE_THEME, target);
          applyThemeMeta(target);
        }
      } catch {
        applyThemeMeta(target === systemScheme ? null : target);
      }
    });
  }

  // Sync theme override across browser tabs
  window.addEventListener("storage", (e) => {
    if (e.key === STORAGE_THEME) {
      applyThemeMeta(getStoredTheme());
    }
  });

  /* -----------------------------------------------------------------------
     2. Bilingual TH / EN Internationalization + Responsive Table Labels
     ----------------------------------------------------------------------- */
  const dict = window.CORAL_I18N || { th: {}, en: {} };
  const langBtn = document.getElementById("lang-toggle");
  const langLabel = document.getElementById("lang-label");

  function detectInitialLang() {
    try {
      const saved = localStorage.getItem(STORAGE_LANG);
      if (saved === "th" || saved === "en") return saved;
    } catch {}
    return navigator.language && navigator.language.toLowerCase().startsWith("th") ? "th" : "en";
  }

  let currentLang = detectInitialLang();

  function t(key) {
    return (dict[currentLang] && dict[currentLang][key]) || (dict.en && dict.en[key]) || key;
  }

  function syncTableLabels() {
    const benchTable = document.getElementById("bench-table");
    if (benchTable) {
      benchTable.style.setProperty("--label-2", JSON.stringify(t("bench.colCoral")));
      benchTable.style.setProperty("--label-3", JSON.stringify(t("bench.colRunc")));
      benchTable.style.setProperty("--label-4", JSON.stringify(t("bench.colDocker")));
    }
    const archTable = document.getElementById("arch-table");
    if (archTable) {
      archTable.style.setProperty("--label-2", JSON.stringify(t("arch.colPath")));
      archTable.style.setProperty("--label-3", JSON.stringify(t("arch.colRole")));
    }
  }

  function applyLanguage(lang) {
    currentLang = lang === "en" ? "en" : "th";
    document.documentElement.lang = currentLang;
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      const text = t(key);
      if (text) el.textContent = text;
    });
    if (langLabel) {
      langLabel.textContent = currentLang === "th" ? "EN" : "ไทย";
    }
    syncTableLabels();
    hydrateSponsorCards();
    if (typeof window.coralSyncNavbar === "function") {
      window.coralSyncNavbar();
    }
  }

  if (langBtn) {
    langBtn.addEventListener("click", () => {
      const next = currentLang === "th" ? "en" : "th";
      try {
        localStorage.setItem(STORAGE_LANG, next);
      } catch {}
      applyLanguage(next);
    });
  }

  /* -----------------------------------------------------------------------
     3. Accessible Tabs (focusgroup + beforematch + keyboard roving fallback)
     ----------------------------------------------------------------------- */
  document.querySelectorAll(".tabs").forEach((tabsRoot) => {
    const tablist = tabsRoot.querySelector('[role="tablist"]');
    const tabs = Array.from(tablist.querySelectorAll('[role="tab"]'));
    const panels = Array.from(tabsRoot.querySelectorAll('[role="tabpanel"]'));

    function selectTab(selectedTab, focusTab = false) {
      tabs.forEach((tab, idx) => {
        const isSelected = tab === selectedTab;
        tab.setAttribute("aria-selected", String(isSelected));
        tab.setAttribute("tabindex", isSelected ? "0" : "-1");
        tab.toggleAttribute("focusgroupstart", isSelected);

        const panel = panels[idx];
        if (panel) {
          if (isSelected) {
            panel.hidden = false;
            panel.setAttribute("tabindex", "0");
          } else {
            panel.hidden = "until-found";
            panel.removeAttribute("tabindex");
          }
        }
      });

      selectedTab.scrollIntoView({
        block: "nearest",
        inline: "nearest",
        container: "nearest",
      });
      if (focusTab) selectedTab.focus();
    }

    tabs.forEach((tab, idx) => {
      tab.addEventListener("click", () => selectTab(tab, false));

      tab.addEventListener("keydown", (e) => {
        let nextIdx = null;
        if (e.key === "ArrowRight") nextIdx = (idx + 1) % tabs.length;
        else if (e.key === "ArrowLeft") nextIdx = (idx - 1 + tabs.length) % tabs.length;
        else if (e.key === "Home") nextIdx = 0;
        else if (e.key === "End") nextIdx = tabs.length - 1;

        if (nextIdx !== null) {
          e.preventDefault();
          // Selection follows focus
          selectTab(tabs[nextIdx], true);
        }
      });

      const panel = panels[idx];
      if (panel) {
        // Sync tab state if browser Find-in-Page reveals a hidden="until-found" panel
        panel.addEventListener("beforematch", () => selectTab(tab, false));
      }
    });

    // Fallback for browsers without scroll-driven animations for tablist edge fades
    if (!CSS.supports("animation-timeline", "scroll()") && "IntersectionObserver" in window && tabs.length > 1) {
      const first = tabs[0];
      const last = tabs[tabs.length - 1];
      const fadeObserver = new IntersectionObserver(
        (entries) => {
          for (const { target, intersectionRatio } of entries) {
            tablist.classList.toggle(target === first ? "more-start" : "more-end", intersectionRatio < 1);
          }
        },
        { root: tablist, threshold: 1, rootMargin: "1px" }
      );
      fadeObserver.observe(first);
      fadeObserver.observe(last);
    }
  });

  /* -----------------------------------------------------------------------
     4. Scrollspy (CSS scroll-target-group + scrollend ARIA / IO fallback)
     ----------------------------------------------------------------------- */
  const tocLinks = Array.from(document.querySelectorAll(".toc-nav a"));
  const sections = Array.from(document.querySelectorAll("main .doc-section[id]"));

  if (CSS.supports("scroll-target-group: auto")) {
    const syncAriaCurrent = () => {
      const currentLink = document.querySelector(".toc-nav a:target-current");
      tocLinks.forEach((link) => {
        link.setAttribute("aria-current", link === currentLink ? "true" : "false");
      });
    };
    syncAriaCurrent();
    document.addEventListener("scrollend", syncAriaCurrent);
  } else if ("IntersectionObserver" in window) {
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            const id = entry.target.getAttribute("id");
            tocLinks.forEach((link) => {
              const isActive = link.getAttribute("href") === `#${id}`;
              link.classList.toggle("target-current", isActive);
              link.setAttribute("aria-current", String(isActive));
            });
          }
        });
      },
      { rootMargin: "-30% 0px -60% 0px", threshold: 0 }
    );
    sections.forEach((sec) => observer.observe(sec));
  }

  /* -----------------------------------------------------------------------
     5. Copy-to-Clipboard Buttons
     ----------------------------------------------------------------------- */
  document.querySelectorAll(".copy-btn").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const targetId = btn.getAttribute("data-copy-target");
      const codeEl = targetId ? document.getElementById(targetId) : null;
      const text = codeEl ? codeEl.textContent.trim() : "";
      if (!text) return;

      try {
        await navigator.clipboard.writeText(text);
      } catch {
        const ta = document.createElement("textarea");
        ta.value = text;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        ta.remove();
      }

      btn.textContent = t("copy.done");
      setTimeout(() => {
        btn.textContent = t("copy.btn");
      }, 1600);
    });
  });

  /* -----------------------------------------------------------------------
     6. Donation / Sponsor Config Hydration + Native <dialog closedby="any">
     ----------------------------------------------------------------------- */
  function hydrateSponsorCards() {
    const cfg = window.CORAL_SPONSOR_CONFIG || {};
    const ver = cfg.releaseVersion || "v2.0.0";
    const num = ver.replace(/^v/, "");
    const repo = cfg.repoSlug || "seashyne/Coral-Container-Project";

    const versionPill = document.getElementById("topbar-version");
    if (versionPill) versionPill.textContent = ver;

    const debLink = document.getElementById("dl-deb-link");
    if (debLink) {
      debLink.href = `https://github.com/${repo}/releases/download/${ver}/coral-runtime_${num}_amd64.deb`;
    }
    const tarLink = document.getElementById("dl-tar-link");
    if (tarLink) {
      tarLink.href = `https://github.com/${repo}/releases/download/${ver}/coral-${ver}-linux-x86_64.tar.gz`;
    }

    const pp = cfg.promptpay || {};
    const gh = cfg.githubSponsors || {};
    const bmc = cfg.buyMeACoffee || {};

    setBadge("badge-promptpay", !!pp.configured);
    setBadge("badge-github", !!gh.configured);
    setBadge("badge-bmc", !!bmc.configured);

    setText("meta-promptpay", pp.promptpayId || "");
    setText("meta-github", gh.handle || "");
    setText("meta-bmc", bmc.handle || "");
    setText("pp-account-name", pp.accountName || "");
    setText("pp-id-text", pp.promptpayId || "");

    const ghLink = document.getElementById("link-github-sponsor");
    if (ghLink && gh.url) ghLink.href = gh.url;

    const bmcLink = document.getElementById("link-bmc-sponsor");
    if (bmcLink && bmc.url) bmcLink.href = bmc.url;

    const qrFrame = document.getElementById("promptpay-qr-frame");
    if (qrFrame && pp.qrImage) {
      qrFrame.innerHTML = "";
      const img = document.createElement("img");
      img.src = pp.qrImage;
      img.alt = "PromptPay QR Code";
      qrFrame.appendChild(img);
    }

    const hintEl = document.getElementById("sponsor-config-hint");
    if (hintEl) {
      hintEl.hidden = Boolean(pp.configured && gh.configured && bmc.configured);
    }
  }

  function setBadge(id, isActive) {
    const el = document.getElementById(id);
    if (!el) return;
    el.classList.toggle("active", isActive);
    el.textContent = isActive ? t("support.activeBadge") : t("support.templateBadge");
  }

  function setText(id, val) {
    const el = document.getElementById(id);
    if (el) el.textContent = val;
  }

  const promptpayDialog = document.getElementById("promptpay-dialog");
  const openPromptpayBtn = document.getElementById("open-promptpay-btn");

  if (openPromptpayBtn && promptpayDialog) {
    openPromptpayBtn.addEventListener("click", () => {
      promptpayDialog.showModal();
    });

    // Mandatory light-dismiss fallback for browsers without <dialog closedby="any"> (e.g. Safari)
    if (!("closedBy" in HTMLDialogElement.prototype)) {
      promptpayDialog.addEventListener("click", (event) => {
        if (event.target !== promptpayDialog) return;
        const rect = promptpayDialog.getBoundingClientRect();
        const isDialogContent =
          rect.top <= event.clientY &&
          event.clientY <= rect.top + rect.height &&
          rect.left <= event.clientX &&
          event.clientX <= rect.left + rect.width;
        if (isDialogContent) return;
        promptpayDialog.close();
      });
    }
  }

  applyLanguage(currentLang);
})();

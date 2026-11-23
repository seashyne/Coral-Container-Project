(() => {
  "use strict";

  // Feature-detected popover polyfill fallback for older browsers (navbar guidance)
  if (!Object.hasOwn(HTMLElement.prototype, "popover")) {
    import("https://unpkg.com/@oddbird/popover-polyfill@latest/dist/popover.min.js").catch(() => {});
  }

  const header = document.getElementById("site-header");
  const inner = document.getElementById("header-inner");
  const menu = document.getElementById("site-menu");
  if (!header || !inner || !menu) return;

  let lastCollapsedWidth = 0;

  function isMenuOpen() {
    try {
      return menu.matches(":popover-open") || menu.classList.contains(":popover-open");
    } catch {
      return menu.classList.contains(":popover-open");
    }
  }

  function syncNavbarLayout(forceMeasure = false) {
    const headerWidth = header.clientWidth;
    const currentlyWide = header.classList.contains("is-wide");

    // Avoid thrashing when collapsed and shrinking further
    if (!currentlyWide && !forceMeasure && headerWidth <= lastCollapsedWidth) {
      return;
    }

    const wasOpen = !currentlyWide && isMenuOpen();
    header.classList.add("is-measuring");
    header.classList.add("is-wide");
    menu.removeAttribute("popover");

    const fitsInline = inner.scrollWidth <= inner.clientWidth + 1;

    if (fitsInline) {
      header.classList.add("is-wide");
      menu.removeAttribute("popover");
      lastCollapsedWidth = 0;
    } else {
      header.classList.remove("is-wide");
      menu.setAttribute("popover", "auto");
      lastCollapsedWidth = headerWidth;
      if (wasOpen && typeof menu.showPopover === "function") {
        try {
          menu.showPopover();
        } catch {}
      }
    }

    requestAnimationFrame(() => {
      header.classList.remove("is-measuring");
    });
  }

  // Close mobile popover cleanly when selecting an anchor link
  menu.addEventListener("click", (e) => {
    const link = e.target.closest("a");
    if (link && !header.classList.contains("is-wide") && typeof menu.hidePopover === "function") {
      try {
        menu.hidePopover();
      } catch {}
    }
  });

  window.coralSyncNavbar = () => syncNavbarLayout(true);

  if ("ResizeObserver" in window) {
    const ro = new ResizeObserver(() => syncNavbarLayout(false));
    ro.observe(header);
    ro.observe(menu);
  } else {
    window.addEventListener("resize", () => syncNavbarLayout(true));
  }

  window.addEventListener("load", () => syncNavbarLayout(true));
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(() => syncNavbarLayout(true));
  }

  syncNavbarLayout(true);
})();

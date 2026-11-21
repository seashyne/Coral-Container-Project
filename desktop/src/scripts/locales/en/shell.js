// Topbar, sidebar, statusbar, search and shared actions.
window.CORAL_DESKTOP_LOCALES = window.CORAL_DESKTOP_LOCALES || {};
window.CORAL_DESKTOP_LOCALES.en = window.CORAL_DESKTOP_LOCALES.en || {};
Object.assign(window.CORAL_DESKTOP_LOCALES.en, {
  // Chrome & Topbar
  "app.title": "Coral Desktop",
  "app.name": "Coral",
  "topbar.toggleSidebar": "Toggle sidebar (Ctrl+B)",
  "topbar.toggleSidebarAria": "Toggle sidebar",
  "topbar.toggleTheme": "Switch light / dark theme",
  "topbar.toggleThemeAria": "Toggle theme",
  "topbar.themeMenu": "Theme & Appearance",
  "topbar.themeShort": "Theme",
  "topbar.clearSearch": "Clear search",
  "topbar.langTitle": "Switch language (EN / ไทย)",
  "topbar.langAria": "Switch language",
  "topbar.settings": "Settings",
  "topbar.searchShortcut": "Ctrl K",
  "topbar.globalSearchAria": "Global search",

  // Theme Popover
  "themePop.title": "Appearance & Theme",
  "themePop.allSettings": "All settings",
  "themePop.modeAuto": "System",

  // Sidebar Navigation & Engine Card
  "nav.workloads": "Workloads",
  "nav.containers": "Containers",
  "nav.images": "Images",
  "nav.volumes": "Volumes",
  "nav.advanced": "Advanced",
  "nav.nextgen": "Sandboxes & VMs",
  "nav.pods": "Pods",
  "nav.application": "Application",
  "nav.settings": "Settings",
  "engine.running": "Engine running",
  "engine.stopped": "Engine stopped",
  "engine.connecting": "Connecting…",
  "engine.clickConnect": "Click Connect to start",
  "engine.runningMeta": "{count} running",
  "engine.connect": "Connect",
  "engine.disconnect": "Disconnect",
  "engine.connectedBadge": "Connected ({mode})",
  "engine.disconnectedBadge": "Disconnected",
  "engine.connectPromptFirst": "Connect to Coral Engine first",

  // Statusbar
  "status.defaultLeft": "Coral Desktop",
  "status.criLeft": "Coral v{version}",
  "status.connectedMode": "Connected · {mode}",
  "status.pulling": "Pulling {names}…",
  "status.summary": "{running} running · {images} images",

  // Search Placeholders
  "search.containers": "Search containers, stacks, images…",
  "search.images": "Search installed images & catalog…",
  "search.volumes": "Search volumes & mounts…",
  "search.nextgen": "Filter presets & tools…",
  "search.pods": "Search pod sandboxes…",
  "search.default": "Search…",

  // Common Actions & States
  "common.cancel": "Cancel",
  "common.close": "Close dialog",
  "common.refresh": "Refresh",
  "common.start": "Start",
  "common.stop": "Stop",
  "common.remove": "Remove",
  "common.run": "Run",
  "common.pull": "Pull",
  "common.update": "Update",
  "common.create": "Create",
  "common.recreate": "Recreate",
  "common.pullAndRun": "Pull & Run",
  "common.installed": "Installed",
  "common.available": "Available",
  "common.pulling": "Pulling…",
  "common.unused": "Unused",
  "common.ready": "Ready",
  "common.none": "none",
  "common.selected": "selected",
  "common.clearSelection": "Clear selection",

  "state.running": "running",
  "state.exited": "exited",
  "state.created": "created",
  "state.unknown": "unknown",
  "state.ready": "ready",
  "state.notready": "not ready",
  "health.healthy": "healthy",
  "health.unhealthy": "unhealthy",
  "health.starting": "starting",

});

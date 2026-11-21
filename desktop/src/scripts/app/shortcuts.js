// Global click and keyboard shortcut handlers.
document.addEventListener("click", (e) => {
  if (!e.target.closest("#row-menu") && !e.target.closest("button[data-menu-id]")) {
    closeRowMenu();
  }
});

document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    $("search").focus();
    $("search").select();
  } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "b") {
    e.preventDefault();
    toggleRail();
  } else if (e.key === "Escape") {
    if (!$("row-menu").classList.contains("hidden")) closeRowMenu();
    else if (state.detailId) closeDetail();
    else if (document.activeElement === $("search")) {
      $("search").value = "";
      state.search = "";
      syncSearchClearButton();
      $("search").blur();
      rerenderActiveSearch();
    }
  }
});

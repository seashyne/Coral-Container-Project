// Images view event bindings: catalog, installed images, pull and registry forms.
// ---------- Images View Events ----------

const selectImagesTab = setupTabGroup("images-tablist", (tabKey) => {
  state.imagesTab = tabKey;
});

$("add-image-btn").addEventListener("click", () => {
  openDialog("image-dialog");
  $("oci-image-ref").focus();
});
$("refresh-images").addEventListener("click", () => loadImages(false));

$("base-catalog").addEventListener("click", async (e) => {
  const pullBtn = e.target.closest("button[data-base-pull]");
  if (pullBtn) {
    pullOciImage(pullBtn.dataset.basePull);
    return;
  }
  const pullRunBtn = e.target.closest("button[data-base-pull-run]");
  if (pullRunBtn) {
    const item = BASE_IMAGES.find((b) => b.id === pullRunBtn.dataset.basePullRun);
    if (item) {
      if (QUICK_STARTERS.some((q) => q.id === item.id)) {
        launchQuickStarter(item.id);
      } else {
        openRunModalFromCatalog(item);
      }
    }
    return;
  }
  const runBtn = e.target.closest("button[data-base-run]");
  if (runBtn) {
    const item = BASE_IMAGES.find((b) => b.id === runBtn.dataset.baseRun);
    if (item) {
      openRunModalFromCatalog(item, runBtn.dataset.runTag || item.ref);
    }
  }
});

$("images-table").addEventListener("click", (e) => {
  const switchBtn = e.target.closest("[data-images-switch]");
  if (switchBtn) {
    selectImagesTab(switchBtn.dataset.imagesSwitch);
    return;
  }
  const remBtn = e.target.closest("button[data-img-remove]");
  if (remBtn) {
    removeImage(remBtn.dataset.imgRemove);
    return;
  }
  const expBtn = e.target.closest("button[data-img-export]");
  if (expBtn) {
    exportImageArchive(expBtn.dataset.imgExport);
    return;
  }
  const secBtn = e.target.closest("button[data-img-sec]");
  if (secBtn) {
    $("sec-image").value = secBtn.dataset.imgSec;
    switchView("nextgen");
    selectNextgenTab("sec");
    return;
  }
  const runEl = e.target.closest("[data-img-run]");
  if (runEl) {
    const tag = runEl.dataset.imgRun;
    const base = BASE_IMAGES.find((b) => b.matchTags.includes(tag));
    if (base) {
      openRunModalFromCatalog(base, tag);
    } else {
      openRunModal({ slug: tag, image: tag });
    }
  }
});

const selectImageModalTab = setupTabGroup("image-modal-tablist", (tabKey) => {
  state.imageModalTab = tabKey;
});

$("discover-pull-form")?.addEventListener("submit", async (e) => {
  e.preventDefault();
  const ref = $("discover-pull-input").value.trim();
  if (!ref) return;
  $("discover-pull-submit").disabled = true;
  try {
    await pullOciImage(ref);
    $("discover-pull-input").value = "";
  } finally {
    $("discover-pull-submit").disabled = false;
  }
});

$("oci-pull-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const ref = $("oci-image-ref").value.trim();
  const platform = $("oci-pull-platform")?.value.trim() || null;
  if (!ref) return;
  $("oci-pull-submit").disabled = true;
  closeDialog("image-dialog");
  $("oci-image-ref").value = "";
  try {
    await pullOciImage(ref, platform);
  } finally {
    $("oci-pull-submit").disabled = false;
  }
});

$("local-rootfs-form").addEventListener("submit", registerLocalImage);
$("dockerfile-build-form").addEventListener("submit", buildDockerfileImage);
$("archive-load-form").addEventListener("submit", loadImageArchive);
$("img-tag-form").addEventListener("submit", submitTagImage);
$("reg-login-form").addEventListener("submit", submitRegistryLogin);
$("reg-push-form").addEventListener("submit", submitPushImage);

// Images view: Local Images table + Quick Pull Catalog tabs, Export tar.zst, Dockerfile build,
// offline rootfs import, and Registry Login & Push.

function findInstalledBase(baseItem) {
  return state.images.find((img) =>
    img.repoTags.some((tag) => baseItem.matchTags.includes(tag)),
  );
}

function updatePullStatusBanner() {
  const banner = $("pull-status");
  if (!banner) return;
  if (state.pulling.size === 0) {
    banner.classList.add("hidden");
    banner.innerHTML = "";
  } else {
    const list = Array.from(state.pulling).map((r) => `<code>${esc(r)}</code>`).join(", ");
    banner.classList.remove("hidden");
    banner.innerHTML = `<span class="spinner" aria-hidden="true"></span><span>${t("images.pullingBanner", { refs: list })}</span>`;
  }
  updateChrome();
}

function renderBaseCatalog() {
  const el = $("base-catalog");
  if (!el) return;
  const q = state.view === "images" ? state.search.trim().toLowerCase() : "";

  const items = BASE_IMAGES.filter((b) => {
    if (!q) return true;
    const desc = t(b.descKey).toLowerCase();
    return (
      b.name.toLowerCase().includes(q) ||
      b.ref.toLowerCase().includes(q) ||
      b.pkgManager.toLowerCase().includes(q) ||
      b.libc.toLowerCase().includes(q) ||
      desc.includes(q)
    );
  });

  if (!items.length) {
    el.innerHTML = `<p class="muted">${esc(t("images.noCatalogMatch", { query: state.search }))}</p>`;
    return;
  }

  el.innerHTML = items
    .map((b) => {
      const installed = findInstalledBase(b);
      const isPulling = state.pulling.has(b.ref);
      const badgeClass = isPulling ? "pulling" : installed ? "installed" : "";
      const badgeText = isPulling
        ? t("common.pulling")
        : installed
          ? t("common.installed")
          : t("common.available");
      const installedTag = installed
        ? installed.repoTags.find((tTag) => b.matchTags.includes(tTag)) ||
          installed.repoTags[0] ||
          b.ref
        : b.ref;
      const sizeText = installed ? t("images.onDisk", { size: fmtBytes(installed.size) }) : b.approxSize;
      const pullLabel = b.isScratch
        ? installed
          ? t("common.recreate")
          : t("common.create")
        : installed
          ? t("common.update")
          : t("common.pull");

      return `
        <article class="base-card ${installed ? "installed" : ""}">
          <header>
            <span class="tile ${b.distroClass}">${esc(b.monogram)}</span>
            <div class="base-head-text">
              <div class="base-head-top">
                <h3>${esc(b.name)}</h3>
                <span class="base-badge ${badgeClass}">${esc(badgeText)}</span>
              </div>
              <span class="base-ref" title="${esc(b.ref)}">${esc(b.ref)}</span>
            </div>
          </header>
          <div class="base-card-body">
            <p class="base-desc">${esc(t(b.descKey))}</p>
          </div>
          <footer>
            <span class="base-size-meta">${esc(sizeText)}</span>
            ${
              installed && !b.isScratch
                ? `<button type="button" class="btn primary sm" data-base-run="${esc(b.id)}" data-run-tag="${esc(installedTag)}" ${!state.connected ? "disabled" : ""}>${esc(t("common.run"))}</button>`
                : !b.isScratch
                  ? `<button type="button" class="btn sm" data-base-pull-run="${esc(b.id)}" ${!state.connected || isPulling ? "disabled" : ""}>${esc(t("common.pullAndRun"))}</button>`
                  : ""
            }
            <button
              type="button"
              class="btn ${installed && !b.isScratch ? "" : "primary"} sm"
              data-base-pull="${esc(b.ref)}"
              ${!state.connected || isPulling ? "disabled" : ""}
              title="${esc(state.connected ? `${t("common.pull")} ${b.ref}` : t("engine.connectPromptFirst"))}"
            >${esc(isPulling ? t("common.pulling") : pullLabel)}</button>
          </footer>
        </article>`;
    })
    .join("");
}

async function loadImages(quiet) {
  try {
    const rows = await invoke("list_images");
    state.images = rows;
    $("image-list").innerHTML = rows
      .flatMap((i) => i.repoTags)
      .map((tag) => `<option value="${esc(tag)}">`)
      .join("");
    const json = JSON.stringify(rows);
    if (json !== state.lastImages || state.view === "images") {
      state.lastImages = json;
      renderBaseCatalog();
      renderInstalledImages(rows);
    }
  } catch (e) {
    if (!quiet) toast(String(e), true);
  }
}

function renderInstalledImages(rows) {
  const countEl = $("installed-images-count");
  if (countEl) {
    countEl.textContent = state.connected
      ? t("images.countLabel", { count: rows.length })
      : t("engine.disconnectedBadge");
  }

  if (!state.connected) {
    $("images-table").innerHTML = emptyStateHtml(
      "image",
      t("images.disconnectedTitle"),
      t("images.disconnectedSub"),
    );
    return;
  }

  const q = state.view === "images" ? state.search.trim().toLowerCase() : "";
  const filtered = rows.filter((i) => {
    if (!q) return true;
    return (
      i.repoTags.some((tag) => tag.toLowerCase().includes(q)) ||
      i.rootfsPath.toLowerCase().includes(q) ||
      i.id.toLowerCase().includes(q)
    );
  });

  if (!filtered.length) {
    $("images-table").innerHTML = emptyStateHtml(
      "image",
      rows.length ? t("images.noMatchTitle") : t("images.emptyTitle"),
      rows.length ? t("images.noMatchSub") : t("images.emptySub"),
      `<button type="button" class="btn primary" data-images-switch="discover">${esc(t("images.openCatalog"))}</button>`,
    );
    return;
  }

  const body = filtered
    .map((i) => {
      const primaryTag = i.repoTags[0] || i.id;
      const tagsLabel = i.repoTags.join(", ") || i.id;
      const baseMatch = BASE_IMAGES.find((b) =>
        i.repoTags.some((tag) => b.matchTags.includes(tag)),
      );
      const monogram = baseMatch ? baseMatch.monogram : primaryTag.slice(0, 2).toUpperCase();
      const tileClass = baseMatch ? `tile ${baseMatch.distroClass}` : "tile cmp";
      return `
        <tr class="row">
          <td data-label="${esc(t("images.colImage"))}">
            <div class="name">
              <span class="${tileClass}">${esc(monogram)}</span>
              <div>
                <button type="button" class="nm-btn mono" data-img-run="${esc(primaryTag)}" title="${esc(i.rootfsPath || i.id)}">${esc(tagsLabel)}</button>
              </div>
            </div>
          </td>
          <td class="r mono" data-label="${esc(t("images.colSize"))}">${esc(fmtBytes(i.size))}</td>
          <td class="actions">
            <div class="acts-row">
              <button type="button" class="btn ic go" data-img-run="${esc(primaryTag)}" title="${esc(t("images.actRun"))}" aria-label="${esc(t("images.actRun"))}">${ICONS.play}</button>
              <button type="button" class="btn ic" data-img-export="${esc(primaryTag)}" title="${esc(t("images.actExport"))}" aria-label="${esc(t("images.actExport"))}">${ICONS.download}</button>
              <button type="button" class="btn ic" data-img-sec="${esc(primaryTag)}" title="${esc(t("images.actSignSbom"))}" aria-label="${esc(t("images.actSignSbom"))}">${ICONS.shield}</button>
              <button type="button" class="btn ic danger" data-img-remove="${esc(primaryTag)}" title="${esc(t("images.actRemove"))}" aria-label="${esc(t("images.actRemove"))}">${ICONS.trash}</button>
            </div>
          </td>
        </tr>`;
    })
    .join("");

  $("images-table").innerHTML = `
    <div class="responsive-table-wrap" tabindex="0" role="region" aria-labelledby="imgtab-local">
      <table>
        <caption class="visually-hidden">${esc(t("images.localHeading"))}</caption>
        <thead>
          <tr>
            <th scope="col">${esc(t("images.colImage"))}</th>
            <th scope="col" class="r">${esc(t("images.colSize"))}</th>
            <th scope="col" class="r">${esc(t("images.colActions"))}</th>
          </tr>
        </thead>
        <tbody>${body}</tbody>
      </table>
    </div>`;
}

async function pullOciImage(ref, platform = null) {
  const imageRef = ref.trim();
  if (!imageRef) return;
  if (!state.connected) {
    toast(t("engine.connectPromptFirst"), true);
    return;
  }
  if (state.pulling.has(imageRef)) return;
  state.pulling.add(imageRef);
  updatePullStatusBanner();
  renderBaseCatalog();
  try {
    await invoke("pull_image", { name: imageRef, rootfs: null, platform });
    toast(
      imageRef === "scratch"
        ? t("toast.pulledScratch")
        : t("toast.pulledImage", { image: imageRef }),
    );
    await loadImages(false);
    updateChrome();
  } catch (e) {
    toast(String(e), true);
    if (String(e).includes("not connected")) setConnected(false);
    throw e;
  } finally {
    state.pulling.delete(imageRef);
    updatePullStatusBanner();
    renderBaseCatalog();
  }
}

async function exportImageArchive(imageRef) {
  const safeName = imageRef.replace(/[^a-zA-Z0-9._-]/g, "_") + ".tar.zst";
  const outputPath = await dialog.save({
    defaultPath: safeName,
    filters: [{ name: "OCI Tar Archive", extensions: ["tar.zst", "tar.gz", "tar"] }],
  });
  if (!outputPath) return;
  await guard(async () => {
    const out = await invoke("save_image", { image: imageRef, outputPath });
    toast(t("toast.exportedImage", { image: imageRef, path: out }));
  });
}

async function registerLocalImage(e) {
  e.preventDefault();
  const name = $("img-name").value.trim();
  const rootfs = $("img-rootfs").value.trim();
  if (!name || !rootfs) {
    toast("Provide both an image name and a rootfs path", true);
    return;
  }
  $("img-register").disabled = true;
  try {
    await guard(async () => {
      await invoke("pull_image", { name, rootfs });
      toast(t("toast.registeredLocal", { name }));
      $("img-name").value = "";
      $("img-rootfs").value = "";
      closeDialog("image-dialog");
      await loadImages(false);
      updateChrome();
    });
  } finally {
    $("img-register").disabled = false;
  }
}

async function buildDockerfileImage(e) {
  e.preventDefault();
  const name = $("build-tag").value.trim();
  const dockerfile = $("build-dockerfile").value;
  const contextDir = $("build-context").value.trim() || null;
  const noCache = Boolean($("build-no-cache")?.checked);
  const platform = $("build-platform")?.value.trim() || null;
  if (!name || !dockerfile.trim()) {
    toast("Provide both an image tag and Dockerfile instructions", true);
    return;
  }
  $("build-submit").disabled = true;
  try {
    await guard(async () => {
      await invoke("build_image", { name, dockerfile, contextDir, noCache, platform });
      toast(t("toast.builtImage", { name }));
      $("build-tag").value = "";
      closeDialog("image-dialog");
      await loadImages(false);
      updateChrome();
    });
  } finally {
    $("build-submit").disabled = false;
  }
}

async function loadImageArchive(e) {
  e.preventDefault();
  const inputPath = $("load-archive-path").value.trim();
  const tag = $("load-archive-tag").value.trim() || null;
  if (!inputPath) {
    toast("Select or enter an archive path (.tar, .tar.gz, .tar.zst)", true);
    return;
  }
  $("load-archive-submit").disabled = true;
  try {
    await guard(async () => {
      const loaded = await invoke("load_image", { inputPath, tag });
      toast(t("toast.loadedArchive", { name: loaded }));
      $("load-archive-path").value = "";
      $("load-archive-tag").value = "";
      closeDialog("image-dialog");
      await loadImages(false);
      updateChrome();
    });
  } finally {
    $("load-archive-submit").disabled = false;
  }
}

async function submitTagImage(e) {
  e.preventDefault();
  const source = $("tag-source-img").value.trim();
  const target = $("tag-target-img").value.trim();
  if (!source || !target) {
    toast("Enter both source image and new tag alias", true);
    return;
  }
  $("tag-submit").disabled = true;
  try {
    await guard(async () => {
      await invoke("tag_image", { source, target });
      toast(t("toast.taggedImage", { source, target }));
      $("tag-target-img").value = "";
      await loadImages(false);
      updateChrome();
    });
  } finally {
    $("tag-submit").disabled = false;
  }
}

async function removeImage(name) {
  await guard(async () => {
    await invoke("remove_image", { name });
    toast(t("toast.removedImage", { name }));
    await loadImages(false);
    updateChrome();
  });
}

async function submitRegistryLogin(e) {
  e.preventDefault();
  const registry = $("reg-host").value.trim() || "docker.io";
  const username = $("reg-user").value.trim();
  const password = $("reg-pass").value;
  if (!username || !password) {
    toast("Enter username and password/token", true);
    return;
  }
  $("reg-login-submit").disabled = true;
  try {
    await guard(async () => {
      const key = await invoke("registry_login", { registry, username, password });
      $("reg-pass").value = "";
      toast(t("toast.loggedIn", { host: key, user: username }));
    });
  } finally {
    $("reg-login-submit").disabled = false;
  }
}

async function submitPushImage(e) {
  e.preventDefault();
  const image = $("push-image-ref").value.trim();
  if (!image) return;
  $("push-submit").disabled = true;
  try {
    await guard(async () => {
      const res = await invoke("push_image", { image });
      toast(
        t("toast.pushedImage", {
          image: res.image,
          digest: res.manifestDigest.slice(0, 19),
        }),
      );
    });
  } finally {
    $("push-submit").disabled = false;
  }
}

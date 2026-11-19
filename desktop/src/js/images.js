// Images view: Linux Base Images Catalog cards, Installed Images table, OCI Registry v2 pull,
// offline local rootfs registration, and Dockerfile image builder.

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
    banner.innerHTML = `<span class="spinner" aria-hidden="true"></span><span>Pulling ${list} from OCI registry and extracting Linux rootfs…</span>`;
  }
  updateChrome();
}

function renderBaseCatalog() {
  const el = $("base-catalog");
  if (!el) return;
  const q = state.view === "images" ? state.search.trim().toLowerCase() : "";

  const items = BASE_IMAGES.filter((b) => {
    if (!q) return true;
    return (
      b.name.toLowerCase().includes(q) ||
      b.ref.toLowerCase().includes(q) ||
      b.pkgManager.toLowerCase().includes(q) ||
      b.libc.toLowerCase().includes(q) ||
      b.bestFor.toLowerCase().includes(q)
    );
  });

  if (!items.length) {
    el.innerHTML = `<p class="muted">No base images match "${esc(state.search)}".</p>`;
    return;
  }

  el.innerHTML = items
    .map((b) => {
      const installed = findInstalledBase(b);
      const isPulling = state.pulling.has(b.ref);
      const badgeClass = isPulling ? "pulling" : installed ? "installed" : "";
      const badgeText = isPulling ? "Pulling…" : installed ? "Installed" : "Available";
      const installedTag = installed
        ? installed.repoTags.find((t) => b.matchTags.includes(t)) || installed.repoTags[0] || b.ref
        : b.ref;
      const sizeText = installed
        ? `${fmtBytes(installed.size)} on disk`
        : b.approxSize;
      const pullLabel = b.isScratch
        ? installed ? "Recreate" : "Create"
        : installed ? "Re-pull" : "Pull";

      return `
        <article class="base-card ${installed ? "installed" : ""}">
          <header>
            <span class="tile ${b.distroClass}">${esc(b.monogram)}</span>
            <div class="base-head-text">
              <div class="base-head-top">
                <h4>${esc(b.name)}</h4>
                <span class="base-badge ${badgeClass}">${badgeText}</span>
              </div>
              <span class="base-ref" title="${esc(b.ref)}">${esc(b.ref)}</span>
            </div>
          </header>
          <div class="base-card-body">
            <p class="base-desc">${esc(b.bestFor)}</p>
            <div class="base-specs">
              <span class="spec-chip">Size <b>${esc(b.approxSize)}</b></span>
              <span class="spec-chip">Pkg <b>${esc(b.pkgManager)}</b></span>
              <span class="spec-chip">C lib <b>${esc(b.libc)}</b></span>
            </div>
          </div>
          <footer>
            <span class="base-size-meta">${esc(sizeText)}</span>
            ${
              installed && !b.isScratch
                ? `<button type="button" class="btn primary sm" data-base-run="${esc(b.id)}" data-run-tag="${esc(installedTag)}" ${!state.connected ? "disabled" : ""}>Run</button>`
                : ""
            }
            <button
              type="button"
              class="btn ${installed && !b.isScratch ? "" : "primary"} sm"
              data-base-pull="${esc(b.ref)}"
              ${!state.connected || isPulling ? "disabled" : ""}
              title="${state.connected ? `Pull ${b.ref}` : "Connect to coral-daemon first"}"
            >${isPulling ? "Pulling…" : pullLabel}</button>
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
      .map((t) => `<option value="${esc(t)}">`)
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
    countEl.textContent = state.connected ? `${rows.length} installed` : "Disconnected";
  }

  if (!state.connected) {
    $("images-table").innerHTML = emptyStateHtml(
      "image",
      "Not connected",
      "Connect to coral-daemon to pull Linux base images or inspect installed rootfs.",
    );
    return;
  }

  const q = state.view === "images" ? state.search.trim().toLowerCase() : "";
  const filtered = rows.filter((i) => {
    if (!q) return true;
    return (
      i.repoTags.some((t) => t.toLowerCase().includes(q)) ||
      i.rootfsPath.toLowerCase().includes(q) ||
      i.id.toLowerCase().includes(q)
    );
  });

  if (!filtered.length) {
    $("images-table").innerHTML = emptyStateHtml(
      "image",
      rows.length ? "No matching installed images" : "No images installed yet",
      rows.length
        ? "Try clearing your search query."
        : "Click Pull on any Linux Base Image above, or use Pull / Add image.",
    );
    return;
  }

  const body = filtered
    .map((i) => {
      const primaryTag = i.repoTags[0] || i.id;
      const tagsLabel = i.repoTags.join(", ") || i.id;
      const baseMatch = BASE_IMAGES.find((b) =>
        i.repoTags.some((t) => b.matchTags.includes(t)),
      );
      const monogram = baseMatch ? baseMatch.monogram : primaryTag.slice(0, 2).toUpperCase();
      const tileClass = baseMatch ? `tile ${baseMatch.distroClass}` : "tile cmp";
      return `
        <tr class="row">
          <td data-label="Image">
            <div class="name">
              <span class="${tileClass}">${esc(monogram)}</span>
              <div>
                <div class="nm mono" data-img-run="${esc(primaryTag)}">${esc(tagsLabel)}</div>
                <div class="sub-name mono" title="${esc(i.id)}">${esc(shortId(i.id))}</div>
              </div>
            </div>
          </td>
          <td class="r mono" data-label="Size">${esc(fmtBytes(i.size))}</td>
          <td class="mono muted path-cell" data-label="Rootfs" title="${esc(i.rootfsPath)}">${esc(i.rootfsPath)}</td>
          <td class="actions">
            <div class="acts-row">
              <button type="button" class="btn ic go" data-img-run="${esc(primaryTag)}" title="Run container from image" aria-label="Run container">${ICONS.play}</button>
              <button type="button" class="btn ic danger" data-img-remove="${esc(primaryTag)}" title="Remove image" aria-label="Remove image">${ICONS.trash}</button>
            </div>
          </td>
        </tr>`;
    })
    .join("");

  $("images-table").innerHTML = `
    <table>
      <thead>
        <tr>
          <th>Image / Tags</th>
          <th class="r">Size</th>
          <th>Rootfs path</th>
          <th class="r">Actions</th>
        </tr>
      </thead>
      <tbody>${body}</tbody>
    </table>`;
}

async function pullOciImage(ref) {
  const imageRef = ref.trim();
  if (!imageRef) return;
  if (!state.connected) {
    toast("Connect to coral-daemon first", true);
    return;
  }
  if (state.pulling.has(imageRef)) return;

  state.pulling.add(imageRef);
  updatePullStatusBanner();
  renderBaseCatalog();

  try {
    await invoke("pull_image", { name: imageRef, rootfs: null });
    toast(imageRef === "scratch" ? "Created scratch base image" : `Pulled ${imageRef}`);
    await loadImages(false);
    updateChrome();
  } catch (e) {
    toast(String(e), true);
    if (String(e).includes("not connected")) setConnected(false);
  } finally {
    state.pulling.delete(imageRef);
    updatePullStatusBanner();
    renderBaseCatalog();
  }
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
      toast(`Registered local image ${name}`);
      $("img-name").value = "";
      $("img-rootfs").value = "";
      $("image-modal").classList.add("hidden");
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
  if (!name || !dockerfile.trim()) {
    toast("Provide both an image tag and Dockerfile instructions", true);
    return;
  }
  $("build-submit").disabled = true;
  try {
    await guard(async () => {
      await invoke("build_image", { name, dockerfile, contextDir, noCache });
      toast(`Built image ${name} from Dockerfile`);
      $("build-tag").value = "";
      $("image-modal").classList.add("hidden");
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
    toast("Enter an archive path (.tar, .tar.gz, .tar.zst)", true);
    return;
  }
  $("load-archive-submit").disabled = true;
  try {
    await guard(async () => {
      const loaded = await invoke("load_image", { inputPath, tag });
      toast(`Loaded image ${loaded}`);
      $("load-archive-path").value = "";
      $("load-archive-tag").value = "";
      $("image-modal").classList.add("hidden");
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
      toast(`Tagged ${source} -> ${target}`);
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
    toast(`Removed image ${name}`);
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
      toast(`Logged in to ${key} as ${username}`);
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
      toast(`Pushed ${res.image} (${res.manifestDigest.slice(0, 19)}…)`);
    });
  } finally {
    $("push-submit").disabled = false;
  }
}

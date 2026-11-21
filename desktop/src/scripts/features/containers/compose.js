// Containers view: Compose stack modal and compose up/down actions.
function openComposeModal() {
  openDialog("compose-dialog");
}

async function browseComposeFile(openModalAfter = false) {
  const file = await dialog.open({
    multiple: false,
    filters: [{ name: "Compose YAML", extensions: ["yml", "yaml"] }],
  });
  if (!file) return;
  await guard(async () => {
    const yaml = await invoke("read_compose_file", { path: file });
    const parts = file.replace(/\\/g, "/").split("/");
    parts.pop();
    const dir = parts.join("/") || "/";
    const folderName = (parts[parts.length - 1] || "my-stack")
      .toLowerCase()
      .replace(/[^a-z0-9_-]/g, "-");
    $("compose-yaml").value = yaml;
    $("compose-basedir").value = dir;
    $("compose-project").value = folderName;
    if (openModalAfter) openComposeModal();
    toast(t("toast.loadedFile", { path: file }));
  });
}

async function submitComposeUp() {
  const yaml = $("compose-yaml").value;
  const projectName = $("compose-project").value.trim() || null;
  const baseDir = $("compose-basedir").value.trim() || null;
  if (!yaml.trim()) {
    toast(t("toast.enterCompose"), true);
    return;
  }
  $("compose-up-btn").disabled = true;
  try {
    await guard(async () => {
      const res = await invoke("compose_up", { yaml, projectName, baseDir });
      closeDialog("compose-dialog");
      toast(t("toast.stackStarted", { res }));
      await refreshAll(false);
    });
  } finally {
    $("compose-up-btn").disabled = false;
  }
}

async function submitComposeDown() {
  const projectName = $("compose-project").value.trim();
  if (!projectName) {
    toast(t("toast.enterComposeName"), true);
    return;
  }
  $("compose-down-btn").disabled = true;
  try {
    await guard(async () => {
      const removed = await invoke("compose_down", { projectName });
      closeDialog("compose-dialog");
      toast(t("toast.stackRemoved", { name: projectName, count: removed }));
      await refreshAll(false);
    });
  } finally {
    $("compose-down-btn").disabled = false;
  }
}

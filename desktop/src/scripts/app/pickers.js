// Native file and directory pickers.
$("img-pick").addEventListener("click", async () => {
  const dir = await dialog.open({ directory: true });
  if (dir) $("img-rootfs").value = dir;
});
document.querySelectorAll(".pick-dir").forEach((b) =>
  b.addEventListener("click", async () => {
    const dir = await dialog.open({ directory: true });
    if (dir) $(b.dataset.target).value = dir;
  }),
);
document.querySelectorAll(".pick").forEach((b) =>
  b.addEventListener("click", async () => {
    const file = await dialog.open({ multiple: false });
    if (file) $(b.dataset.target).value = file;
  }),
);

// Pods and volumes view event bindings.
// ---------- Pods & Volumes View Events ----------

$("refresh-pods").addEventListener("click", () => loadPods(false));
$("pods-table").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-pod-act]");
  if (btn) podAction(btn.dataset.podAct, btn.dataset.id);
});

$("refresh-volumes").addEventListener("click", () => loadVolumes(false));
$("prune-btn").addEventListener("click", openPruneModal);
$("prune-submit").addEventListener("click", submitSystemPrune);
$("vol-create-form").addEventListener("submit", createVolumeSubmit);
$("volumes-table").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-vol-remove]");
  if (btn) removeVolume(btn.dataset.volRemove);
});

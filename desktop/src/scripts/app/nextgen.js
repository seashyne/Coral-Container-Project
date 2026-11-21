// Next-gen runtimes view event bindings (KVM, WASI, sandboxes, signing).
// ---------- Next-Gen Runtimes View Events ----------

const selectNextgenTab = setupTabGroup("nextgen-tablist", (tabKey) => {
  state.nextgenTab = tabKey;
});

document.querySelectorAll("[data-sbx-preset]").forEach((btn) =>
  btn.addEventListener("click", () => applySandboxPreset(btn.dataset.sbxPreset)),
);
document.querySelectorAll("[data-wasm-tpl]").forEach((btn) =>
  btn.addEventListener("click", () => applyWasmTemplate(btn.dataset.wasmTpl)),
);

$("kvm-probe-btn").addEventListener("click", probeKvmHardware);
$("wasm-run-form").addEventListener("submit", submitWasmPlayground);
$("sandbox-form").addEventListener("submit", submitAiSandbox);
$("vm-setup-kernel-btn").addEventListener("click", setupMicroVmKernel);
$("vm-boot-form").addEventListener("submit", submitVmBoot);
$("sec-sign-btn").addEventListener("click", () => runImageSecurityAction("sign"));
$("sec-verify-btn").addEventListener("click", () => runImageSecurityAction("verify"));
$("sec-sbom-btn").addEventListener("click", () => runImageSecurityAction("sbom"));
$("wasm-serve-target").addEventListener("input", updateWasmServePreview);
$("wasm-serve-addr").addEventListener("input", updateWasmServePreview);
$("wasm-serve-copy-btn").addEventListener("click", copyWasmServeCmd);

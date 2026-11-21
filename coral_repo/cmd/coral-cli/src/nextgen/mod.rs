//! CLI handlers for Coral v2.0 Next-Gen workloads: `coral wasm`, `coral vm`, and `coral sandbox`.

mod sandbox;
mod signing;
mod vm;
mod wasm;

pub use sandbox::cmd_sandbox_run;
pub use signing::{cmd_keygen, cmd_sbom, cmd_sign, cmd_verify};
pub use vm::{cmd_vm_boot, cmd_vm_disk_create, cmd_vm_probe, cmd_vm_run, cmd_vm_setup_kernel};
pub use wasm::{cmd_wasm_pack, cmd_wasm_run, cmd_wasm_serve, resolve_wasm_target};

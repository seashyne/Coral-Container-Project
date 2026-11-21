//! Coral v2.0 Next-Gen runtime hooks in `CoralDaemonService`:
//! - Direct microsecond WebAssembly (WASI Preview 1) container & playground execution (`coral-wasm`)
//! - Native Linux `/dev/kvm` MicroVM hardware probe & self-test (`coral-vmm`)
//! - AI Agent / Untrusted Code hardened sandbox runner (`coral-vmm::sandbox`)

mod image_ops;
mod sandbox;
mod vmm;
mod wasm;

pub use vmm::VmmProbePayload;

//! Coral v2.0 Next-Gen runtime hooks in `CoralDaemonService`:
//! - Direct microsecond WebAssembly (WASI Preview 1) container & playground execution (`coral-wasm`)
//! - Native Linux `/dev/kvm` MicroVM hardware probe & self-test (`coral-vmm`)
//! - AI Agent / Untrusted Code hardened sandbox runner (`coral-vmm::sandbox`)

mod audit;
mod image_ops;
mod sandbox;
mod vmm;
mod wasm;

pub use audit::{AuditCheck, SecurityAuditReport, constant_time_eq, run_host_security_audit};
pub use vmm::VmmProbePayload;

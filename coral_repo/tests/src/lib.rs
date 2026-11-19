//! Shared test helpers for Coral end-to-end integration tests.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static CORAL_BIN: OnceLock<PathBuf> = OnceLock::new();

/// Returns the path to the workspace root (`coral_repo`).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests crate must be inside workspace root")
        .to_path_buf()
}

/// Returns the path to the unpacked Alpine Linux minirootfs used by E2E tests.
pub fn alpine_rootfs() -> PathBuf {
    workspace_root().join("rootfs")
}

/// Ensures the `coral` OCI CLI binary is built and returns its absolute path.
pub fn ensure_coral_binary() -> &'static PathBuf {
    CORAL_BIN.get_or_init(|| {
        let root = workspace_root();
        let candidate = root.join("target/debug/coral");

        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .current_dir(&root)
            .args(["build", "--bin", "coral"])
            .status()
            .expect("invoke cargo build --bin coral");
        assert!(status.success(), "failed to build `coral` binary");
        assert!(
            candidate.is_file(),
            "expected built binary at {}",
            candidate.display()
        );
        candidate
    })
}

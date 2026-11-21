//! OverlayFS copy-on-write isolation from the shared base rootfs.

use super::*;

#[test]
fn overlayfs_cow_isolates_writes_from_shared_base_rootfs() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine-cow:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // Container 1 creates /cow_only.txt inside its OverlayFS upperdir.
    let c1 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine-cow:3.20",
            "--name",
            "cow-1",
            "--",
            "/bin/sh",
            "-c",
            "echo secret > /cow_only.txt && cat /cow_only.txt",
        ])
        .output()
        .unwrap();
    assert!(
        c1.status.success(),
        "cow-1 failed: {}",
        String::from_utf8_lossy(&c1.stderr)
    );
    assert!(String::from_utf8_lossy(&c1.stdout).contains("secret"));

    // Base rootfs must remain untouched!
    assert!(
        !rootfs.join("cow_only.txt").exists(),
        "OverlayFS CoW failed: base rootfs was mutated!"
    );

    // Container 2 sharing the same base image must NOT see /cow_only.txt!
    let c2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine-cow:3.20",
            "--name",
            "cow-2",
            "--",
            "/bin/sh",
            "-c",
            "test ! -e /cow_only.txt && echo CLEAN_BASE",
        ])
        .output()
        .unwrap();
    assert!(c2.status.success());
    assert!(String::from_utf8_lossy(&c2.stdout).contains("CLEAN_BASE"));
}

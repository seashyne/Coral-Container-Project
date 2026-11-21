//! `coral diff`, `coral commit` and the image tag/save/load roundtrip.

use super::*;

#[test]
fn overlayfs_diff_and_commit_creates_new_runnable_image() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Run container with --keep that adds, modifies, and deletes files in its CoW upper layer.
    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--keep",
            "--image",
            "alpine:3.20",
            "--name",
            "cow-commit-ctr",
            "--",
            "/bin/sh",
            "-c",
            "echo COMMITTED_V1 > /etc/coral-committed.txt && echo MODIFIED > /etc/issue && rm -f /etc/motd",
        ])
        .output()
        .unwrap();
    assert!(
        run_out.status.success(),
        "cow-commit-ctr failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    // 2. Inspect `coral diff --local cow-commit-ctr`.
    let diff_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["diff", "--local", "cow-commit-ctr"])
        .output()
        .unwrap();
    assert!(diff_out.status.success());
    let diff_txt = String::from_utf8_lossy(&diff_out.stdout);
    assert!(
        diff_txt.contains("A /etc/coral-committed.txt"),
        "expected added file in diff:\n{diff_txt}"
    );
    assert!(
        diff_txt.contains("C /etc/issue"),
        "expected modified file in diff:\n{diff_txt}"
    );
    assert!(
        diff_txt.contains("D /etc/motd"),
        "expected deleted whiteout in diff:\n{diff_txt}"
    );

    // 3. Commit the container into a new image `committed-alpine:v1`.
    let commit_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["commit", "--local", "cow-commit-ctr", "committed-alpine:v1"])
        .output()
        .unwrap();
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    // 4. Run a fresh container from `committed-alpine:v1` and verify changes persisted while base rootfs stayed untouched.
    let verify_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "committed-alpine:v1",
            "--name",
            "from-commit-ctr",
            "--",
            "/bin/sh",
            "-c",
            "cat /etc/coral-committed.txt && (test ! -e /etc/motd && echo MOTD_GONE)",
        ])
        .output()
        .unwrap();
    assert!(verify_out.status.success());
    let v_stdout = String::from_utf8_lossy(&verify_out.stdout);
    assert!(v_stdout.contains("COMMITTED_V1"), "stdout:\n{v_stdout}");
    assert!(v_stdout.contains("MOTD_GONE"), "stdout:\n{v_stdout}");
    assert!(!rootfs.join("etc/coral-committed.txt").exists());
    assert!(rootfs.join("etc/motd").exists());
}

#[test]
fn image_tag_save_zstd_and_load_roundtrip_cli() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let src_images = work.path().join("src-images");
    let dst_images = work.path().join("dst-images");
    let archive_path = work.path().join("exported-alpine.tar.zst");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&src_images)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Tag image
    let tag_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&src_images)
        .args(["tag", "--local", "alpine:3.20", "my-corp/alpine:prod"])
        .output()
        .unwrap();
    assert!(
        tag_out.status.success(),
        "coral tag failed: {}",
        String::from_utf8_lossy(&tag_out.stderr)
    );

    // 2. Save image to `.tar.zst`
    let save_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&src_images)
        .args(["save", "--local", "-o"])
        .arg(&archive_path)
        .arg("my-corp/alpine:prod")
        .output()
        .unwrap();
    assert!(
        save_out.status.success(),
        "coral save failed: {}",
        String::from_utf8_lossy(&save_out.stderr)
    );
    assert!(archive_path.is_file());

    // 3. Load archive into a completely empty `dst_images` store
    let load_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&dst_images)
        .args(["load", "--local", "-i"])
        .arg(&archive_path)
        .args(["-t", "offline-alpine:v1"])
        .output()
        .unwrap();
    assert!(
        load_out.status.success(),
        "coral load failed: {}",
        String::from_utf8_lossy(&load_out.stderr)
    );

    // 4. Run a container from the imported image
    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&dst_images)
        .args([
            "run",
            "--image",
            "offline-alpine:v1",
            "--name",
            "loaded-ctr",
            "--",
            "/bin/cat",
            "/etc/alpine-release",
        ])
        .output()
        .unwrap();
    assert!(run_out.status.success());
    assert!(
        String::from_utf8_lossy(&run_out.stdout)
            .trim()
            .starts_with("3.20")
    );
}

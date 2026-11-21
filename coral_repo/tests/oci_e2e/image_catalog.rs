//! Base image catalog, `pull --local` and `run --image`.

use super::*;

#[test]
fn cli_catalog_pull_local_and_run_from_image() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    // 1. `coral catalog` shows all 7 Linux base images
    let cat_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["catalog", "--local"])
        .output()
        .expect("coral catalog");
    assert!(cat_out.status.success());
    let cat_str = String::from_utf8_lossy(&cat_out.stdout);
    for expected in [
        "alpine:3.20",
        "debian:bookworm-slim",
        "ubuntu:24.04",
        "busybox:musl",
        "rockylinux:9-minimal",
        "registry.access.redhat.com/ubi9/ubi-minimal:latest",
        "scratch",
    ] {
        assert!(
            cat_str.contains(expected),
            "catalog missing {expected}:\n{cat_str}"
        );
    }

    // 2. `coral pull scratch --local` + `coral images --local`
    let scratch_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "scratch", "--local"])
        .output()
        .expect("coral pull scratch");
    assert!(
        scratch_out.status.success(),
        "coral pull scratch failed: {}",
        String::from_utf8_lossy(&scratch_out.stderr)
    );

    // 3. Register local Alpine rootfs and run directly via `coral run --image`
    let reg_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "my-alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .output()
        .expect("coral pull --rootfs");
    assert!(
        reg_out.status.success(),
        "coral pull --rootfs failed: {}",
        String::from_utf8_lossy(&reg_out.stderr)
    );

    let imgs_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["images", "--local"])
        .output()
        .expect("coral images");
    assert!(imgs_out.status.success());
    let imgs_str = String::from_utf8_lossy(&imgs_out.stdout);
    assert!(imgs_str.contains("scratch"));
    assert!(imgs_str.contains("my-alpine:3.20"));

    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "my-alpine:3.20",
            "--name",
            "cli-img-demo",
            "-e",
            "GREET=coral-cli",
            "--",
            "/bin/sh",
            "-c",
            "echo RUN_OK:$GREET:$(hostname):$(cat /etc/alpine-release)",
        ])
        .output()
        .expect("coral run --image");
    let run_stdout = String::from_utf8_lossy(&run_out.stdout);
    let run_stderr = String::from_utf8_lossy(&run_out.stderr);
    assert!(
        run_out.status.success(),
        "coral run --image failed:\nstdout:\n{run_stdout}\nstderr:\n{run_stderr}"
    );
    assert!(
        run_stdout.contains("RUN_OK:coral-cli:cli-img-demo:3.20"),
        "unexpected output:\n{run_stdout}"
    );

    // 4. `coral rmi scratch --local`
    let rmi_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["rmi", "scratch", "--local"])
        .output()
        .expect("coral rmi");
    assert!(rmi_out.status.success());
}

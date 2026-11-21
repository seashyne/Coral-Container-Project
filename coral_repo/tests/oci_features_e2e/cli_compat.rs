//! Docker-compatible positional image arguments and memory units.

use super::*;

#[test]
fn docker_compatible_positional_image_and_memory_units() {
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

    // Docker-style positional `<IMAGE> <CMD...>` without `--image` or `--`,
    // paired with `--memory 64m` and `--pids-limit 64`.
    let out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--pull",
            "never",
            "--memory",
            "64m",
            "--pids-limit",
            "64",
            "--name",
            "docker-ergonomics",
            "alpine:3.20",
            "/bin/sh",
            "-c",
            "echo DOCKER_OK:PID=$$",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "positional coral run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("DOCKER_OK:PID=1"),
        "expected DOCKER_OK:PID=1, got:\n{stdout}"
    );
}

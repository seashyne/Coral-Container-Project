//! `coral build` from a Dockerfile with FROM/ENV/COPY/RUN.

use super::*;

#[test]
fn dockerfile_build_creates_custom_image_with_from_env_copy_and_run() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");
    let ctx_dir = work.path().join("build-ctx");
    std::fs::create_dir_all(&ctx_dir).unwrap();

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    std::fs::write(ctx_dir.join("greeting.txt"), "from-build-context\n").unwrap();
    std::fs::write(
        ctx_dir.join("Dockerfile"),
        r#"
FROM alpine:3.20
ARG BUILD_TAG=v1.0
ENV SERVICE_VERSION=$BUILD_TAG
WORKDIR /opt/service
COPY greeting.txt /opt/service/greeting.txt
RUN echo "version=$SERVICE_VERSION msg=$(cat /opt/service/greeting.txt)" > /opt/service/manifest.txt
"#,
    )
    .unwrap();

    let build_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v1"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(
        build_out.status.success(),
        "coral build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr)
    );

    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "coral-service:v1",
            "--name",
            "built-svc-ctr",
            "--",
            "/bin/cat",
            "/opt/service/manifest.txt",
        ])
        .output()
        .unwrap();
    assert!(run_out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "version=v1.0 msg=from-build-context"
    );

    // 2. Second build with identical context must hit step cache (`[CACHED]`).
    let build2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v1"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(build2.status.success());
    let build2_err = String::from_utf8_lossy(&build2.stderr);
    assert!(
        build2_err.contains("[CACHED]"),
        "expected second build to hit cache, stderr:\n{build2_err}"
    );

    // 3. Changing a copied context file invalidates COPY & RUN cache steps.
    std::fs::write(ctx_dir.join("greeting.txt"), "updated-context-v2\n").unwrap();
    let build3 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v2"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(build3.status.success());
    let run_v2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "coral-service:v2",
            "--name",
            "built-svc-v2",
            "--",
            "/bin/cat",
            "/opt/service/manifest.txt",
        ])
        .output()
        .unwrap();
    assert!(run_v2.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run_v2.stdout).trim(),
        "version=v1.0 msg=updated-context-v2"
    );

    // 4. Appending to $PATH inside ENV must expand the base image's existing PATH.
    let custom_bin = ctx_dir.join("hello-tool");
    std::fs::write(&custom_bin, "#!/bin/sh\necho custom-tool-ok\n").unwrap();
    std::fs::write(
        ctx_dir.join("Dockerfile"),
        r#"
FROM alpine:3.20
ENV APP=hello APP_PORT=8080
ENV PATH="$PATH:/opt/tools"
COPY hello-tool /opt/tools/hello-tool
RUN chmod +x /opt/tools/hello-tool && hello-tool > /opt/tool.out && echo "$APP:$APP_PORT" > /opt/vars.out
"#,
    )
    .unwrap();

    let build4 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v3"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(
        build4.status.success(),
        "build4 failed:\n{}",
        String::from_utf8_lossy(&build4.stderr)
    );

    let run_v3 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "coral-service:v3",
            "--name",
            "built-svc-v3",
            "--",
            "/bin/sh",
            "-c",
            "cat /opt/tool.out && cat /opt/vars.out",
        ])
        .output()
        .unwrap();
    assert!(run_v3.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run_v3.stdout).trim(),
        "custom-tool-ok\nhello:8080"
    );

    // 5. BuildKit heredoc (`RUN <<EOF`), `--mount=type=cache`, `--mount=type=secret`,
    // `EXPOSE`/`VOLUME`/`HEALTHCHECK` metadata persistence, and `--device` passthrough.
    std::fs::write(ctx_dir.join("api.secret"), "top-secret-token-99\n").unwrap();
    std::fs::write(
        ctx_dir.join("Dockerfile"),
        r#"
FROM alpine:3.20
EXPOSE 8080/tcp 5353/udp
VOLUME ["/data"]
HEALTHCHECK --interval=2s --retries=2 CMD test -f /opt/built.ok
RUN --mount=type=cache,id=e2e-cache,target=/var/cache/coral --mount=type=secret,id=api_key,src=api.secret <<EOF
echo "cached-v1" > /var/cache/coral/marker.txt
grep -q "top-secret-token-99" /run/secrets/api_key
cat /var/cache/coral/marker.txt > /opt/built.ok
EOF
"#,
    )
    .unwrap();

    let build5 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v4"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(
        build5.status.success(),
        "build5 failed:\n{}",
        String::from_utf8_lossy(&build5.stderr)
    );

    // Verify secret was NOT persisted into the final image rootfs, while `/opt/built.ok` was!
    let run_v4 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "coral-service:v4",
            "--name",
            "built-svc-v4",
            "--device",
            "/dev/null:/dev/custom-null:ro",
            "--",
            "/bin/sh",
            "-c",
            "test ! -e /run/secrets/api_key && test -c /dev/custom-null && cat /opt/built.ok",
        ])
        .output()
        .unwrap();
    assert!(
        run_v4.status.success(),
        "run_v4 failed:\n{}",
        String::from_utf8_lossy(&run_v4.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run_v4.stdout).trim(), "cached-v1");
}

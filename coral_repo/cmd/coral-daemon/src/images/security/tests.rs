use super::*;
use crate::images::{register_local_rootfs, save_index};
use std::fs;
use tempfile::tempdir;

#[test]
fn ed25519_sign_verify_and_tamper_detection_plus_spdx_sbom() {
    let dir = tempdir().unwrap();
    let images_dir = dir.path().join("images");
    let rootfs = dir.path().join("alpine-rootfs");
    fs::create_dir_all(rootfs.join("lib/apk/db")).unwrap();
    fs::write(rootfs.join("etc_release"), "3.20.3\n").unwrap();
    fs::write(
        rootfs.join("lib/apk/db/installed"),
        "P:busybox\nV:1.36.1-r29\nA:x86_64\nL:GPL-2.0-only\nT:Size optimized toolbox\n\n\
         P:musl\nV:1.2.5-r0\nA:x86_64\nL:MIT\nT:the musl c library\n\n",
    )
    .unwrap();

    let stored = register_local_rootfs("demo-signed:2.1", &rootfs).unwrap();
    save_index(&images_dir, &[stored]).unwrap();

    // 1. Keygen
    let priv_path = dir.path().join("cosign.key");
    let pub_path = dir.path().join("cosign.pub");
    let kp = generate_keypair(&priv_path, &pub_path, false).unwrap();
    assert!(kp.fingerprint.starts_with("sha256:"));

    // 2. Sign & Verify OK
    let env = sign_image(&images_dir, "demo-signed:2.1", &priv_path).unwrap();
    assert_eq!(env.algorithm, "Ed25519");
    let vr = verify_image(&images_dir, "demo-signed:2.1", &pub_path).unwrap();
    assert!(vr.verified);
    assert_eq!(vr.tree_digest, env.tree_digest);

    // 3. SBOM SPDX 2.3
    let sbom = generate_spdx_sbom(&images_dir, "demo-signed:2.1").unwrap();
    assert_eq!(sbom["spdxVersion"], "SPDX-2.3");
    let pkgs = sbom["packages"].as_array().unwrap();
    assert_eq!(pkgs.len(), 2);
    assert_eq!(pkgs[0]["name"], "busybox");
    assert!(
        pkgs[0]["externalRefs"][0]["referenceLocator"]
            .as_str()
            .unwrap()
            .contains("pkg:apk/alpine/busybox@1.36.1-r29")
    );

    // 4. Tamper with 1 byte -> verify MUST reject!
    fs::write(rootfs.join("etc_release"), "3.20.4\n").unwrap();
    let err = verify_image(&images_dir, "demo-signed:2.1", &pub_path).unwrap_err();
    assert!(
        err.to_string().contains("TAMPERING DETECTED"),
        "unexpected error: {err}"
    );
}

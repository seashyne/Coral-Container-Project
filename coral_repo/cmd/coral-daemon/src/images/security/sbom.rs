//! SPDX 2.3 JSON Software Bill of Materials generation.

use super::digest::compute_image_tree_digest;
use super::util::{resolve_stored_image, sha256_hex};
use anyhow::Result;
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

/// Generates a standards-compliant SPDX 2.3 JSON Software Bill of Materials (SBOM) by
/// inspecting Alpine APK DB (`/lib/apk/db/installed`), Debian/Ubuntu DPKG DB
/// (`/var/lib/dpkg/status`), and WebAssembly artifacts (`.wasm`/`.wat`) inside `image_ref`.
pub fn generate_spdx_sbom(images_dir: &Path, image_ref: &str) -> Result<serde_json::Value> {
    let img = resolve_stored_image(images_dir, image_ref)?;
    let rootfs = PathBuf::from(&img.rootfs_path);
    let tree_digest = compute_image_tree_digest(&rootfs)?;

    let mut packages = Vec::new();
    let apk_db = rootfs.join("lib/apk/db/installed");
    if apk_db.is_file() {
        packages.extend(parse_alpine_apk_db(&fs::read_to_string(&apk_db)?));
    }
    let dpkg_db = rootfs.join("var/lib/dpkg/status");
    if dpkg_db.is_file() {
        packages.extend(parse_dpkg_status(&fs::read_to_string(&dpkg_db)?));
    }
    if let Some(wasm_mod) = coral_wasm::find_wasm_module_in_rootfs(&rootfs)
        && let Ok(bytes) = fs::read(&wasm_mod)
    {
        let fname = wasm_mod
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("module.wasm");
        packages.push(serde_json::json!({
            "SPDXID": format!("SPDXRef-wasm-{}", sanitize_spdx_id(fname)),
            "name": fname,
            "versionInfo": "wasm32-wasi",
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": false,
            "checksums": [{
                "algorithm": "SHA256",
                "checksumValue": sha256_hex(&bytes),
            }],
            "externalRefs": [{
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": format!("pkg:generic/{fname}?type=wasm")
            }]
        }));
    }

    let created = Utc::now().to_rfc3339();
    Ok(serde_json::json!({
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": format!("coral-sbom-{image_ref}"),
        "documentNamespace": format!("https://coral.local/spdx/{}/{}", sanitize_spdx_id(image_ref), &tree_digest[7..19]),
        "creationInfo": {
            "created": created,
            "creators": [format!("Tool: coral-{}", env!("CARGO_PKG_VERSION"))]
        },
        "comment": format!("Image tree digest: {tree_digest}"),
        "packages": packages
    }))
}

fn parse_alpine_apk_db(content: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for block in content.split("\n\n") {
        let mut name = "";
        let mut version = "";
        let mut license = "NOASSERTION";
        let mut arch = "x86_64";
        let mut desc = "";
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("P:") {
                name = v.trim();
            } else if let Some(v) = line.strip_prefix("V:") {
                version = v.trim();
            } else if let Some(v) = line.strip_prefix("L:") {
                license = v.trim();
            } else if let Some(v) = line.strip_prefix("A:") {
                arch = v.trim();
            } else if let Some(v) = line.strip_prefix("T:") {
                desc = v.trim();
            }
        }
        if !name.is_empty() {
            out.push(serde_json::json!({
                "SPDXID": format!("SPDXRef-apk-{}", sanitize_spdx_id(&format!("{name}-{version}"))),
                "name": name,
                "versionInfo": version,
                "licenseConcluded": license,
                "summary": desc,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": false,
                "externalRefs": [{
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": format!("pkg:apk/alpine/{name}@{version}?arch={arch}")
                }]
            }));
        }
    }
    out
}

fn parse_dpkg_status(content: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for block in content.split("\n\n") {
        let mut name = "";
        let mut version = "";
        let mut arch = "amd64";
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("Package:") {
                name = v.trim();
            } else if let Some(v) = line.strip_prefix("Version:") {
                version = v.trim();
            } else if let Some(v) = line.strip_prefix("Architecture:") {
                arch = v.trim();
            }
        }
        if !name.is_empty() {
            out.push(serde_json::json!({
                "SPDXID": format!("SPDXRef-deb-{}", sanitize_spdx_id(&format!("{name}-{version}"))),
                "name": name,
                "versionInfo": version,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": false,
                "externalRefs": [{
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": format!("pkg:deb/debian/{name}@{version}?arch={arch}")
                }]
            }));
        }
    }
    out
}

fn sanitize_spdx_id(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect()
}

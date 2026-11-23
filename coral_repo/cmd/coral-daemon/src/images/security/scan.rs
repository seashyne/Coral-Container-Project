//! Built-in OCI image vulnerability / CVE scanner (`coral scan <image>`) powered by
//! SPDX package extraction + OSV.dev Batch API (`https://api.osv.dev/v1/querybatch`).

use super::sbom::generate_spdx_sbom;
use super::util::resolve_stored_image;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_OSV_BATCH_URL: &str = "https://api.osv.dev/v1/querybatch";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SeveritySummary {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub unknown: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VulnerabilityFinding {
    pub id: String,
    pub package: String,
    pub version: String,
    pub ecosystem: String,
    pub severity: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageScanReport {
    pub image: String,
    pub os_ecosystem: String,
    pub packages_scanned: usize,
    pub source: String,
    pub summary: SeveritySummary,
    pub findings: Vec<VulnerabilityFinding>,
}

impl ImageScanReport {
    /// Returns `true` when any finding meets or exceeds `fail_on` (`critical|high|medium|low`).
    pub fn exceeds_threshold(&self, fail_on: &str) -> bool {
        match fail_on.trim().to_ascii_lowercase().as_str() {
            "critical" => self.summary.critical > 0,
            "high" => self.summary.critical > 0 || self.summary.high > 0,
            "medium" => self.summary.critical > 0 || self.summary.high > 0 || self.summary.medium > 0,
            "low" => self.summary.total > 0,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
struct ScannedPkg {
    name: String,
    version: String,
    ecosystem: String,
}

/// Scans `image_ref` inside `images_dir` for known CVEs using OSV.dev (or offline mode).
pub async fn scan_image_vulnerabilities(
    images_dir: &Path,
    image_ref: &str,
    offline: bool,
    osv_endpoint: Option<&str>,
) -> Result<ImageScanReport> {
    let img = resolve_stored_image(images_dir, image_ref)?;
    let rootfs = PathBuf::from(&img.rootfs_path);
    let os_eco = detect_os_ecosystem(&rootfs);
    let sbom = generate_spdx_sbom(images_dir, image_ref)?;

    let mut pkgs = Vec::new();
    if let Some(arr) = sbom.get("packages").and_then(|v| v.as_array()) {
        for p in arr {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let ver = p.get("versionInfo").and_then(|v| v.as_str()).unwrap_or("").trim();
            if name.is_empty() || ver.is_empty() || ver == "wasm32-wasi" {
                continue;
            }
            let purl = p
                .get("externalRefs")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.get("referenceLocator"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let eco = if purl.starts_with("pkg:apk/") {
                os_eco.clone().unwrap_or_else(|| "Alpine".into())
            } else if purl.starts_with("pkg:deb/") {
                os_eco.clone().unwrap_or_else(|| "Debian".into())
            } else {
                continue;
            };
            pkgs.push(ScannedPkg {
                name: name.to_string(),
                version: ver.to_string(),
                ecosystem: eco,
            });
        }
    }

    let eco_label = os_eco.unwrap_or_else(|| "generic".into());
    if offline || pkgs.is_empty() {
        return Ok(ImageScanReport {
            image: image_ref.to_string(),
            os_ecosystem: eco_label,
            packages_scanned: pkgs.len(),
            source: if offline { "offline".into() } else { "osv.dev".into() },
            summary: SeveritySummary::default(),
            findings: Vec::new(),
        });
    }

    let endpoint = osv_endpoint.unwrap_or(DEFAULT_OSV_BATCH_URL);
    let findings = query_osv_batch(endpoint, &pkgs).await?;
    let mut summary = SeveritySummary::default();
    for f in &findings {
        summary.total += 1;
        match f.severity.as_str() {
            "CRITICAL" => summary.critical += 1,
            "HIGH" => summary.high += 1,
            "MEDIUM" => summary.medium += 1,
            "LOW" => summary.low += 1,
            _ => summary.unknown += 1,
        }
    }

    Ok(ImageScanReport {
        image: image_ref.to_string(),
        os_ecosystem: eco_label,
        packages_scanned: pkgs.len(),
        source: "osv.dev".into(),
        summary,
        findings,
    })
}

fn detect_os_ecosystem(rootfs: &Path) -> Option<String> {
    if let Ok(rel) = fs::read_to_string(rootfs.join("etc/alpine-release")) {
        let parts: Vec<&str> = rel.trim().split('.').collect();
        if parts.len() >= 2 {
            return Some(format!("Alpine:v{}.{}", parts[0], parts[1]));
        }
        return Some("Alpine".into());
    }
    if let Ok(osr) = fs::read_to_string(rootfs.join("etc/os-release")) {
        let mut id = "";
        let mut ver = "";
        for line in osr.lines() {
            if let Some(v) = line.strip_prefix("ID=") {
                id = v.trim().trim_matches('"');
            } else if let Some(v) = line.strip_prefix("VERSION_ID=") {
                ver = v.trim().trim_matches('"');
            }
        }
        if id.eq_ignore_ascii_case("debian") {
            return Some(if ver.is_empty() { "Debian".into() } else { format!("Debian:{ver}") });
        }
        if id.eq_ignore_ascii_case("ubuntu") {
            return Some(if ver.is_empty() { "Ubuntu".into() } else { format!("Ubuntu:{ver}") });
        }
    }
    None
}

async fn query_osv_batch(endpoint: &str, pkgs: &[ScannedPkg]) -> Result<Vec<VulnerabilityFinding>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("coral/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut findings = Vec::new();

    for chunk in pkgs.chunks(100) {
        let queries: Vec<serde_json::Value> = chunk
            .iter()
            .map(|p| {
                serde_json::json!({
                    "version": p.version,
                    "package": {
                        "name": p.name,
                        "ecosystem": p.ecosystem,
                    }
                })
            })
            .collect();
        let resp = client
            .post(endpoint)
            .json(&serde_json::json!({ "queries": queries }))
            .send()
            .await
            .with_context(|| format!("query OSV vulnerability API at {endpoint}"))?;
        let val: serde_json::Value = resp
            .error_for_status()
            .context("OSV API returned HTTP error")?
            .json()
            .await
            .context("decode OSV JSON response")?;

        if let Some(results) = val.get("results").and_then(|v| v.as_array()) {
            for (idx, item) in results.iter().enumerate() {
                let Some(pkg) = chunk.get(idx) else {
                    continue;
                };
                if let Some(vulns) = item.get("vulns").and_then(|v| v.as_array()) {
                    for v in vulns {
                        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("UNKNOWN").to_string();
                        let summary = v
                            .get("summary")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let severity = extract_severity(v);
                        findings.push(VulnerabilityFinding {
                            id,
                            package: pkg.name.clone(),
                            version: pkg.version.clone(),
                            ecosystem: pkg.ecosystem.clone(),
                            severity,
                            summary,
                        });
                    }
                }
            }
        }
    }

    Ok(findings)
}

fn extract_severity(vuln: &serde_json::Value) -> String {
    if let Some(sev) = vuln
        .get("database_specific")
        .and_then(|d| d.get("severity"))
        .and_then(|s| s.as_str())
    {
        return match sev.to_ascii_uppercase().as_str() {
            "CRITICAL" => "CRITICAL".into(),
            "HIGH" => "HIGH".into(),
            "MODERATE" | "MEDIUM" => "MEDIUM".into(),
            "LOW" => "LOW".into(),
            _ => "UNKNOWN".into(),
        };
    }
    if let Some(sev_str) = vuln.get("severity").and_then(|s| s.as_str()) {
        return sev_str.to_ascii_uppercase();
    }
    "UNKNOWN".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{StoredImage, save_index};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn scans_alpine_packages_against_osv_batch_api_and_evaluates_threshold() {
        let dir = tempdir().unwrap();
        let rootfs = dir.path().join("alpine/rootfs");
        fs::create_dir_all(rootfs.join("etc")).unwrap();
        fs::create_dir_all(rootfs.join("lib/apk/db")).unwrap();
        fs::write(rootfs.join("etc/alpine-release"), "3.20.3\n").unwrap();
        fs::write(
            rootfs.join("lib/apk/db/installed"),
            "P:openssl\nV:3.1.0-r0\nA:x86_64\n\nP:busybox\nV:1.36.1-r0\nA:x86_64\n\n",
        )
        .unwrap();
        let img = StoredImage {
            id: "sha256:scan-test".into(),
            repo_tags: vec!["alpine:3.20".into()],
            size: 1024,
            rootfs_path: rootfs.to_string_lossy().into_owned(),
            managed: true,
        };
        save_index(dir.path(), &[img]).unwrap();

        // Mock OSV batch HTTP server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let body = r#"{"results":[{"vulns":[{"id":"CVE-2024-9999","summary":"Test OpenSSL vuln","database_specific":{"severity":"HIGH"}}]},{}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });

        let report = scan_image_vulnerabilities(
            dir.path(),
            "alpine:3.20",
            false,
            Some(&format!("http://{addr}/v1/querybatch")),
        )
        .await
        .unwrap();
        let _ = srv.await;

        assert_eq!(report.os_ecosystem, "Alpine:v3.20");
        assert_eq!(report.packages_scanned, 2);
        assert_eq!(report.summary.high, 1);
        assert_eq!(report.summary.total, 1);
        assert_eq!(report.findings[0].id, "CVE-2024-9999");
        assert!(report.exceeds_threshold("high"));
        assert!(!report.exceeds_threshold("critical"));
    }
}

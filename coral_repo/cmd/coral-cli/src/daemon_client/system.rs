//! System housekeeping (`coral system df`, `coral system prune`) and full JSON
//! container inspection (`coral inspect`) over `coral-daemon` CRI v1 gRPC.

use super::{connect_channel, try_connect_channel};
use crate::catalog;
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerStatusRequest, PodSandboxConfig, PodSandboxMetadata, RunPodSandboxRequest,
};
use coral_daemon::images;
use coral_daemon::service::housekeeping::{PruneReport, SystemDiskUsage};
use coral_runtime::rootfs::{default_volumes_dir, list_named_volumes};
use std::collections::{HashMap, HashSet};
use std::path::Path;

async fn invoke_system_action(
    addr: &str,
    annotations: HashMap<String, String>,
) -> Result<String> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let resp = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "coral-system-action".into(),
                    uid: "coral-system-action".into(),
                    namespace: "coral-system".into(),
                    attempt: 0,
                }),
                hostname: "coral-system".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations,
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .context("coral system RPC")?
        .into_inner();
    Ok(resp.pod_sandbox_id)
}

pub async fn cmd_system_df(addr: &str, state_root: &Path, images_dir: &Path) -> Result<()> {
    let usage = if try_connect_channel(addr).await.is_some() {
        let json = invoke_system_action(
            addr,
            HashMap::from([("coral.system.action".into(), "df".into())]),
        )
        .await?;
        serde_json::from_str::<SystemDiskUsage>(&json).context("parse system df JSON")?
    } else {
        compute_local_df(state_root, images_dir)
    };

    println!(
        "{:<16} {:>8} {:>10} {:>14}",
        "TYPE", "TOTAL", "ACTIVE", "SIZE"
    );
    println!(
        "{:<16} {:>8} {:>10} {:>14}",
        "Images",
        usage.images_count,
        "-",
        catalog::format_bytes(usage.images_bytes)
    );
    println!(
        "{:<16} {:>8} {:>10} {:>14}",
        "Containers",
        usage.containers_count,
        usage.containers_running,
        catalog::format_bytes(usage.containers_bytes)
    );
    println!(
        "{:<16} {:>8} {:>10} {:>14}",
        "Named Volumes",
        usage.volumes_count,
        "-",
        catalog::format_bytes(usage.volumes_bytes)
    );
    println!(
        "{:<16} {:>8} {:>10} {:>14}",
        "Build Cache",
        usage.cache_entries,
        "-",
        catalog::format_bytes(usage.cache_bytes)
    );
    println!("{}", "-".repeat(52));
    println!(
        "{:<16} {:>8} {:>10} {:>14}",
        "TOTAL",
        "",
        "",
        catalog::format_bytes(usage.total_bytes)
    );
    Ok(())
}

fn compute_local_df(state_root: &Path, images_dir: &Path) -> SystemDiskUsage {
    let imgs = images::load_index(images_dir);
    let mut seen = HashSet::new();
    let mut images_bytes = 0u64;
    for img in &imgs {
        if seen.insert(img.rootfs_path.clone()) {
            images_bytes = images_bytes.saturating_add(img.size);
        }
    }
    let cache_dir = state_root.join("build-cache");
    let (cache_entries, cache_bytes) = if let Ok(rd) = std::fs::read_dir(&cache_dir) {
        let mut n = 0usize;
        let mut b = 0u64;
        for e in rd.flatten() {
            n += 1;
            let p = e.path();
            b = b.saturating_add(if p.is_dir() {
                images::dir_size(&p)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            });
        }
        (n, b)
    } else {
        (0, 0)
    };
    let vols = list_named_volumes(&default_volumes_dir()).unwrap_or_default();
    let volumes_bytes = vols
        .iter()
        .fold(0u64, |acc, (_, p)| acc.saturating_add(images::dir_size(p)));

    SystemDiskUsage {
        images_count: imgs.len(),
        images_bytes,
        cache_entries,
        cache_bytes,
        containers_count: 0,
        containers_running: 0,
        containers_exited: 0,
        containers_bytes: 0,
        volumes_count: vols.len(),
        volumes_bytes,
        total_bytes: images_bytes
            .saturating_add(cache_bytes)
            .saturating_add(volumes_bytes),
    }
}

pub async fn cmd_system_prune(addr: &str, prune_all: bool, prune_volumes: bool) -> Result<()> {
    let mut ann = HashMap::from([("coral.system.action".into(), "prune".into())]);
    if prune_all {
        ann.insert("coral.system.prune_all".into(), "true".into());
    }
    if prune_volumes {
        ann.insert("coral.system.prune_volumes".into(), "true".into());
    }
    let json = invoke_system_action(addr, ann).await?;
    let report = serde_json::from_str::<PruneReport>(&json).context("parse prune report JSON")?;
    println!("Pruned Coral daemon state:");
    println!("  Stopped containers removed: {}", report.containers_removed);
    println!("  Unused pod sandboxes:       {}", report.pods_removed);
    println!("  Build cache entries purged: {}", report.cache_entries_removed);
    if prune_volumes {
        println!("  Unused volumes removed:     {}", report.volumes_removed);
    }
    if prune_all {
        println!("  Unused images removed:      {}", report.images_removed);
    }
    println!(
        "Total reclaimed space: {}",
        catalog::format_bytes(report.bytes_reclaimed)
    );
    Ok(())
}

pub async fn cmd_inspect_via_daemon(addr: &str, id: &str) -> Result<Option<String>> {
    let Some(ch) = try_connect_channel(addr).await else {
        return Ok(None);
    };
    let mut rt = RuntimeServiceClient::new(ch);
    let Ok(resp) = rt
        .container_status(ContainerStatusRequest {
            container_id: id.to_string(),
            verbose: true,
        })
        .await
    else {
        return Ok(None);
    };
    Ok(resp.into_inner().info.get("inspectJson").cloned())
}

pub async fn cmd_login(
    addr: &str,
    registry: &str,
    username: &str,
    password: &str,
) -> Result<()> {
    // Store locally AND forward to coral-daemon if connected (so WSL2/Remote daemons also receive auth).
    let key = images::login_registry(registry, username, password)?;
    if try_connect_channel(addr).await.is_some() {
        let _ = invoke_system_action(
            addr,
            HashMap::from([
                ("coral.system.action".into(), "login".into()),
                ("coral.auth.registry".into(), registry.to_string()),
                ("coral.auth.username".into(), username.to_string()),
                ("coral.auth.password".into(), password.to_string()),
            ]),
        )
        .await;
    }
    println!("Login Succeeded for '{key}' (user: {username})");
    Ok(())
}

pub async fn cmd_logout(addr: &str, registry: &str) -> Result<()> {
    let removed = images::logout_registry(registry)?;
    if try_connect_channel(addr).await.is_some() {
        let _ = invoke_system_action(
            addr,
            HashMap::from([
                ("coral.system.action".into(), "logout".into()),
                ("coral.auth.registry".into(), registry.to_string()),
            ]),
        )
        .await;
    }
    if removed {
        println!("Removed login credentials for '{registry}'");
    } else {
        println!("No stored credentials found for '{registry}'");
    }
    Ok(())
}

pub async fn cmd_push(
    addr: &str,
    images_dir: &Path,
    image_ref: &str,
    local: bool,
) -> Result<()> {
    if !local && try_connect_channel(addr).await.is_some() {
        let json = invoke_system_action(
            addr,
            HashMap::from([
                ("coral.system.action".into(), "push".into()),
                ("coral.image.ref".into(), image_ref.to_string()),
            ]),
        )
        .await?;
        let res: images::PushResult =
            serde_json::from_str(&json).context("parse push result JSON")?;
        println!(
            "Pushed {}\n  Digest: {}\n  Layer:  {} ({})",
            res.image,
            res.manifest_digest,
            res.layer_digest,
            catalog::format_bytes(res.layer_size)
        );
        return Ok(());
    }

    let index = images::load_index(images_dir);
    let stored = images::find_image(&index, image_ref).ok_or_else(|| {
        anyhow::anyhow!(
            "image '{image_ref}' is not installed in {} — tag or build it first",
            images_dir.display()
        )
    })?;
    let res = images::push_image(image_ref, stored).await?;
    println!(
        "Pushed {}\n  Digest: {}\n  Layer:  {} ({})",
        res.image,
        res.manifest_digest,
        res.layer_digest,
        catalog::format_bytes(res.layer_size)
    );
    Ok(())
}

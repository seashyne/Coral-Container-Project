//! `coral compose watch` — watches Compose service build contexts (honoring `.dockerignore`)
//! and automatically rebuilds & restarts changed services.

use super::common::{resolve_compose_file, resolve_project_name};
use super::up::cmd_compose_up;
use anyhow::Result;
use coral_daemon::builder::dockerignore::DockerIgnore;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[allow(clippy::too_many_arguments)]
pub async fn cmd_compose_watch(
    addr: &str,
    state_root: &Path,
    images_dir: &Path,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    profiles: &[String],
    services: &[String],
    interval_ms: u64,
) -> Result<()> {
    let compose_path = resolve_compose_file(file)?;
    let spec = coral_daemon::compose::ComposeSpec::from_file_with_env(&compose_path, env_file)?;
    let project = resolve_project_name(project_override, Some(&spec), Some(&compose_path));
    let selected = spec.select_services(services, profiles, false)?;

    let watched: Vec<(String, PathBuf)> = selected
        .iter()
        .filter_map(|s| {
            s.build_context
                .as_ref()
                .map(|ctx| (s.name.clone(), PathBuf::from(ctx)))
        })
        .collect();

    if watched.is_empty() {
        println!("No services with `build:` contexts found in Compose stack '{project}'.");
        return Ok(());
    }

    let mut fingerprints: HashMap<String, u64> = HashMap::new();
    for (name, ctx) in &watched {
        fingerprints.insert(name.clone(), compute_dir_fingerprint(ctx));
        println!("Watching service '{name}' context: {}", ctx.display());
    }

    let poll = Duration::from_millis(interval_ms.max(250));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("Stopped compose watch.");
                break;
            }
            _ = tokio::time::sleep(poll) => {
                for (name, ctx) in &watched {
                    let fp = compute_dir_fingerprint(ctx);
                    if fingerprints.get(name).copied() != Some(fp) {
                        fingerprints.insert(name.clone(), fp);
                        println!("[watch] Change detected in '{name}' — rebuilding & restarting...");
                        let single = vec![name.clone()];
                        if let Err(e) = cmd_compose_up(
                            addr,
                            state_root,
                            images_dir,
                            Some(&compose_path),
                            env_file,
                            Some(&project),
                            profiles,
                            &single,
                            true,
                            true,
                            false,
                            true,
                            &[],
                        )
                        .await
                        {
                            eprintln!("[watch] Rebuild failed for '{name}': {e:#}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Computes a fast recursive mtime+size fingerprint for a build context directory,
/// skipping paths matched by `.dockerignore`.
pub(crate) fn compute_dir_fingerprint(root: &Path) -> u64 {
    let ignore = DockerIgnore::load(root);
    let mut hash = 14695981039346656037u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            let rel = p.strip_prefix(root).unwrap_or(&p);
            if ignore.is_ignored(rel) {
                continue;
            }
            if let Ok(meta) = fs::symlink_metadata(&p) {
                for b in rel.to_string_lossy().as_bytes() {
                    hash ^= u64::from(*b);
                    hash = hash.wrapping_mul(1099511628211);
                }
                if meta.is_dir() {
                    stack.push(p);
                } else {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    hash ^= meta.len() ^ mtime;
                    hash = hash.wrapping_mul(1099511628211);
                }
            }
        }
    }
    hash
}

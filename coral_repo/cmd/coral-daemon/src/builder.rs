//! Dockerfile build engine (`coral build` & Coral Desktop Image Builder).
//!
//! Supports `FROM`, `ARG`, `ENV`, `WORKDIR`, `COPY`, `ADD`, `RUN`, `CMD`, `ENTRYPOINT`,
//! `EXPOSE`, `LABEL`, `USER`, `VOLUME`, executing `RUN` steps inside isolated Coral containers.

use crate::images::{self, OciImageConfig, StoredImage};
use anyhow::{Context, Result, anyhow, bail};
use coral_oci::OciSpec;
use coral_runtime::overlay::commit_overlay_to_rootfs;
use coral_runtime::rootfs::{host_dns_mounts, safe_join_rootfs};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Options for building a container image from a `Dockerfile`.
pub struct BuildOptions<'a> {
    pub tag: &'a str,
    pub dockerfile_content: &'a str,
    pub context_dir: Option<&'a Path>,
    pub images_dir: &'a Path,
    pub state_dir: &'a Path,
    pub coral_bin: &'a Path,
    pub no_cache: bool,
}

/// Builds a new image from a `Dockerfile`, persists it into `images_dir/images.json`,
/// and returns its `StoredImage` record.
pub async fn build_image(opts: BuildOptions<'_>) -> Result<StoredImage> {
    let tag = opts.tag.trim();
    if tag.is_empty() {
        bail!("target image tag must not be empty");
    }
    let display_tag = if tag.contains(':') || tag == "scratch" {
        tag.to_string()
    } else {
        format!("{tag}:latest")
    };

    let instructions = parse_dockerfile(opts.dockerfile_content)?;
    if instructions.is_empty() {
        bail!("Dockerfile contains no instructions");
    }

    let ts = chrono::Utc::now().timestamp_millis();
    let staging_dir = opts
        .images_dir
        .join(".tmp")
        .join(format!("build-{ts}"));
    let staging_rootfs = staging_dir.join("rootfs");
    fs::create_dir_all(&staging_rootfs)
        .with_context(|| format!("create build staging rootfs '{}'", staging_rootfs.display()))?;

    let build_res = execute_build_steps(
        &instructions,
        &staging_dir,
        &staging_rootfs,
        &opts,
        ts,
    )
    .await;

    let (digest_hex, built_img_cfg) = match build_res {
        Ok(res) => res,
        Err(err) => {
            let _ = images::remove_dir_force(&staging_dir);
            return Err(err);
        }
    };

    // Atomically install the built rootfs into `<images_dir>/<slug>/rootfs`.
    let slug = format!(
        "{}-{}",
        display_tag
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            })
            .collect::<String>(),
        &digest_hex[..8]
    );

    let final_parent = opts.images_dir.join(&slug);
    let final_rootfs = final_parent.join("rootfs");
    fs::create_dir_all(&final_parent)?;
    if final_rootfs.exists() {
        let _ = images::remove_dir_force(&final_rootfs);
    }
    fs::rename(&staging_rootfs, &final_rootfs)
        .with_context(|| format!("install built rootfs to '{}'", final_rootfs.display()))?;
    let _ = images::remove_dir_force(&staging_dir);

    let abs_rootfs = fs::canonicalize(&final_rootfs)?;
    let _ = built_img_cfg.save_for_rootfs(&abs_rootfs);
    let size = images::dir_size(&abs_rootfs);
    let stored = StoredImage {
        id: format!("sha256:{digest_hex}"),
        repo_tags: vec![display_tag.clone()],
        size,
        rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
        managed: true,
    };

    let mut index = images::load_index(opts.images_dir);
    let orphaned = images::upsert_image(&mut index, stored.clone());
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(opts.images_dir, &index, &old);
    }
    images::save_index(opts.images_dir, &index)?;

    info!(tag = %display_tag, size_bytes = size, "completed Dockerfile image build");
    Ok(stored)
}

async fn execute_build_steps(
    instructions: &[(String, String)],
    staging_dir: &Path,
    staging_rootfs: &Path,
    opts: &BuildOptions<'_>,
    ts: i64,
) -> Result<(String, OciImageConfig)> {
    let mut has_from = false;
    let mut workdir = "/".to_string();
    let mut envs = vec![
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "TERM=xterm".to_string(),
    ];
    let mut img_cfg = OciImageConfig::default();
    let mut vars: HashMap<String, String> = HashMap::new();
    let cache_root = opts.images_dir.join(".build-cache");
    let mut chain_key = String::from("coral-build-v1");
    let mut cache_active = !opts.no_cache;
    let total_steps = instructions.len();

    for (step_idx, (verb, raw_arg)) in instructions.iter().enumerate() {
        let arg = expand_vars(raw_arg, &vars);
        let step_num = step_idx + 1;
        info!(step = step_num, total = total_steps, verb = %verb, arg = %arg, "building step");

        match verb.as_str() {
            "FROM" => {
                let base_ref = arg
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| anyhow!("FROM instruction requires an image reference"))?;
                eprintln!("Step {step_num}/{total_steps} : FROM {base_ref}");
                if staging_rootfs.exists() {
                    let _ = images::remove_dir_force(staging_rootfs);
                }
                fs::create_dir_all(staging_rootfs)?;

                let mut h = Sha256::new();
                h.update(b"FROM:");
                h.update(base_ref.as_bytes());
                if base_ref != "scratch" && base_ref != "scratch:latest" {
                    let base_path = resolve_or_pull_base_image(base_ref, opts.images_dir).await?;
                    let base_cfg = OciImageConfig::load_for_rootfs(&base_path);
                    envs = images::merge_env_vars(&envs, &base_cfg.env, &[]);
                    if !base_cfg.working_dir.trim().is_empty() {
                        workdir = base_cfg.working_dir.clone();
                    }
                    img_cfg = base_cfg;
                    h.update(base_path.to_string_lossy().as_bytes());
                    let empty_upper = staging_dir.join("empty-upper");
                    fs::create_dir_all(&empty_upper)?;
                    commit_overlay_to_rootfs(&base_path, &empty_upper, staging_rootfs)
                        .with_context(|| format!("copy base rootfs from '{base_ref}'"))?;
                    let _ = fs::remove_dir_all(&empty_upper);
                }
                chain_key = format!("{:x}", h.finalize());
                cache_active = !opts.no_cache;
                has_from = true;
            }
            "ARG" => {
                eprintln!("Step {step_num}/{total_steps} : ARG {arg}");
                for tok in arg.split_whitespace() {
                    if let Some((k, v)) = tok.split_once('=') {
                        vars.insert(k.trim().to_string(), unquote(v.trim()));
                    } else {
                        vars.entry(tok.trim().to_string()).or_default();
                    }
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "ENV" => {
                eprintln!("Step {step_num}/{total_steps} : ENV {arg}");
                if let Some((k, v)) = arg.split_once('=') {
                    let key = k.trim().to_string();
                    let val = unquote(v.trim());
                    vars.insert(key.clone(), val.clone());
                    set_env_var(&mut envs, &key, &val);
                } else if let Some((k, v)) = arg.split_once(char::is_whitespace) {
                    let key = k.trim().to_string();
                    let val = unquote(v.trim());
                    vars.insert(key.clone(), val.clone());
                    set_env_var(&mut envs, &key, &val);
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "WORKDIR" => {
                if !has_from {
                    bail!("WORKDIR cannot appear before FROM");
                }
                eprintln!("Step {step_num}/{total_steps} : WORKDIR {arg}");
                let dir = arg.trim();
                if dir.starts_with('/') {
                    workdir = dir.to_string();
                } else if workdir == "/" {
                    workdir = format!("/{dir}");
                } else {
                    workdir = format!("{workdir}/{dir}");
                }
                let target = safe_join_rootfs(staging_rootfs, &workdir)?;
                fs::create_dir_all(&target)?;
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "COPY" | "ADD" => {
                if !has_from {
                    bail!("{verb} cannot appear before FROM");
                }
                let ctx_dir = opts.context_dir.ok_or_else(|| {
                    anyhow!("{verb} instruction requires a build context directory")
                })?;
                let parts: Vec<&str> = arg
                    .split_whitespace()
                    .filter(|p| !p.starts_with("--"))
                    .collect();
                if parts.len() < 2 {
                    bail!("{verb} requires <src>... <dest>");
                }
                let sources = &parts[..parts.len() - 1];
                let src_hash = hash_context_sources(ctx_dir, sources)?;
                let next_key =
                    compute_step_key(&chain_key, verb, &arg, &workdir, &envs, &src_hash);
                let cache_dir = cache_root.join(&next_key).join("rootfs");

                if cache_active && cache_dir.is_dir() {
                    eprintln!("Step {step_num}/{total_steps} : {verb} {arg} [CACHED]");
                    restore_cached_rootfs(&cache_dir, staging_dir, staging_rootfs)?;
                    chain_key = next_key;
                    continue;
                }

                cache_active = false;
                eprintln!("Step {step_num}/{total_steps} : {verb} {arg}");
                let dest_raw = parts[parts.len() - 1];
                let dest_in_container = if dest_raw.starts_with('/') {
                    dest_raw.to_string()
                } else if workdir == "/" {
                    format!("/{dest_raw}")
                } else {
                    format!("{workdir}/{dest_raw}")
                };
                let dest_target = safe_join_rootfs(staging_rootfs, &dest_in_container)?;

                for &src_rel in sources {
                    let src_path = safe_join_context(ctx_dir, src_rel)?;
                    copy_context_item(&src_path, &dest_target, dest_raw.ends_with('/'))?;
                }
                if !opts.no_cache {
                    let _ = save_cached_rootfs(staging_rootfs, staging_dir, &cache_dir);
                }
                chain_key = next_key;
            }
            "RUN" => {
                if !has_from {
                    bail!("RUN cannot appear before FROM");
                }
                let next_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
                let cache_dir = cache_root.join(&next_key).join("rootfs");

                if cache_active && cache_dir.is_dir() {
                    eprintln!("Step {step_num}/{total_steps} : RUN {arg} [CACHED]");
                    restore_cached_rootfs(&cache_dir, staging_dir, staging_rootfs)?;
                    chain_key = next_key;
                    continue;
                }

                cache_active = false;
                eprintln!("Step {step_num}/{total_steps} : RUN {arg}");
                let cmd_args = if arg.trim_start().starts_with('[')
                    && let Ok(parsed) = serde_json::from_str::<Vec<String>>(arg.trim())
                    && !parsed.is_empty()
                {
                    parsed
                } else {
                    vec!["/bin/sh".into(), "-c".into(), arg.clone()]
                };

                let step_bundle = staging_dir.join(format!("step-{step_idx}"));
                fs::create_dir_all(&step_bundle)?;
                let host_uid = unsafe { libc::geteuid() };
                let host_gid = unsafe { libc::getegid() };
                let mut spec = OciSpec::default_spec(host_uid != 0, host_uid, host_gid);
                if let Some(r) = spec.root.as_mut() {
                    r.path = fs::canonicalize(staging_rootfs)?
                        .to_string_lossy()
                        .into_owned();
                }
                if let Some(p) = spec.process.as_mut() {
                    p.args = cmd_args;
                    p.cwd = workdir.clone();
                    p.env = envs.clone();
                }
                spec.mounts.extend(host_dns_mounts());
                spec.annotations.insert("coral.overlay".into(), "false".into());
                spec.to_file(step_bundle.join("config.json"))?;

                let build_state_root = opts.state_dir.join("build-state");
                let ctr_id = format!("build-{ts}-{step_idx}");
                let out = Command::new(opts.coral_bin)
                    .arg("--root")
                    .arg(&build_state_root)
                    .arg("run")
                    .arg("--bundle")
                    .arg(&step_bundle)
                    .arg(&ctr_id)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .await
                    .context("spawn coral for Dockerfile RUN step")?;
                let _ = fs::remove_dir_all(&step_bundle);

                if !out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    bail!(
                        "Dockerfile RUN step failed (`RUN {arg}`, exit {:?}):\n{}{}",
                        out.status.code(),
                        stdout,
                        stderr
                    );
                }

                if !opts.no_cache {
                    let _ = save_cached_rootfs(staging_rootfs, staging_dir, &cache_dir);
                }
                chain_key = next_key;
            }
            "CMD" => {
                eprintln!("Step {step_num}/{total_steps} : CMD {arg}");
                img_cfg.cmd = parse_exec_or_shell_instruction(&arg);
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "ENTRYPOINT" => {
                eprintln!("Step {step_num}/{total_steps} : ENTRYPOINT {arg}");
                img_cfg.entrypoint = parse_exec_or_shell_instruction(&arg);
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "USER" => {
                eprintln!("Step {step_num}/{total_steps} : USER {arg}");
                img_cfg.user = unquote(arg.trim());
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "STOPSIGNAL" => {
                eprintln!("Step {step_num}/{total_steps} : STOPSIGNAL {arg}");
                img_cfg.stop_signal = unquote(arg.trim());
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "LABEL" => {
                eprintln!("Step {step_num}/{total_steps} : LABEL {arg}");
                if let Some((k, v)) = arg.split_once('=') {
                    img_cfg
                        .labels
                        .insert(unquote(k.trim()), unquote(v.trim()));
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "EXPOSE" | "VOLUME" | "SHELL" | "HEALTHCHECK" | "MAINTAINER" => {
                eprintln!("Step {step_num}/{total_steps} : {verb} {arg}");
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            other => {
                bail!("unsupported Dockerfile instruction '{other}'");
            }
        }
    }

    if !has_from {
        bail!("Dockerfile is missing a FROM instruction");
    }
    img_cfg.env = envs;
    img_cfg.working_dir = workdir;
    Ok((chain_key, img_cfg))
}

fn parse_exec_or_shell_instruction(arg: &str) -> Vec<String> {
    let trimmed = arg.trim();
    if trimmed.starts_with('[')
        && let Ok(parsed) = serde_json::from_str::<Vec<String>>(trimmed)
    {
        return parsed;
    }
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec!["/bin/sh".into(), "-c".into(), trimmed.to_string()]
    }
}

fn compute_step_key(
    parent_key: &str,
    verb: &str,
    arg: &str,
    workdir: &str,
    envs: &[String],
    extra: &[u8],
) -> String {
    let mut h = Sha256::new();
    h.update(parent_key.as_bytes());
    h.update(b"|");
    h.update(verb.as_bytes());
    h.update(b"|");
    h.update(arg.as_bytes());
    h.update(b"|");
    h.update(workdir.as_bytes());
    for e in envs {
        h.update(b"|");
        h.update(e.as_bytes());
    }
    if !extra.is_empty() {
        h.update(b"|");
        h.update(extra);
    }
    format!("{:x}", h.finalize())
}

fn hash_context_sources(ctx_dir: &Path, sources: &[&str]) -> Result<Vec<u8>> {
    let mut h = Sha256::new();
    for &src_rel in sources {
        let src_path = safe_join_context(ctx_dir, src_rel)?;
        if !src_path.exists() {
            bail!("source '{src_rel}' not found in build context");
        }
        hash_path_recursive(&src_path, &src_path, &mut h)?;
    }
    Ok(h.finalize().to_vec())
}

fn hash_path_recursive(root: &Path, cur: &Path, h: &mut Sha256) -> Result<()> {
    let rel = cur.strip_prefix(root).unwrap_or(cur);
    h.update(rel.to_string_lossy().as_bytes());
    let meta = fs::symlink_metadata(cur)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(cur)?;
        h.update(target.to_string_lossy().as_bytes());
    } else if meta.is_file() {
        let bytes = fs::read(cur)?;
        h.update(&bytes);
    } else if meta.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(cur)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            hash_path_recursive(root, &entry.path(), h)?;
        }
    }
    Ok(())
}

fn save_cached_rootfs(staging_rootfs: &Path, staging_dir: &Path, cache_dir: &Path) -> Result<()> {
    if cache_dir.exists() {
        let _ = images::remove_dir_force(cache_dir);
    }
    fs::create_dir_all(cache_dir)?;
    let empty_upper = staging_dir.join("empty-cache-upper");
    fs::create_dir_all(&empty_upper)?;
    let res = commit_overlay_to_rootfs(staging_rootfs, &empty_upper, cache_dir);
    let _ = fs::remove_dir_all(&empty_upper);
    res.map_err(Into::into)
}

fn restore_cached_rootfs(
    cache_dir: &Path,
    staging_dir: &Path,
    staging_rootfs: &Path,
) -> Result<()> {
    if staging_rootfs.exists() {
        let _ = images::remove_dir_force(staging_rootfs);
    }
    fs::create_dir_all(staging_rootfs)?;
    let empty_upper = staging_dir.join("empty-cache-upper");
    fs::create_dir_all(&empty_upper)?;
    let res = commit_overlay_to_rootfs(cache_dir, &empty_upper, staging_rootfs);
    let _ = fs::remove_dir_all(&empty_upper);
    res.map_err(Into::into)
}

async fn resolve_or_pull_base_image(base_ref: &str, images_dir: &Path) -> Result<PathBuf> {
    let index = images::load_index(images_dir);
    if let Some(found) = images::find_image(&index, base_ref) {
        let p = PathBuf::from(&found.rootfs_path);
        if p.is_dir() {
            return Ok(p);
        }
    }
    if Path::new(base_ref).is_dir() {
        return Ok(fs::canonicalize(base_ref)?);
    }
    let pulled = images::pull_image(base_ref, images_dir).await?;
    let path = PathBuf::from(&pulled.rootfs_path);
    let mut index = images::load_index(images_dir);
    let orphaned = images::upsert_image(&mut index, pulled);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    let _ = images::save_index(images_dir, &index);
    Ok(path)
}

fn parse_dockerfile(raw: &str) -> Result<Vec<(String, String)>> {
    let mut joined_lines = Vec::new();
    let mut current = String::new();

    for line in raw.lines() {
        let trimmed_start = line.trim_start();
        if current.is_empty() && (trimmed_start.is_empty() || trimmed_start.starts_with('#')) {
            continue;
        }
        if trimmed_start.starts_with('#') {
            continue;
        }
        let trimmed_end = line.trim_end();
        if let Some(without_slash) = trimmed_end.strip_suffix('\\') {
            current.push_str(without_slash.trim());
            current.push(' ');
        } else {
            current.push_str(line.trim());
            let full = std::mem::take(&mut current);
            if !full.is_empty() {
                joined_lines.push(full);
            }
        }
    }
    if !current.trim().is_empty() {
        joined_lines.push(current.trim().to_string());
    }

    let mut out = Vec::new();
    for line in joined_lines {
        let (verb, rest) = line
            .split_once(char::is_whitespace)
            .ok_or_else(|| anyhow!("malformed Dockerfile line: '{line}'"))?;
        out.push((verb.trim().to_ascii_uppercase(), rest.trim().to_string()));
    }
    Ok(out)
}

fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut out = input.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
        out = out.replace(&format!("${k}"), v);
    }
    out
}

fn unquote(s: &str) -> String {
    if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
        && s.len() >= 2
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn set_env_var(envs: &mut Vec<String>, key: &str, val: &str) {
    let prefix = format!("{key}=");
    envs.retain(|e| !e.starts_with(&prefix));
    envs.push(format!("{key}={val}"));
}

fn safe_join_context(ctx_dir: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = fs::canonicalize(ctx_dir)
        .with_context(|| format!("resolve build context '{}'", ctx_dir.display()))?;
    for comp in Path::new(rel).components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(p) => out.push(p),
            Component::ParentDir | Component::Prefix(_) => {
                bail!("unsafe path '{rel}' outside build context");
            }
        }
    }
    Ok(out)
}

fn copy_context_item(src: &Path, dst: &Path, force_dir: bool) -> Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        let empty = Path::new("/nonexistent-upper");
        commit_overlay_to_rootfs(src, empty, dst)?;
        Ok(())
    } else {
        let target_file = if force_dir || dst.is_dir() {
            fs::create_dir_all(dst)?;
            dst.join(
                src.file_name()
                    .ok_or_else(|| anyhow!("invalid source filename"))?,
            )
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            dst.to_path_buf()
        };
        fs::copy(src, &target_file)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_dockerfile_with_comments() {
        let df = r#"
            # Base image
            FROM alpine:3.20
            ARG APP_ENV=prod
            ENV MODE=$APP_ENV
            WORKDIR /app
            RUN echo hello \
                && echo world
            COPY . /app
        "#;
        let parsed = parse_dockerfile(df).unwrap();
        assert_eq!(parsed.len(), 6);
        assert_eq!(parsed[0], ("FROM".into(), "alpine:3.20".into()));
        assert_eq!(parsed[4].0, "RUN");
        assert!(parsed[4].1.contains("echo hello && echo world"));
    }
}

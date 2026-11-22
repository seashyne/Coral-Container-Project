//! BuildKit Dockerfile extensions: Heredoc (`RUN <<EOF`), `RUN --mount=type=cache|secret`,
//! and OCI image config metadata instructions (`EXPOSE`, `VOLUME`, `SHELL`, `HEALTHCHECK`, `ONBUILD`).

use super::context::safe_join_context;
use super::dockerfile::{parse_exec_or_shell_instruction, split_shell_tokens, unquote};
use crate::images::OciImageConfig;
use anyhow::{Context, Result, anyhow, bail};
use coral_oci::{Mount, OciSpec};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunMountKind {
    Cache { id: String, target: String },
    Secret { id: String, target: String, src: Option<String> },
    Tmpfs { target: String },
}

/// Folds BuildKit heredoc blocks (`RUN <<EOF ... EOF`, `RUN <<-EOF ... EOF`, `RUN --mount=... <<'EOF'`)
/// into standard single-instruction lines before line-continuation joining.
pub(super) fn preprocess_heredocs(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut lines = raw.lines();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some((prefix, delim)) = detect_heredoc_header(trimmed) {
            let mut body_cmds = Vec::new();
            for body_line in lines.by_ref() {
                if body_line.trim() == delim {
                    break;
                }
                let bl = body_line.trim();
                if !bl.is_empty() && !bl.starts_with('#') {
                    body_cmds.push(bl);
                }
            }
            let joined = if body_cmds.is_empty() {
                "true".to_string()
            } else {
                body_cmds.join(" && ")
            };
            out.push_str(prefix.trim_end());
            out.push(' ');
            out.push_str(&joined);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn detect_heredoc_header(trimmed: &str) -> Option<(&str, String)> {
    let upper = trimmed.to_ascii_uppercase();
    if !upper.starts_with("RUN ") && !upper.starts_with("RUN\t") {
        return None;
    }
    let idx = trimmed.find("<<")?;
    let prefix = &trimmed[..idx];
    let after = trimmed[idx + 2..].trim_start_matches('-').trim();
    let delim = unquote(after);
    if delim.is_empty()
        || !delim
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((prefix, delim))
}

/// Extracts leading `--mount=...` flags from a `RUN` argument string, returning the parsed
/// mount specs and the remaining command text.
pub(super) fn parse_run_mounts(arg: &str) -> Result<(Vec<RunMountKind>, String)> {
    let mut rest = arg.trim_start();
    let mut mounts = Vec::new();

    while let Some(after_flag) = rest.strip_prefix("--mount=") {
        let end = after_flag
            .find(char::is_whitespace)
            .unwrap_or(after_flag.len());
        let spec_str = &after_flag[..end];
        mounts.push(parse_single_run_mount(spec_str)?);
        rest = after_flag[end..].trim_start();
    }

    Ok((mounts, rest.to_string()))
}

fn parse_single_run_mount(spec: &str) -> Result<RunMountKind> {
    let mut mtype = "bind".to_string();
    let mut target = String::new();
    let mut id = String::new();
    let mut src: Option<String> = None;

    for kv in spec.split(',') {
        let kv = kv.trim();
        if kv.is_empty() {
            continue;
        }
        if let Some((k, v)) = kv.split_once('=') {
            match k.trim() {
                "type" => mtype = v.trim().to_ascii_lowercase(),
                "target" | "dst" | "destination" => target = unquote(v.trim()),
                "id" => id = unquote(v.trim()),
                "src" | "source" => src = Some(unquote(v.trim())),
                _ => {}
            }
        }
    }

    match mtype.as_str() {
        "cache" => {
            if target.is_empty() {
                bail!("RUN --mount=type=cache requires target=<path>");
            }
            if id.is_empty() {
                id = target
                    .trim_matches('/')
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                if id.is_empty() {
                    id = "default-cache".into();
                }
            }
            Ok(RunMountKind::Cache { id, target })
        }
        "secret" => {
            if id.is_empty() {
                id = src
                    .as_deref()
                    .and_then(|s| Path::new(s).file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("secret")
                    .to_string();
            }
            if target.is_empty() {
                target = format!("/run/secrets/{id}");
            }
            Ok(RunMountKind::Secret { id, target, src })
        }
        "tmpfs" => {
            if target.is_empty() {
                bail!("RUN --mount=type=tmpfs requires target=<path>");
            }
            Ok(RunMountKind::Tmpfs { target })
        }
        other => bail!("unsupported RUN --mount type '{other}' (supported: cache, secret, tmpfs)"),
    }
}

/// Provisions host directories/files for `RUN --mount=...` specs and attaches them to the
/// temporary OCI container `spec` for the `RUN` step.
pub(super) fn attach_run_mounts_to_spec(
    mounts: &[RunMountKind],
    cache_root: &Path,
    context_dir: Option<&Path>,
    staging_rootfs: &Path,
    spec: &mut OciSpec,
) -> Result<()> {
    for m in mounts {
        match m {
            RunMountKind::Cache { id, target } => {
                let safe_id: String = id
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect();
                let host_cache_dir = cache_root.join("mounts").join(safe_id);
                fs::create_dir_all(&host_cache_dir).with_context(|| {
                    format!("create build cache mount dir '{}'", host_cache_dir.display())
                })?;
                let abs_cache = fs::canonicalize(&host_cache_dir)?;
                let in_rootfs = coral_runtime::rootfs::safe_join_rootfs(staging_rootfs, target)?;
                fs::create_dir_all(&in_rootfs)?;
                spec.mounts.push(Mount {
                    destination: target.clone(),
                    typ: Some("bind".into()),
                    source: Some(abs_cache.to_string_lossy().into_owned()),
                    options: vec!["bind".into(), "rw".into()],
                });
            }
            RunMountKind::Secret { id, target, src } => {
                let ctx = context_dir.ok_or_else(|| {
                    anyhow!("RUN --mount=type=secret requires a build context directory")
                })?;
                let rel = src.as_deref().unwrap_or(id.as_str());
                let host_secret = safe_join_context(ctx, rel)?;
                if !host_secret.is_file() {
                    bail!(
                        "secret source '{}' (id='{id}') not found in build context",
                        rel
                    );
                }
                let abs_secret = fs::canonicalize(&host_secret)?;
                spec.mounts.push(Mount {
                    destination: target.clone(),
                    typ: Some("bind".into()),
                    source: Some(abs_secret.to_string_lossy().into_owned()),
                    options: vec!["bind".into(), "ro".into()],
                });
            }
            RunMountKind::Tmpfs { target } => {
                let in_rootfs = coral_runtime::rootfs::safe_join_rootfs(staging_rootfs, target)?;
                fs::create_dir_all(&in_rootfs)?;
                spec.mounts.push(Mount {
                    destination: target.clone(),
                    typ: Some("tmpfs".into()),
                    source: Some("tmpfs".into()),
                    options: vec!["nosuid".into(), "nodev".into(), "mode=1777".into()],
                });
            }
        }
    }
    Ok(())
}

/// Applies `EXPOSE`, `VOLUME`, `SHELL`, `HEALTHCHECK`, and `ONBUILD` instructions to `img_cfg`.
pub(super) fn apply_metadata_instruction(verb: &str, arg: &str, img_cfg: &mut OciImageConfig) {
    match verb {
        "EXPOSE" => {
            for tok in split_shell_tokens(arg) {
                let t = tok.trim();
                if t.is_empty() {
                    continue;
                }
                let norm = if t.contains('/') {
                    t.to_ascii_lowercase()
                } else {
                    format!("{t}/tcp")
                };
                if !img_cfg.exposed_ports.contains(&norm) {
                    img_cfg.exposed_ports.push(norm);
                }
            }
        }
        "VOLUME" => {
            let trimmed = arg.trim();
            let items: Vec<String> = if trimmed.starts_with('[')
                && let Ok(parsed) = serde_json::from_str::<Vec<String>>(trimmed)
            {
                parsed
            } else {
                split_shell_tokens(trimmed)
            };
            for v in items {
                let clean = unquote(v.trim());
                if !clean.is_empty() && !img_cfg.volumes.contains(&clean) {
                    img_cfg.volumes.push(clean);
                }
            }
        }
        "SHELL" => {
            let parsed = parse_exec_or_shell_instruction(arg);
            if !parsed.is_empty() {
                img_cfg.shell = parsed;
            }
        }
        "HEALTHCHECK" => parse_healthcheck_instruction(arg, img_cfg),
        "ONBUILD" => {
            let sub = arg.trim();
            if !sub.is_empty() {
                img_cfg.onbuild.push(sub.to_string());
            }
        }
        _ => {}
    }
}

fn parse_healthcheck_instruction(arg: &str, img_cfg: &mut OciImageConfig) {
    let trimmed = arg.trim();
    if trimmed.eq_ignore_ascii_case("NONE") {
        img_cfg.healthcheck_cmd = None;
        return;
    }
    let mut rest = trimmed;
    let mut interval_ms = 5_000u64;
    let mut retries = 3u32;
    while rest.starts_with("--") {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let flag = &rest[..end];
        if let Some(val) = flag.strip_prefix("--interval=") {
            interval_ms = parse_duration_ms(val).unwrap_or(5_000);
        } else if let Some(val) = flag.strip_prefix("--retries=")
            && let Ok(r) = val.parse::<u32>()
        {
            retries = r.max(1);
        }
        rest = rest[end..].trim_start();
    }
    if let Some(cmd_body) = rest.strip_prefix("CMD").or_else(|| rest.strip_prefix("cmd")) {
        let cmd_trimmed = cmd_body.trim();
        let final_cmd = if cmd_trimmed.starts_with('[')
            && let Ok(arr) = serde_json::from_str::<Vec<String>>(cmd_trimmed)
        {
            arr.join(" ")
        } else {
            cmd_trimmed.to_string()
        };
        if !final_cmd.is_empty() {
            img_cfg.healthcheck_cmd = Some(final_cmd);
            img_cfg.healthcheck_interval_ms = interval_ms;
            img_cfg.healthcheck_retries = retries;
        }
    }
}

fn parse_duration_ms(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        ms.parse::<u64>().ok()
    } else if let Some(sec) = s.strip_suffix('s') {
        sec.parse::<u64>().ok().map(|v| v.saturating_mul(1_000))
    } else if let Some(min) = s.strip_suffix('m') {
        min.parse::<u64>().ok().map(|v| v.saturating_mul(60_000))
    } else {
        s.parse::<u64>().ok().map(|v| v.saturating_mul(1_000))
    }
}

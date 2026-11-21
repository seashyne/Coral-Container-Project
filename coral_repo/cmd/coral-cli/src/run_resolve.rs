//! Target resolution for `coral run` supporting both OCI bundle syntax (`--bundle <DIR> <ID>`)
//! and Docker-compatible positional syntax (`coral run [OPTIONS] <IMAGE> [CMD...]`).

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedRunTarget {
    Bundle {
        bundle_dir: PathBuf,
        container_id: String,
    },
    Image {
        image_ref: String,
        explicit_id: Option<String>,
        cmd: Vec<String>,
    },
}

/// Resolves whether `coral run` targets an OCI bundle (`config.json`) or an OCI image,
/// handling both explicit `--bundle`/`--image` flags and Docker-style positional `<IMAGE> [CMD...]`.
pub fn resolve_run_target<F>(
    bundle: Option<PathBuf>,
    image: Option<String>,
    name: Option<String>,
    target: Option<String>,
    mut cmd: Vec<String>,
    has_local_bundle_config: bool,
    is_installed_image: F,
) -> Result<ResolvedRunTarget>
where
    F: Fn(&str) -> bool,
{
    if let Some(bundle_dir) = bundle {
        let container_id = target.or(name).ok_or_else(|| {
            anyhow::anyhow!(
                "container ID is required in OCI bundle mode (e.g. `coral run --bundle . my-box`)"
            )
        })?;
        return Ok(ResolvedRunTarget::Bundle {
            bundle_dir,
            container_id,
        });
    }

    if let Some(image_ref) = image {
        let mut full_cmd = Vec::new();
        let explicit_id = match (name, target) {
            (Some(n), Some(pos)) => {
                full_cmd.push(pos);
                full_cmd.extend(cmd);
                Some(n)
            }
            (Some(n), None) => {
                full_cmd = cmd;
                Some(n)
            }
            (None, Some(pos)) => {
                let treat_as_cmd = pos.starts_with('/')
                    || pos.starts_with("./")
                    || (!cmd.is_empty() && !cmd[0].starts_with('/'));
                if treat_as_cmd {
                    full_cmd.push(pos);
                    full_cmd.extend(cmd);
                    None
                } else {
                    full_cmd = cmd;
                    Some(pos)
                }
            }
            (None, None) => {
                full_cmd = cmd;
                None
            }
        };
        return Ok(ResolvedRunTarget::Image {
            image_ref,
            explicit_id,
            cmd: full_cmd,
        });
    }

    let Some(t) = target else {
        bail!(
            "image or --bundle is required (e.g. `coral run alpine:3.20 /bin/sh` or `coral run --bundle . my-box`)"
        );
    };

    let looks_like_bundle_id = has_local_bundle_config
        && cmd.is_empty()
        && name.is_none()
        && !t.contains(':')
        && !t.contains('/')
        && !Path::new(&t).is_dir()
        && !is_installed_image(&t);

    if looks_like_bundle_id {
        return Ok(ResolvedRunTarget::Bundle {
            bundle_dir: PathBuf::from("."),
            container_id: t,
        });
    }

    // Drain any unused `cmd` cleanly into `Image`.
    let resolved_cmd = std::mem::take(&mut cmd);
    Ok(ResolvedRunTarget::Image {
        image_ref: t,
        explicit_id: name,
        cmd: resolved_cmd,
    })
}

#[cfg(test)]
mod tests {
    use super::{ResolvedRunTarget, resolve_run_target};
    use std::path::PathBuf;

    #[test]
    fn resolves_explicit_bundle_mode() {
        let res = resolve_run_target(
            Some(PathBuf::from(".")),
            None,
            None,
            Some("verify-live-box".into()),
            vec![],
            true,
            |_| false,
        )
        .unwrap();
        assert_eq!(
            res,
            ResolvedRunTarget::Bundle {
                bundle_dir: PathBuf::from("."),
                container_id: "verify-live-box".into(),
            }
        );
    }

    #[test]
    fn resolves_explicit_image_with_name_and_cmd() {
        let res = resolve_run_target(
            None,
            Some("alpine-sec:3.20".into()),
            Some("sec-vol-ctr".into()),
            Some("/bin/sh".into()),
            vec!["-c".into(), "echo ok".into()],
            false,
            |_| true,
        )
        .unwrap();
        assert_eq!(
            res,
            ResolvedRunTarget::Image {
                image_ref: "alpine-sec:3.20".into(),
                explicit_id: Some("sec-vol-ctr".into()),
                cmd: vec!["/bin/sh".into(), "-c".into(), "echo ok".into()],
            }
        );
    }

    #[test]
    fn resolves_docker_positional_image_and_cmd_without_double_dash() {
        let res = resolve_run_target(
            None,
            None,
            Some("mc-server".into()),
            Some("itzg/minecraft-server".into()),
            vec![],
            true,
            |_| false,
        )
        .unwrap();
        assert_eq!(
            res,
            ResolvedRunTarget::Image {
                image_ref: "itzg/minecraft-server".into(),
                explicit_id: Some("mc-server".into()),
                cmd: vec![],
            }
        );

        let res2 = resolve_run_target(
            None,
            None,
            None,
            Some("alpine:3.20".into()),
            vec!["/bin/sh".into(), "-c".into(), "echo hi".into()],
            true,
            |_| true,
        )
        .unwrap();
        assert_eq!(
            res2,
            ResolvedRunTarget::Image {
                image_ref: "alpine:3.20".into(),
                explicit_id: None,
                cmd: vec!["/bin/sh".into(), "-c".into(), "echo hi".into()],
            }
        );
    }

    #[test]
    fn resolves_implicit_bundle_id_when_config_exists() {
        let res = resolve_run_target(
            None,
            None,
            None,
            Some("my-oci-box".into()),
            vec![],
            true,
            |_| false,
        )
        .unwrap();
        assert_eq!(
            res,
            ResolvedRunTarget::Bundle {
                bundle_dir: PathBuf::from("."),
                container_id: "my-oci-box".into(),
            }
        );
    }
}

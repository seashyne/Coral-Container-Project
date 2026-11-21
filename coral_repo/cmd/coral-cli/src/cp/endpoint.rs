//! `coral cp` endpoint parsing and container target resolution.

use crate::daemon_client::try_connect_channel;
use crate::local_run::resolve_local_container_overlay;
use anyhow::{Result, bail};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, ContainerStatusRequest};
use coral_runtime::StateStore;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpEndpoint {
    Host(PathBuf),
    Container { id: String, path: String },
}

pub fn parse_cp_endpoint(raw: &str) -> CpEndpoint {
    if let Some((left, right)) = raw.split_once(':')
        && !left.is_empty()
        && !left.contains('/')
        && !left.starts_with('.')
        && !right.is_empty()
    {
        return CpEndpoint::Container {
            id: left.to_string(),
            path: right.to_string(),
        };
    }
    CpEndpoint::Host(PathBuf::from(raw))
}

pub(super) fn sanitize_container_rel_path(raw: &str) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for comp in Path::new(raw).components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(seg) => clean.push(seg),
            Component::ParentDir => {
                bail!("container path '{raw}' must not contain parent traversal ('..')");
            }
            Component::Prefix(_) => {
                bail!("invalid container path '{raw}'");
            }
        }
    }
    Ok(clean)
}

pub(super) struct ResolvedContainer {
    pub id: String,
    pub lower_rootfs: PathBuf,
    pub upperdir: PathBuf,
    pub is_running: bool,
    pub runtime_root: Option<PathBuf>,
}

pub(super) async fn resolve_container_target(
    addr: &str,
    state_root: &Path,
    id: &str,
) -> Result<ResolvedContainer> {
    if let Some(ch) = try_connect_channel(addr).await {
        let mut rt = RuntimeServiceClient::new(ch);
        if let Ok(resp) = rt
            .container_status(ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: true,
            })
            .await
        {
            let inner = resp.into_inner();
            let is_running = inner
                .status
                .as_ref()
                .is_some_and(|s| s.state == ContainerState::ContainerRunning as i32);
            let inspect = inner
                .info
                .get("inspectJson")
                .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok());
            let lower = inspect
                .as_ref()
                .and_then(|v| v["rootfsPath"].as_str())
                .map(PathBuf::from)
                .or_else(|| inner.status.as_ref().map(|s| PathBuf::from(&s.image_ref)))
                .unwrap_or_default();
            let bundle = inner
                .info
                .get("bundleDir")
                .map(PathBuf::from)
                .unwrap_or_default();
            let upperdir = bundle.join("overlay/upper");
            let runtime_root = bundle
                .parent()
                .and_then(|p| p.parent())
                .map(|sd| sd.join("runtime-state"));
            return Ok(ResolvedContainer {
                id: id.to_string(),
                lower_rootfs: lower,
                upperdir,
                is_running,
                runtime_root,
            });
        }
    }

    let (lower_rootfs, upperdir) = resolve_local_container_overlay(state_root, id)?;
    let daemon_rt = state_root.join("daemon/runtime-state");
    let (is_running, runtime_root) = if let Ok(st) = StateStore::new(state_root).load(id) {
        (
            st.status == coral_oci::ContainerStatus::Running,
            Some(state_root.to_path_buf()),
        )
    } else if let Ok(st) = StateStore::new(&daemon_rt).load(id) {
        (
            st.status == coral_oci::ContainerStatus::Running,
            Some(daemon_rt),
        )
    } else {
        (false, None)
    };

    Ok(ResolvedContainer {
        id: id.to_string(),
        lower_rootfs,
        upperdir,
        is_running,
        runtime_root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_container_cp_endpoints() {
        assert_eq!(
            parse_cp_endpoint("ctr-123:/etc/motd"),
            CpEndpoint::Container {
                id: "ctr-123".into(),
                path: "/etc/motd".into(),
            }
        );
        assert_eq!(
            parse_cp_endpoint("./local.txt"),
            CpEndpoint::Host(PathBuf::from("./local.txt"))
        );
        assert_eq!(
            parse_cp_endpoint("/tmp/out.txt"),
            CpEndpoint::Host(PathBuf::from("/tmp/out.txt"))
        );
    }

    #[test]
    fn rejects_parent_traversal_in_container_path() {
        assert!(sanitize_container_rel_path("/etc/motd").is_ok());
        assert!(sanitize_container_rel_path("/../../etc/shadow").is_err());
    }
}

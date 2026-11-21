//! CNI v1.0.0 exec-request construction shared with the Auto-GC reaper.

use coral_net::CniExecRequest;

pub(crate) fn build_cni_exec_request(
    id: &str,
    config: &coral_api::PodSandboxConfig,
) -> CniExecRequest {
    let meta = config.metadata.as_ref();
    let netns_path = config
        .annotations
        .get("coral.cni.netns")
        .cloned()
        .unwrap_or_else(|| "/proc/self/ns/net".into());
    CniExecRequest {
        container_id: id.to_string(),
        netns_path,
        ifname: "eth0".into(),
        pod_name: meta.map(|m| m.name.clone()).unwrap_or_default(),
        pod_namespace: meta.map(|m| m.namespace.clone()).unwrap_or_default(),
        pod_uid: meta.map(|m| m.uid.clone()).unwrap_or_default(),
        runtime_config: None,
    }
}

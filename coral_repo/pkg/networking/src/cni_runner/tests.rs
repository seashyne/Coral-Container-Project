//! Unit tests for the CNI v1.0.0 plugin chain runner.

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

#[test]
fn executes_multi_plugin_cni_conflist_chain_add_check_and_reverse_del() {
    let dir = tempdir().unwrap();
    let conf_dir = dir.path().join("net.d");
    let bin_dir = dir.path().join("bin");
    let trace_log = dir.path().join("cni-trace.log");
    fs::create_dir_all(&conf_dir).unwrap();
    fs::create_dir_all(&bin_dir).unwrap();

    // Plugin 1: simulates Cilium/Calico/bridge assigning 10.244.7.15/24
    let p1 = bin_dir.join("mock-cilium");
    fs::write(
        &p1,
        format!(
            "#!/bin/sh\n\
             cat >/dev/null\n\
             echo \"$CNI_COMMAND:mock-cilium:$CNI_CONTAINERID:$CNI_IFNAME\" >> \"{}\"\n\
             if [ \"$CNI_COMMAND\" = \"ADD\" ]; then\n\
               echo '{{\"cniVersion\":\"1.0.0\",\"interfaces\":[{{\"name\":\"eth0\",\"mac\":\"02:42:ac:11:00:02\"}}],\"ips\":[{{\"address\":\"10.244.7.15/24\",\"gateway\":\"10.244.7.1\",\"interface\":0}}]}}'\n\
             fi\n",
            trace_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&p1, fs::Permissions::from_mode(0o755)).unwrap();

    // Plugin 2: simulates portmap reading `prevResult` from stdin and echoing it forward
    let p2 = bin_dir.join("mock-portmap");
    fs::write(
        &p2,
        format!(
            "#!/bin/sh\n\
             INPUT=$(cat)\n\
             echo \"$CNI_COMMAND:mock-portmap:$CNI_CONTAINERID\" >> \"{}\"\n\
             if [ \"$CNI_COMMAND\" = \"ADD\" ]; then\n\
               echo \"$INPUT\" | grep -q '10.244.7.15/24' || exit 42\n\
               echo '{{\"cniVersion\":\"1.0.0\",\"interfaces\":[{{\"name\":\"eth0\",\"mac\":\"02:42:ac:11:00:02\"}}],\"ips\":[{{\"address\":\"10.244.7.15/24\",\"gateway\":\"10.244.7.1\",\"interface\":0}}]}}'\n\
             fi\n",
            trace_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&p2, fs::Permissions::from_mode(0o755)).unwrap();

    // Write standard Kubernetes CNI 1.0.0 .conflist chaining both plugins
    fs::write(
        conf_dir.join("10-coral-k8s.conflist"),
        r#"{
          "cniVersion": "1.0.0",
          "name": "k8s-pod-network",
          "plugins": [
            { "type": "mock-cilium", "ipam": { "type": "host-local", "subnet": "10.244.7.0/24" } },
            { "type": "mock-portmap", "capabilities": { "portMappings": true } }
          ]
        }"#,
    )
    .unwrap();

    let chain = CniPluginChain::new(&conf_dir, &bin_dir);
    let req = CniExecRequest {
        container_id: "pod-123".into(),
        netns_path: "/proc/self/ns/net".into(),
        ifname: "eth0".into(),
        pod_name: "web-0".into(),
        pod_namespace: "default".into(),
        pod_uid: "uid-123".into(),
        runtime_config: None,
    };

    let added = chain.add_pod_network(&req).unwrap().unwrap();
    assert_eq!(added.network_name, "k8s-pod-network");
    assert_eq!(added.pod_ip.as_deref(), Some("10.244.7.15"));

    chain
        .check_pod_network(&req, Some(&added.raw_result))
        .unwrap();
    chain
        .del_pod_network(&req, Some(&added.raw_result))
        .unwrap();

    let trace = fs::read_to_string(&trace_log).unwrap();
    let lines: Vec<&str> = trace.lines().collect();
    assert_eq!(
        lines,
        vec![
            "ADD:mock-cilium:pod-123:eth0",
            "ADD:mock-portmap:pod-123",
            "CHECK:mock-cilium:pod-123:eth0",
            "CHECK:mock-portmap:pod-123",
            "DEL:mock-portmap:pod-123",
            "DEL:mock-cilium:pod-123:eth0",
        ]
    );
}

#[test]
fn passive_default_skips_incomplete_system_cni_without_failing() {
    let tmp = tempfile::tempdir().unwrap();
    let conf_dir = tmp.path().join("net.d");
    let empty_bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&conf_dir).unwrap();
    fs::create_dir_all(&empty_bin_dir).unwrap();
    fs::write(
        conf_dir.join("10-partial.conflist"),
        r#"{"cniVersion":"1.0.0","name":"partial","plugins":[{"type":"bridge"}]}"#,
    )
    .unwrap();

    let mut passive = CniPluginChain::new(&conf_dir, &empty_bin_dir);
    passive.strict = false;
    let req = CniExecRequest {
        container_id: "pod-1".into(),
        netns_path: "/proc/self/ns/net".into(),
        ifname: "eth0".into(),
        pod_name: "p".into(),
        pod_namespace: "default".into(),
        pod_uid: "u".into(),
        runtime_config: None,
    };
    assert!(passive.add_pod_network(&req).unwrap().is_none());

    let strict = CniPluginChain::new(&conf_dir, &empty_bin_dir);
    assert!(strict.add_pod_network(&req).is_err());
}

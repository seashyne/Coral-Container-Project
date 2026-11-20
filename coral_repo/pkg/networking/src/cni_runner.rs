//! Official CNI v1.0.0 Plugin Chain Runner (`ADD`, `CHECK`, `DEL`) compatible with
//! Kubernetes CNI plugins such as Cilium, Calico, Flannel, bridge, and portmap.
//!
//! Scans `conf_dir` (`/etc/cni/net.d`) lexicographically for `*.conflist` or `*.conf`,
//! executes plugin binaries from `bin_dir` (`/opt/cni/bin`) chaining `prevResult` JSON
//! across plugins on `ADD` and tearing them down in reverse order on `DEL`.

use crate::cni::CniResult;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Parsed CNI `.conflist` (or single `.conf` wrapped as a 1-plugin chain).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CniNetworkList {
    pub cni_version: String,
    pub name: String,
    #[serde(default)]
    pub disable_check: bool,
    pub plugins: Vec<Value>,
}

/// Parameters passed to a CNI plugin chain invocation for a Kubernetes Pod Sandbox.
#[derive(Debug, Clone)]
pub struct CniExecRequest {
    pub container_id: String,
    pub netns_path: String,
    pub ifname: String,
    pub pod_name: String,
    pub pod_namespace: String,
    pub pod_uid: String,
    pub runtime_config: Option<Value>,
}

/// Outcome returned by `CniPluginChain::add_pod_network`, including raw JSON `prevResult`
/// needed for subsequent `CHECK` and `DEL` operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CniAddOutcome {
    pub network_name: String,
    pub result: CniResult,
    pub raw_result: Value,
    pub pod_ip: Option<String>,
}

/// CNI v1.0.0 plugin chain executor.
#[derive(Debug, Clone)]
pub struct CniPluginChain {
    pub conf_dir: PathBuf,
    pub bin_dir: PathBuf,
    /// When true (custom CNI dir or explicit env config), missing plugins return an error.
    /// When false (passive `/etc/cni/net.d` auto-discovery), incomplete host CNI setups
    /// are skipped cleanly so rootless pods/compose keep working.
    pub strict: bool,
}

impl Default for CniPluginChain {
    fn default() -> Self {
        let env_conf = std::env::var_os("CORAL_CNI_CONF_DIR").map(PathBuf::from);
        let env_bin = std::env::var_os("CORAL_CNI_BIN_DIR").map(PathBuf::from);
        let strict = env_conf.is_some() || env_bin.is_some();
        Self {
            conf_dir: env_conf.unwrap_or_else(|| PathBuf::from("/etc/cni/net.d")),
            bin_dir: env_bin.unwrap_or_else(|| PathBuf::from("/opt/cni/bin")),
            strict,
        }
    }
}

impl CniPluginChain {
    pub fn new(conf_dir: impl Into<PathBuf>, bin_dir: impl Into<PathBuf>) -> Self {
        let conf_dir = conf_dir.into();
        let bin_dir = bin_dir.into();
        let strict = conf_dir != Path::new("/etc/cni/net.d") || bin_dir != Path::new("/opt/cni/bin");
        Self {
            conf_dir,
            bin_dir,
            strict,
        }
    }

    fn all_plugin_bins_exist(&self, net_list: &CniNetworkList) -> bool {
        net_list.plugins.iter().all(|plugin| {
            plugin
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|p| self.bin_dir.join(p).is_file())
        })
    }

    /// Scans `conf_dir` in lexicographical order and loads the first valid `.conflist` or `.conf`.
    pub fn discover_network_list(&self) -> Result<Option<CniNetworkList>> {
        if !self.conf_dir.is_dir() {
            return Ok(None);
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&self.conf_dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|ext| matches!(ext, "conflist" | "conf" | "json"))
            })
            .collect();
        files.sort();

        for path in files {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read CNI config '{}'", path.display()))?;
            let val: Value = serde_json::from_str(&raw)
                .with_context(|| format!("parse CNI JSON '{}'", path.display()))?;
            if val.get("plugins").and_then(|p| p.as_array()).is_some() {
                let list: CniNetworkList = serde_json::from_value(val)
                    .with_context(|| format!("decode CNI conflist '{}'", path.display()))?;
                if !list.plugins.is_empty() {
                    return Ok(Some(list));
                }
            } else if val.get("type").and_then(|t| t.as_str()).is_some() {
                let cni_version = val
                    .get("cniVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("1.0.0")
                    .to_string();
                let name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("coral-cni")
                    .to_string();
                return Ok(Some(CniNetworkList {
                    cni_version,
                    name,
                    disable_check: false,
                    plugins: vec![val],
                }));
            }
        }
        Ok(None)
    }

    /// Executes `CNI_COMMAND=ADD` across every plugin in the discovered CNI chain, chaining
    /// `prevResult` from each plugin to the next. Automatically rolls back with reverse-order
    /// `DEL` if any plugin in the chain fails.
    pub fn add_pod_network(&self, req: &CniExecRequest) -> Result<Option<CniAddOutcome>> {
        let Some(net_list) = self.discover_network_list()? else {
            return Ok(None);
        };
        if !self.strict {
            if !self.all_plugin_bins_exist(&net_list) {
                return Ok(None);
            }
            // Do not attempt privileged system-wide CNI bridge attachment against host `/proc/self/ns/net` when rootless.
            let is_root = fs::metadata("/proc/self")
                .map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
                .unwrap_or(false);
            if !is_root && req.netns_path == "/proc/self/ns/net" {
                return Ok(None);
            }
        }
        let mut prev_result: Option<Value> = None;
        let mut executed = 0usize;

        for plugin in &net_list.plugins {
            match self.exec_plugin("ADD", &net_list, plugin, req, prev_result.as_ref()) {
                Ok(out) => {
                    executed += 1;
                    if !out.is_null() {
                        prev_result = Some(out);
                    }
                }
                Err(err) => {
                    // Best-effort reverse-order DEL rollback per CNI v1.0.0 spec
                    for prev_plugin in net_list.plugins[..executed].iter().rev() {
                        let _ = self.exec_plugin(
                            "DEL",
                            &net_list,
                            prev_plugin,
                            req,
                            prev_result.as_ref(),
                        );
                    }
                    return Err(err);
                }
            }
        }

        let raw_result = prev_result.unwrap_or_else(|| {
            serde_json::json!({
                "cniVersion": net_list.cni_version,
                "interfaces": [],
                "ips": []
            })
        });
        let parsed: CniResult = serde_json::from_value(raw_result.clone()).unwrap_or(CniResult {
            cni_version: net_list.cni_version.clone(),
            interfaces: Vec::new(),
            ips: Vec::new(),
        });
        let pod_ip = extract_primary_ip(&parsed);

        Ok(Some(CniAddOutcome {
            network_name: net_list.name,
            result: parsed,
            raw_result,
            pod_ip,
        }))
    }

    /// Executes `CNI_COMMAND=CHECK` across the CNI chain (unless `disableCheck` is set).
    pub fn check_pod_network(
        &self,
        req: &CniExecRequest,
        prev_result: Option<&Value>,
    ) -> Result<()> {
        let Some(net_list) = self.discover_network_list()? else {
            return Ok(());
        };
        if net_list.disable_check {
            return Ok(());
        }
        for plugin in &net_list.plugins {
            let _ = self.exec_plugin("CHECK", &net_list, plugin, req, prev_result)?;
        }
        Ok(())
    }

    /// Executes `CNI_COMMAND=DEL` across the CNI chain in **reverse order** per CNI v1.0.0 spec.
    pub fn del_pod_network(
        &self,
        req: &CniExecRequest,
        prev_result: Option<&Value>,
    ) -> Result<()> {
        let Some(net_list) = self.discover_network_list()? else {
            return Ok(());
        };
        let mut first_err: Option<anyhow::Error> = None;
        for plugin in net_list.plugins.iter().rev() {
            if let Err(e) = self.exec_plugin("DEL", &net_list, plugin, req, prev_result)
                && first_err.is_none()
            {
                first_err = Some(e);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        Ok(())
    }

    fn exec_plugin(
        &self,
        command: &str,
        net_list: &CniNetworkList,
        plugin: &Value,
        req: &CniExecRequest,
        prev_result: Option<&Value>,
    ) -> Result<Value> {
        let plugin_type = plugin
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("CNI plugin entry missing string 'type' field"))?;
        if plugin_type.is_empty()
            || plugin_type.contains('/')
            || plugin_type.contains('\\')
            || plugin_type.contains("..")
        {
            bail!("unsafe CNI plugin type '{plugin_type}'");
        }

        let bin_path = self.bin_dir.join(plugin_type);
        if !bin_path.is_file() {
            bail!(
                "CNI plugin binary '{}' not found in '{}'",
                plugin_type,
                self.bin_dir.display()
            );
        }

        // Compose stdin config object: plugin fields + top-level cniVersion/name + prevResult + runtimeConfig.
        let mut stdin_obj = plugin.as_object().cloned().unwrap_or_default();
        stdin_obj.insert(
            "cniVersion".into(),
            Value::String(net_list.cni_version.clone()),
        );
        stdin_obj.insert("name".into(), Value::String(net_list.name.clone()));
        if let Some(prev) = prev_result {
            stdin_obj.insert("prevResult".into(), prev.clone());
        }
        if let Some(rc) = &req.runtime_config {
            stdin_obj.insert("runtimeConfig".into(), rc.clone());
        }
        let stdin_bytes = serde_json::to_vec(&Value::Object(stdin_obj))?;

        let cni_args = format!(
            "IgnoreUnknown=1;K8S_POD_NAMESPACE={};K8S_POD_NAME={};K8S_POD_INFRA_CONTAINER_ID={};K8S_POD_UID={}",
            req.pod_namespace, req.pod_name, req.container_id, req.pod_uid
        );
        let ifname = if req.ifname.trim().is_empty() {
            "eth0"
        } else {
            req.ifname.as_str()
        };

        let mut child = Command::new(&bin_path)
            .env("CNI_COMMAND", command)
            .env("CNI_CONTAINERID", &req.container_id)
            .env("CNI_NETNS", &req.netns_path)
            .env("CNI_IFNAME", ifname)
            .env("CNI_ARGS", cni_args)
            .env("CNI_PATH", &self.bin_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn CNI plugin '{}'", bin_path.display()))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&stdin_bytes)?;
        }
        let out = child.wait_with_output()?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();

        if !out.status.success() {
            bail!(
                "CNI plugin '{}' ({command}) exited with {}: stdout={} stderr={}",
                plugin_type,
                out.status,
                stdout,
                stderr
            );
        }
        if stdout.is_empty() {
            return Ok(Value::Null);
        }
        let parsed: Value = serde_json::from_str(&stdout).with_context(|| {
            format!("parse JSON stdout from CNI plugin '{plugin_type}' ({command}): {stdout}")
        })?;
        Ok(parsed)
    }
}

/// Extracts the primary IPv4/IPv6 host address (without `/prefix` CIDR) from a CNI v1.0.0 result.
pub fn extract_primary_ip(res: &CniResult) -> Option<String> {
    res.ips
        .first()
        .map(|ip| ip.address.split('/').next().unwrap_or(&ip.address).to_string())
        .filter(|s| !s.is_empty())
}

/// Checks if any CNI plugin configuration file exists inside `conf_dir`.
pub fn has_cni_configs(conf_dir: &Path) -> bool {
    CniPluginChain::new(conf_dir, PathBuf::new())
        .discover_network_list()
        .ok()
        .flatten()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
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
}

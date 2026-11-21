//! Execution of the CNI plugin chain (`ADD`, `CHECK`, `DEL`).

use super::{CniAddOutcome, CniExecRequest, CniNetworkList, CniPluginChain};
use crate::cni::CniResult;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

impl CniPluginChain {
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
            let is_root = std::fs::metadata("/proc/self")
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
        let pod_ip = super::helpers::extract_primary_ip(&parsed);

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

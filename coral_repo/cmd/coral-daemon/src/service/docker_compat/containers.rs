//! Docker Engine REST API container creation and lifecycle mutation handlers
//! (`POST /containers/create`, `POST /containers/{id}/start`, `stop`, `DELETE /containers/{id}`).

use super::HttpResponse;
use super::archive_build::handle_container_archive;
use super::endpoints::{handle_container_wait, handle_exec_create};
use super::handlers::{handle_container_logs, handle_inspect_container, parse_query_param};
use super::state::DockerCompatState;
use crate::service::CoralDaemonService;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{
    ContainerConfig, ContainerMetadata, CreateContainerRequest, ImageSpec, KeyValue,
    LinuxContainerConfig, LinuxContainerResources, PodSandboxConfig, PodSandboxMetadata,
    RemoveContainerRequest, RemovePodSandboxRequest, RunPodSandboxRequest, StartContainerRequest,
    StopContainerRequest,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use tonic::Request;

pub(super) async fn handle_create_container(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    query: &str,
    body: &[u8],
) -> HttpResponse {
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return HttpResponse::json("400 Bad Request", &json!({ "message": "invalid JSON body" }));
    };
    let image = payload
        .get("Image")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if image.is_empty() {
        return HttpResponse::json("400 Bad Request", &json!({ "message": "Image is required" }));
    }
    let name = parse_query_param(query, "name")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| svc.next_id("docker"));
    let hostname = payload
        .get("Hostname")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&name)
        .to_string();

    let cmd_vec: Vec<String> = payload
        .get("Cmd")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let (command, args) = if cmd_vec.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (vec![cmd_vec[0].clone()], cmd_vec[1..].to_vec())
    };

    let envs: Vec<KeyValue> = payload
        .get("Env")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .filter_map(|e| {
                    e.split_once('=').map(|(k, v)| KeyValue {
                        key: k.to_string(),
                        value: v.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mut annotations = HashMap::new();
    if let Some(ep) = payload.get("Entrypoint") {
        if let Some(arr) = ep.as_array() {
            let v: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect();
            if let Ok(s) = serde_json::to_string(&v) {
                annotations.insert("coral.entrypoint".into(), s);
            }
        } else if let Some(s) = ep.as_str() {
            annotations.insert("coral.entrypoint".into(), s.to_string());
        }
    }
    if let Some(u) = payload.get("User").and_then(|v| v.as_str())
        && !u.is_empty()
    {
        annotations.insert("coral.user".into(), u.to_string());
    }
    let mut mem_bytes = 0i64;
    if let Some(hc) = payload.get("HostConfig") {
        if let Some(binds) = hc.get("Binds").and_then(|v| v.as_array()) {
            let mounts: Vec<&str> = binds.iter().filter_map(|x| x.as_str()).collect();
            if !mounts.is_empty() {
                annotations.insert("coral.mounts".into(), mounts.join("\n"));
            }
        }
        if let Some(net) = hc.get("NetworkMode").and_then(|v| v.as_str())
            && !net.is_empty()
        {
            let mapped = if net == "default" { "bridge" } else { net };
            annotations.insert("coral.network".into(), mapped.to_string());
        }
        if let Some(m) = hc.get("Memory").and_then(|v| v.as_i64())
            && m > 0
        {
            mem_bytes = m;
        }
        if let Some(pb) = hc.get("PortBindings").and_then(|v| v.as_object()) {
            let mut ports = Vec::new();
            for (ctr_spec, bindings) in pb {
                let (cport, proto) = ctr_spec.split_once('/').unwrap_or((ctr_spec, "tcp"));
                if let Some(arr) = bindings.as_array() {
                    for b in arr {
                        let hport = b.get("HostPort").and_then(|v| v.as_str()).unwrap_or("");
                        if !hport.is_empty() {
                            ports.push(format!("{hport}:{cport}/{proto}"));
                        }
                    }
                }
            }
            if !ports.is_empty() {
                annotations.insert("coral.ports".into(), ports.join(","));
            }
        }
        if let Some(rp) = hc.get("RestartPolicy").and_then(|v| v.get("Name")).and_then(|v| v.as_str())
            && !rp.is_empty()
            && rp != "no"
        {
            annotations.insert("coral.restart".into(), rp.to_string());
        }
        if let Some(lc) = hc.get("LogConfig").and_then(|v| v.get("Type")).and_then(|v| v.as_str())
            && !lc.is_empty()
        {
            annotations.insert("coral.log.driver".into(), lc.to_string());
        }
        if hc.get("DeviceRequests").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty()) {
            annotations.insert("coral.gpus".into(), "all".into());
        }
        if hc.get("Privileged").and_then(|v| v.as_bool()).unwrap_or(false) {
            annotations.insert("coral.privileged".into(), "true".into());
        }
        for (hc_key, ann_key) in [("CapDrop", "coral.cap_drop"), ("CapAdd", "coral.cap_add"), ("SecurityOpt", "coral.security_opt")] {
            if let Some(arr) = hc.get(hc_key).and_then(|v| v.as_array()) {
                let vals: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).collect();
                if !vals.is_empty() {
                    annotations.insert(ann_key.into(), vals.join("\n"));
                }
            }
        }
    }

    let pod_resp = match svc
        .run_pod_sandbox(Request::new(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: format!("{name}-pod"),
                    uid: name.clone(),
                    namespace: "docker-compat".into(),
                    attempt: 0,
                }),
                hostname,
                log_directory: String::new(),
                labels: HashMap::from([("managed-by".into(), "docker-compat".into())]),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        }))
        .await
    {
        Ok(r) => r.into_inner(),
        Err(e) => {
            return HttpResponse::json(
                "500 Internal Server Error",
                &json!({ "message": e.message() }),
            );
        }
    };

    let linux = (mem_bytes > 0).then_some(LinuxContainerConfig {
        resources: Some(LinuxContainerResources {
            cpu_period: 0,
            cpu_quota: 0,
            cpu_shares: 0,
            memory_limit_in_bytes: mem_bytes,
            pids_limit: 0,
        }),
    });

    match svc
        .create_container(Request::new(CreateContainerRequest {
            pod_sandbox_id: pod_resp.pod_sandbox_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata { name, attempt: 0 }),
                image: Some(ImageSpec {
                    image,
                    annotations: HashMap::new(),
                }),
                command,
                args,
                working_dir: payload
                    .get("WorkingDir")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                envs,
                labels: HashMap::from([("managed-by".into(), "docker-compat".into())]),
                annotations,
                log_path: String::new(),
                linux,
            }),
            sandbox_config: None,
        }))
        .await
    {
        Ok(r) => {
            let cid = r.into_inner().container_id;
            compat
                .record_event("container", "create", &cid, json!({}))
                .await;
            HttpResponse::json(
                "201 Created",
                &json!({
                    "Id": cid,
                    "Warnings": []
                }),
            )
        }
        Err(e) => {
            let _ = svc
                .remove_pod_sandbox(Request::new(RemovePodSandboxRequest {
                    pod_sandbox_id: pod_resp.pod_sandbox_id,
                }))
                .await;
            HttpResponse::json("404 Not Found", &json!({ "message": e.message() }))
        }
    }
}

pub(super) async fn handle_container_action(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    method: &str,
    subpath: &str,
    query: &str,
    body: &[u8],
) -> HttpResponse {
    let (id_raw, action) = subpath.split_once('/').unwrap_or((subpath, ""));
    let id = id_raw.trim_matches('/');
    match (method, action) {
        ("POST", "start") => match svc
            .start_container(Request::new(StartContainerRequest {
                container_id: id.to_string(),
            }))
            .await
        {
            Ok(_) => {
                compat.record_event("container", "start", id, json!({})).await;
                HttpResponse::empty("204 No Content")
            }
            Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
        },
        ("POST", "stop") | ("POST", "kill") => match svc
            .stop_container(Request::new(StopContainerRequest {
                container_id: id.to_string(),
                timeout: 5,
            }))
            .await
        {
            Ok(_) => {
                compat.record_event("container", "stop", id, json!({})).await;
                HttpResponse::empty("204 No Content")
            }
            Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
        },
        ("POST", "exec") => handle_exec_create(svc, compat, id, body).await,
        ("POST", "wait") => handle_container_wait(svc, id).await,
        ("POST", "pause") => match svc.pause_container(id).await {
            Ok(_) => {
                compat.record_event("container", "pause", id, json!({})).await;
                HttpResponse::empty("204 No Content")
            }
            Err(e) => HttpResponse::json("409 Conflict", &json!({ "message": e.message() })),
        },
        ("POST", "unpause") => match svc.unpause_container(id).await {
            Ok(_) => {
                compat.record_event("container", "unpause", id, json!({})).await;
                HttpResponse::empty("204 No Content")
            }
            Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
        },
        ("POST", "rename") => {
            let new_name = parse_query_param(query, "name").unwrap_or("");
            match svc.rename_container(id, new_name).await {
                Ok(_) => {
                    compat
                        .record_event("container", "rename", id, json!({ "name": new_name }))
                        .await;
                    HttpResponse::empty("204 No Content")
                }
                Err(e) => HttpResponse::json("409 Conflict", &json!({ "message": e.message() })),
            }
        }
        ("POST", "update") => {
            let payload = serde_json::from_slice::<Value>(body).unwrap_or_else(|_| json!({}));
            let mem = payload.get("Memory").and_then(|v| v.as_i64());
            let quota = payload.get("CpuQuota").and_then(|v| v.as_i64());
            let period = payload.get("CpuPeriod").and_then(|v| v.as_i64());
            let pids = payload.get("PidsLimit").and_then(|v| v.as_i64());
            match svc.update_container_resources(id, mem, quota, period, pids).await {
                Ok(_) => HttpResponse::json("200 OK", &json!({ "Warnings": [] })),
                Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
            }
        }
        ("PUT" | "GET", "archive") => {
            handle_container_archive(svc, method, id, query, body).await
        }
        ("DELETE", "") => {
            let pod_id = {
                let store = svc.store.read().await;
                store
                    .resolve_container_id(id)
                    .and_then(|cid| store.containers.get(&cid).map(|c| c.pod_sandbox_id.clone()))
            };
            let _ = svc
                .stop_container(Request::new(StopContainerRequest {
                    container_id: id.to_string(),
                    timeout: 2,
                }))
                .await;
            match svc
                .remove_container(Request::new(RemoveContainerRequest {
                    container_id: id.to_string(),
                }))
                .await
            {
                Ok(_) => {
                    if let Some(pid) = pod_id {
                        let _ = svc
                            .remove_pod_sandbox(Request::new(RemovePodSandboxRequest {
                                pod_sandbox_id: pid,
                            }))
                            .await;
                    }
                    compat.record_event("container", "destroy", id, json!({})).await;
                    HttpResponse::empty("204 No Content")
                }
                Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
            }
        }
        ("GET", "json") => handle_inspect_container(svc, id).await,
        ("GET", "logs") => handle_container_logs(svc, id).await,
        ("GET", "top") => match svc.container_top(id).await {
            Ok(json_str) => {
                let rows: Vec<Value> = serde_json::from_str::<Vec<Value>>(&json_str)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| {
                        json!([
                            p["pid"].to_string(),
                            p["ppid"].to_string(),
                            p["state"].as_str().unwrap_or("?"),
                            format!("{}kB", p["rssKb"].as_u64().unwrap_or(0)),
                            p["cmd"].as_str().unwrap_or(""),
                        ])
                    })
                    .collect();
                HttpResponse::json(
                    "200 OK",
                    &json!({ "Titles": ["PID", "PPID", "STAT", "RSS", "CMD"], "Processes": rows }),
                )
            }
            Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
        },
        _ => HttpResponse::json(
            "404 Not Found",
            &json!({ "message": format!("unsupported container endpoint: {method} /containers/{subpath}") }),
        ),
    }
}

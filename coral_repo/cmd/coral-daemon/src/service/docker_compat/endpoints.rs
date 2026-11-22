//! Extended Docker Engine REST API handlers:
//! `/images/create`, `/images/{name}/json`, `/containers/{id}/exec`, `/exec/{id}/*`,
//! `/containers/{id}/wait`, `/events`, `/networks/*`, and `/volumes/*`.

use super::HttpResponse;
use super::handlers::parse_query_param;
use super::state::{DockerCompatState, DockerExecRecord, encode_docker_stream_frame};
use crate::images::{self, OciImageConfig};
use crate::service::CoralDaemonService;
use coral_api::image_service_server::ImageService;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{
    ContainerState as CriContainerState, ExecSyncRequest, ImageSpec, PullImageRequest,
    RemoveImageRequest,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tonic::Request;

pub(super) async fn handle_image_pull(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    query: &str,
) -> HttpResponse {
    let from_image = parse_query_param(query, "fromImage")
        .map(percent_decode)
        .unwrap_or_default();
    if from_image.is_empty() {
        return HttpResponse::json(
            "400 Bad Request",
            &json!({ "message": "fromImage query parameter is required" }),
        );
    }
    let tag = parse_query_param(query, "tag")
        .map(percent_decode)
        .unwrap_or_default();
    let image_ref = if !tag.is_empty() && !from_image.contains(':') {
        format!("{from_image}:{tag}")
    } else {
        from_image
    };
    let platform = parse_query_param(query, "platform").map(percent_decode);
    let mut annotations = HashMap::new();
    if let Some(p) = platform
        && !p.is_empty()
    {
        annotations.insert("coral.platform".into(), p);
    }

    match svc
        .pull_image(Request::new(PullImageRequest {
            image: Some(ImageSpec {
                image: image_ref.clone(),
                annotations,
            }),
        }))
        .await
    {
        Ok(resp) => {
            let digest = resp.into_inner().image_ref;
            compat
                .record_event("image", "pull", &image_ref, json!({ "name": image_ref }))
                .await;
            let body = format!(
                "{}\n{}\n",
                json!({ "status": format!("Pulling from {image_ref}") }),
                json!({ "status": format!("Status: Downloaded newer image for {image_ref}"), "id": digest }),
            );
            HttpResponse {
                status: "200 OK",
                content_type: "application/json",
                body: body.into_bytes(),
            }
        }
        Err(e) => HttpResponse::json(
            "500 Internal Server Error",
            &json!({ "message": e.message() }),
        ),
    }
}

pub(super) async fn handle_image_item(
    svc: &CoralDaemonService,
    method: &str,
    subpath: &str,
) -> HttpResponse {
    if method == "GET" && let Some(raw_name) = subpath.strip_suffix("/json") {
        let name = percent_decode(raw_name.trim_matches('/'));
        let store = svc.store.read().await;
        let Some(img) = images::find_image(&store.images, &name) else {
            return HttpResponse::json(
                "404 Not Found",
                &json!({ "message": format!("No such image: {name}") }),
            );
        };
        let cfg = OciImageConfig::load_for_rootfs(Path::new(&img.rootfs_path));
        let exposed: serde_json::Map<String, Value> = cfg
            .exposed_ports
            .iter()
            .map(|p| (p.clone(), json!({})))
            .collect();
        return HttpResponse::json(
            "200 OK",
            &json!({
                "Id": img.id,
                "RepoTags": img.repo_tags,
                "Size": img.size,
                "Os": "linux",
                "Architecture": std::env::consts::ARCH,
                "Config": {
                    "Env": cfg.env,
                    "Cmd": cfg.cmd,
                    "Entrypoint": cfg.entrypoint,
                    "WorkingDir": cfg.working_dir,
                    "User": cfg.user,
                    "Labels": cfg.labels,
                    "ExposedPorts": exposed,
                }
            }),
        );
    }
    if method == "DELETE" {
        let name = percent_decode(subpath.trim_matches('/'));
        return match svc
            .remove_image(Request::new(RemoveImageRequest {
                image: Some(ImageSpec {
                    image: name.clone(),
                    annotations: HashMap::new(),
                }),
            }))
            .await
        {
            Ok(_) => HttpResponse::json("200 OK", &json!([{ "Untagged": name }])),
            Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.message() })),
        };
    }
    HttpResponse::json("404 Not Found", &json!({ "message": "not found" }))
}

pub(super) async fn handle_exec_create(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    container_id: &str,
    body: &[u8],
) -> HttpResponse {
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return HttpResponse::json("400 Bad Request", &json!({ "message": "invalid JSON" }));
    };
    let resolved_cid = {
        let store = svc.store.read().await;
        store.resolve_container_id(container_id)
    };
    let Some(cid) = resolved_cid else {
        return HttpResponse::json(
            "404 Not Found",
            &json!({ "message": format!("No such container: {container_id}") }),
        );
    };
    let cmd: Vec<String> = payload
        .get("Cmd")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if cmd.is_empty() {
        return HttpResponse::json("400 Bad Request", &json!({ "message": "Cmd is required" }));
    }
    let tty = payload.get("Tty").and_then(|v| v.as_bool()).unwrap_or(false);
    let exec_id = compat.next_exec_id();
    compat.execs.write().await.insert(
        exec_id.clone(),
        DockerExecRecord {
            id: exec_id.clone(),
            container_id: cid,
            cmd,
            tty,
            running: false,
            exit_code: 0,
        },
    );
    HttpResponse::json("201 Created", &json!({ "Id": exec_id }))
}

pub(super) async fn handle_exec_route(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    method: &str,
    subpath: &str,
) -> HttpResponse {
    let (id_raw, action) = subpath.split_once('/').unwrap_or((subpath, ""));
    let exec_id = id_raw.trim_matches('/');
    match (method, action) {
        ("POST", "start") => {
            let rec = compat.execs.read().await.get(exec_id).cloned();
            let Some(rec) = rec else {
                return HttpResponse::json("404 Not Found", &json!({ "message": "no such exec" }));
            };
            match svc
                .exec_sync(Request::new(ExecSyncRequest {
                    container_id: rec.container_id.clone(),
                    cmd: rec.cmd.clone(),
                    timeout: 30,
                }))
                .await
            {
                Ok(resp) => {
                    let inner = resp.into_inner();
                    if let Some(entry) = compat.execs.write().await.get_mut(exec_id) {
                        entry.running = false;
                        entry.exit_code = inner.exit_code;
                    }
                    let mut body = Vec::new();
                    body.extend_from_slice(&encode_docker_stream_frame(1, &inner.stdout, rec.tty));
                    body.extend_from_slice(&encode_docker_stream_frame(2, &inner.stderr, rec.tty));
                    HttpResponse {
                        status: "200 OK",
                        content_type: "application/vnd.docker.raw-stream",
                        body,
                    }
                }
                Err(e) => HttpResponse::json(
                    "500 Internal Server Error",
                    &json!({ "message": e.message() }),
                ),
            }
        }
        ("GET", "json") => {
            let rec = compat.execs.read().await.get(exec_id).cloned();
            let Some(rec) = rec else {
                return HttpResponse::json("404 Not Found", &json!({ "message": "no such exec" }));
            };
            HttpResponse::json(
                "200 OK",
                &json!({
                    "ID": rec.id,
                    "Running": rec.running,
                    "ExitCode": rec.exit_code,
                    "ContainerID": rec.container_id,
                }),
            )
        }
        _ => HttpResponse::json("404 Not Found", &json!({ "message": "not found" })),
    }
}

pub(super) async fn handle_container_wait(
    svc: &CoralDaemonService,
    query_id: &str,
) -> HttpResponse {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let state_info = {
            let store = svc.store.read().await;
            store
                .resolve_container_id(query_id)
                .and_then(|cid| store.containers.get(&cid).map(|c| (c.state, c.exit_code)))
        };
        let Some((st, code)) = state_info else {
            return HttpResponse::json(
                "404 Not Found",
                &json!({ "message": format!("No such container: {query_id}") }),
            );
        };
        if st == CriContainerState::ContainerExited {
            return HttpResponse::json("200 OK", &json!({ "StatusCode": code }));
        }
        if Instant::now() >= deadline {
            return HttpResponse::json("200 OK", &json!({ "StatusCode": 0 }));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) async fn handle_events(compat: &DockerCompatState, query: &str) -> HttpResponse {
    let since = parse_query_param(query, "since").and_then(|s| s.parse::<i64>().ok());
    let events = compat.snapshot_events(since).await;
    let mut body = String::new();
    for ev in events {
        if let Ok(line) = serde_json::to_string(&ev) {
            body.push_str(&line);
            body.push('\n');
        }
    }
    HttpResponse {
        status: "200 OK",
        content_type: "application/json",
        body: body.into_bytes(),
    }
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex_str, 16)
        {
            out.push(val as char);
            i += 3;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

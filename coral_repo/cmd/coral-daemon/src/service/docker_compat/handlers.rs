//! Route handlers translating read-only Docker Engine REST API endpoints into `CoralDaemonService` queries.

use super::HttpResponse;
use super::containers::{handle_container_action, handle_create_container};
use crate::service::CoralDaemonService;
use coral_api::ContainerState as CriContainerState;
use serde_json::{Value, json};
use std::fs;

pub(super) async fn dispatch_docker_request(
    svc: &CoralDaemonService,
    method: &str,
    route: &str,
    query: &str,
    body: &[u8],
) -> HttpResponse {
    match (method, route) {
        ("GET" | "HEAD", "/_ping") => HttpResponse::text("200 OK", "OK"),
        ("GET", "/version") => handle_version(),
        ("GET", "/info") => handle_info(svc).await,
        ("GET", "/images/json") => handle_list_images(svc).await,
        ("GET", "/containers/json") => handle_list_containers(svc, query).await,
        ("POST", "/containers/create") => handle_create_container(svc, query, body).await,
        _ if route.starts_with("/containers/") => {
            handle_container_action(svc, method, &route["/containers/".len()..]).await
        }
        _ => HttpResponse::json(
            "404 Not Found",
            &json!({ "message": format!("page not found: {method} {route}") }),
        ),
    }
}

fn handle_version() -> HttpResponse {
    HttpResponse::json(
        "200 OK",
        &json!({
            "Platform": { "Name": "Coral Container Runtime (CRI v1 + Docker Compat)" },
            "Version": env!("CARGO_PKG_VERSION"),
            "ApiVersion": "1.45",
            "MinAPIVersion": "1.24",
            "Os": "linux",
            "Arch": std::env::consts::ARCH,
            "Experimental": false
        }),
    )
}

async fn handle_info(svc: &CoralDaemonService) -> HttpResponse {
    let store = svc.store.read().await;
    let total = store.containers.len();
    let running = store
        .containers
        .values()
        .filter(|c| c.state == CriContainerState::ContainerRunning)
        .count();
    let stopped = total.saturating_sub(running);
    let images = store.images.len();

    HttpResponse::json(
        "200 OK",
        &json!({
            "ID": "coral-daemon-v2",
            "Containers": total,
            "ContainersRunning": running,
            "ContainersPaused": 0,
            "ContainersStopped": stopped,
            "Images": images,
            "Driver": "overlayfs",
            "OperatingSystem": "Linux",
            "OSType": "linux",
            "Architecture": std::env::consts::ARCH,
            "ServerVersion": env!("CARGO_PKG_VERSION"),
            "SecurityOptions": ["name=seccomp,profile=default", "name=rootless"]
        }),
    )
}

async fn handle_list_images(svc: &CoralDaemonService) -> HttpResponse {
    let store = svc.store.read().await;
    let list: Vec<Value> = store
        .images
        .iter()
        .map(|img| {
            json!({
                "Id": img.id,
                "ParentId": "",
                "RepoTags": img.repo_tags,
                "RepoDigests": [],
                "Created": 1700000000,
                "Size": img.size,
                "SharedSize": -1,
                "VirtualSize": img.size,
                "Labels": {},
                "Containers": -1
            })
        })
        .collect();
    HttpResponse::json("200 OK", &Value::Array(list))
}

async fn handle_list_containers(svc: &CoralDaemonService, query: &str) -> HttpResponse {
    let show_all = parse_query_param(query, "all")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let store = svc.store.read().await;
    let mut list = Vec::new();

    for c in store.containers.values() {
        let is_running = c.state == CriContainerState::ContainerRunning;
        if !show_all && !is_running {
            continue;
        }
        let name = c
            .config
            .metadata
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| c.id.clone());
        let image = c
            .config
            .image
            .as_ref()
            .map(|i| i.image.clone())
            .unwrap_or_default();
        let (state_str, status_str) = match c.state {
            CriContainerState::ContainerRunning => ("running", "Up".to_string()),
            CriContainerState::ContainerCreated => ("created", "Created".to_string()),
            _ => ("exited", format!("Exited ({})", c.exit_code)),
        };
        let mut cmd_parts = c.config.command.clone();
        cmd_parts.extend(c.config.args.iter().cloned());

        list.push(json!({
            "Id": c.id,
            "Names": [format!("/{name}")],
            "Image": image,
            "ImageID": image,
            "Command": cmd_parts.join(" "),
            "Created": c.created_at / 1_000_000_000,
            "State": state_str,
            "Status": status_str,
            "Ports": [],
            "Labels": c.config.labels
        }));
    }
    HttpResponse::json("200 OK", &Value::Array(list))
}

pub(super) async fn handle_inspect_container(
    svc: &CoralDaemonService,
    query_id: &str,
) -> HttpResponse {
    let store = svc.store.read().await;
    let Some(cid) = store.resolve_container_id(query_id) else {
        return HttpResponse::json(
            "404 Not Found",
            &json!({ "message": format!("No such container: {query_id}") }),
        );
    };
    let Some(c) = store.containers.get(&cid) else {
        return HttpResponse::json("404 Not Found", &json!({ "message": "not found" }));
    };
    let is_running = c.state == CriContainerState::ContainerRunning;
    let status_str = match c.state {
        CriContainerState::ContainerRunning => "running",
        CriContainerState::ContainerCreated => "created",
        _ => "exited",
    };
    let name = c
        .config
        .metadata
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| c.id.clone());
    let image = c
        .config
        .image
        .as_ref()
        .map(|i| i.image.clone())
        .unwrap_or_default();
    HttpResponse::json(
        "200 OK",
        &json!({
            "Id": c.id,
            "Name": format!("/{name}"),
            "State": {
                "Status": status_str,
                "Running": is_running,
                "Paused": false,
                "Restarting": false,
                "ExitCode": c.exit_code,
            },
            "Image": image,
            "Config": {
                "Hostname": name,
                "Image": image,
                "Labels": c.config.labels,
            }
        }),
    )
}

pub(super) async fn handle_container_logs(
    svc: &CoralDaemonService,
    query_id: &str,
) -> HttpResponse {
    let log_path = {
        let store = svc.store.read().await;
        store
            .resolve_container_id(query_id)
            .and_then(|cid| store.containers.get(&cid).map(|c| c.log_path.clone()))
    };
    let Some(path) = log_path else {
        return HttpResponse::json("404 Not Found", &json!({ "message": "container not found" }));
    };
    let content = fs::read_to_string(&path).unwrap_or_default();
    HttpResponse::text("200 OK", content)
}

pub(super) fn parse_query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(v);
        }
    }
    None
}

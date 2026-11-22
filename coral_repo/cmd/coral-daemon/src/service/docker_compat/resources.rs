//! Docker Engine REST API `/networks/*` and `/volumes/*` handlers.

use super::HttpResponse;
use coral_runtime::rootfs::{
    default_volumes_dir, ensure_named_volume, list_named_volumes, remove_named_volume,
};
use serde_json::{Value, json};

pub(super) async fn handle_networks(method: &str, subpath: &str, body: &[u8]) -> HttpResponse {
    match (method, subpath) {
        ("GET", "") => {
            let list: Vec<Value> = coral_net::list_networks()
                .into_iter()
                .map(|n| {
                    json!({
                        "Name": n.name,
                        "Id": format!("coral-net-{}", n.name),
                        "Driver": n.driver,
                        "Scope": "local",
                        "IPAM": { "Config": [{ "Subnet": n.subnet, "Gateway": n.gateway }] }
                    })
                })
                .collect();
            HttpResponse::json("200 OK", &Value::Array(list))
        }
        ("POST", "create") => {
            let Ok(payload) = serde_json::from_slice::<Value>(body) else {
                return HttpResponse::json("400 Bad Request", &json!({ "message": "invalid JSON" }));
            };
            let name = payload.get("Name").and_then(|v| v.as_str()).unwrap_or("");
            let driver = payload.get("Driver").and_then(|v| v.as_str());
            match coral_net::create_network(name, driver, None) {
                Ok(rec) => HttpResponse::json(
                    "201 Created",
                    &json!({ "Id": format!("coral-net-{}", rec.name), "Warning": "" }),
                ),
                Err(e) => HttpResponse::json("409 Conflict", &json!({ "message": e.to_string() })),
            }
        }
        ("DELETE", name) if !name.is_empty() => {
            let clean = name.trim_start_matches("coral-net-");
            match coral_net::remove_network(clean) {
                Ok(()) => HttpResponse::empty("204 No Content"),
                Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.to_string() })),
            }
        }
        ("GET", name) if !name.is_empty() => {
            let clean = name.trim_start_matches("coral-net-");
            if let Some(n) = coral_net::find_network(clean) {
                HttpResponse::json(
                    "200 OK",
                    &json!({
                        "Name": n.name,
                        "Id": format!("coral-net-{}", n.name),
                        "Driver": n.driver,
                        "Scope": "local",
                        "IPAM": { "Config": [{ "Subnet": n.subnet, "Gateway": n.gateway }] }
                    }),
                )
            } else {
                HttpResponse::json("404 Not Found", &json!({ "message": "network not found" }))
            }
        }
        _ => HttpResponse::json("404 Not Found", &json!({ "message": "not found" })),
    }
}

pub(super) async fn handle_volumes(method: &str, subpath: &str, body: &[u8]) -> HttpResponse {
    let root = default_volumes_dir();
    match (method, subpath) {
        ("GET", "") => {
            let vols: Vec<Value> = list_named_volumes(&root)
                .unwrap_or_default()
                .into_iter()
                .map(|(name, path)| {
                    json!({
                        "Name": name,
                        "Driver": "local",
                        "Mountpoint": path.to_string_lossy(),
                    })
                })
                .collect();
            HttpResponse::json("200 OK", &json!({ "Volumes": vols, "Warnings": [] }))
        }
        ("POST", "create") => {
            let payload = serde_json::from_slice::<Value>(body).unwrap_or_else(|_| json!({}));
            let name = payload
                .get("Name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("vol-{}", chrono::Utc::now().timestamp_millis()));
            match ensure_named_volume(&root, &name) {
                Ok(path) => HttpResponse::json(
                    "201 Created",
                    &json!({ "Name": name, "Driver": "local", "Mountpoint": path.to_string_lossy() }),
                ),
                Err(e) => HttpResponse::json("400 Bad Request", &json!({ "message": e.to_string() })),
            }
        }
        ("DELETE", name) if !name.is_empty() => match remove_named_volume(&root, name) {
            Ok(()) => HttpResponse::empty("204 No Content"),
            Err(e) => HttpResponse::json("404 Not Found", &json!({ "message": e.to_string() })),
        },
        _ => HttpResponse::json("404 Not Found", &json!({ "message": "not found" })),
    }
}

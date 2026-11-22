//! Docker Engine REST API tar archive (`/containers/{id}/archive`) and
//! tar-context image build (`POST /build`) handlers.

use super::HttpResponse;
use super::handlers::parse_query_param;
use super::state::DockerCompatState;
use crate::images;
use crate::service::CoralDaemonService;
use coral_runtime::rootfs::safe_join_rootfs;
use serde_json::json;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

/// Handles `PUT /containers/{id}/archive?path=...` (unpack tar into container upperdir)
/// and `GET /containers/{id}/archive?path=...` (export container path as tar stream).
pub(super) async fn handle_container_archive(
    svc: &CoralDaemonService,
    method: &str,
    query_id: &str,
    query: &str,
    body: &[u8],
) -> HttpResponse {
    let target_rel = parse_query_param(query, "path")
        .map(percent_decode_simple)
        .unwrap_or_else(|| "/".to_string());

    let dirs = {
        let store = svc.store.read().await;
        store.resolve_container_id(query_id).and_then(|cid| {
            store
                .containers
                .get(&cid)
                .map(|c| (c.bundle_dir.join("overlay/upper"), c.rootfs_path.clone()))
        })
    };
    let Some((upper_dir, rootfs_dir)) = dirs else {
        return HttpResponse::json(
            "404 Not Found",
            &json!({ "message": format!("No such container: {query_id}") }),
        );
    };

    match method {
        "PUT" => {
            if fs::create_dir_all(&upper_dir).is_err() {
                return HttpResponse::json(
                    "500 Internal Server Error",
                    &json!({ "message": "failed to prepare container overlay upperdir" }),
                );
            }
            let Ok(dest) = safe_join_rootfs(&upper_dir, &target_rel) else {
                return HttpResponse::json(
                    "400 Bad Request",
                    &json!({ "message": format!("unsafe archive path '{target_rel}'") }),
                );
            };
            if let Err(e) = fs::create_dir_all(&dest) {
                return HttpResponse::json(
                    "500 Internal Server Error",
                    &json!({ "message": format!("create archive target dir: {e}") }),
                );
            }
            let mut archive = tar::Archive::new(Cursor::new(body));
            if let Err(e) = archive.unpack(&dest) {
                return HttpResponse::json(
                    "400 Bad Request",
                    &json!({ "message": format!("invalid tar archive: {e}") }),
                );
            }
            HttpResponse::empty("200 OK")
        }
        "GET" => {
            let upper_target = safe_join_rootfs(&upper_dir, &target_rel).ok();
            let src = match upper_target {
                Some(p) if p.exists() => p,
                _ => match safe_join_rootfs(&rootfs_dir, &target_rel) {
                    Ok(p) if p.exists() => p,
                    _ => {
                        return HttpResponse::json(
                            "404 Not Found",
                            &json!({ "message": format!("path '{target_rel}' not found") }),
                        );
                    }
                },
            };
            let mut tar_bytes = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut tar_bytes);
                let res = if src.is_dir() {
                    builder.append_dir_all(".", &src)
                } else {
                    let name = src
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file");
                    builder.append_path_with_name(&src, name)
                };
                if let Err(e) = res.and_then(|()| builder.finish()) {
                    return HttpResponse::json(
                        "500 Internal Server Error",
                        &json!({ "message": format!("pack archive: {e}") }),
                    );
                }
            }
            HttpResponse {
                status: "200 OK",
                content_type: "application/x-tar",
                body: tar_bytes,
            }
        }
        _ => HttpResponse::json("405 Method Not Allowed", &json!({ "message": "method not allowed" })),
    }
}

/// Handles `POST /build?t=<tag>&dockerfile=<Dockerfile>&nocache=<0|1>&platform=<plat>`
/// accepting a `.tar` build context in the HTTP request body.
pub(super) async fn handle_docker_build(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    query: &str,
    body: &[u8],
) -> HttpResponse {
    let tag = parse_query_param(query, "t")
        .map(percent_decode_simple)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("coral-build-{}", chrono::Utc::now().timestamp_millis()));
    let dockerfile_rel = parse_query_param(query, "dockerfile")
        .map(percent_decode_simple)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Dockerfile".into());
    let no_cache = parse_query_param(query, "nocache")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let platform = parse_query_param(query, "platform")
        .map(percent_decode_simple)
        .filter(|s| !s.trim().is_empty());

    let tmp_ctx = svc
        .cfg
        .state_dir
        .join("docker-build-tmp")
        .join(format!("ctx-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
    if let Err(e) = fs::create_dir_all(&tmp_ctx) {
        return HttpResponse::json(
            "500 Internal Server Error",
            &json!({ "message": format!("create build tmp dir: {e}") }),
        );
    }

    let mut archive = tar::Archive::new(Cursor::new(body));
    if let Err(e) = archive.unpack(&tmp_ctx) {
        let _ = images::remove_dir_force(&tmp_ctx);
        return HttpResponse::json(
            "400 Bad Request",
            &json!({ "message": format!("invalid build context tar: {e}") }),
        );
    }

    let df_path: PathBuf = tmp_ctx.join(&dockerfile_rel);
    let Ok(df_content) = fs::read_to_string(&df_path) else {
        let _ = images::remove_dir_force(&tmp_ctx);
        return HttpResponse::json(
            "400 Bad Request",
            &json!({ "message": format!("Dockerfile '{dockerfile_rel}' not found in build context") }),
        );
    };

    {
        let store = svc.store.read().await;
        svc.persist_images(&store.images);
    }

    let build_res = crate::builder::build_image(crate::builder::BuildOptions {
        tag: &tag,
        dockerfile_content: &df_content,
        context_dir: Some(&tmp_ctx),
        images_dir: &svc.images_dir,
        state_dir: &svc.cfg.state_dir,
        coral_bin: &svc.cfg.coral_bin,
        no_cache,
        target: None,
        build_args: &[],
        platform: platform.as_deref(),
    })
    .await;
    let _ = images::remove_dir_force(&tmp_ctx);

    match build_res {
        Ok(stored) => {
            let id = stored.id.clone();
            {
                let mut store = svc.store.write().await;
                store.images = images::load_index(&svc.images_dir);
                let orphaned = images::upsert_image(&mut store.images, stored);
                for old in orphaned {
                    svc.cleanup_unused_managed_dir(&store.images, &old);
                }
                svc.persist_images(&store.images);
                svc.invalidate_images_size_cache();
            }
            compat
                .record_event("image", "build", &id, json!({ "name": tag }))
                .await;
            let body = format!(
                "{}\n{}\n{}\n",
                json!({ "stream": format!("Successfully built {id}\n") }),
                json!({ "aux": { "ID": id } }),
                json!({ "stream": format!("Successfully tagged {tag}\n") }),
            );
            HttpResponse {
                status: "200 OK",
                content_type: "application/json",
                body: body.into_bytes(),
            }
        }
        Err(e) => HttpResponse::json(
            "500 Internal Server Error",
            &json!({ "error": format!("{e:#}") }),
        ),
    }
}

fn percent_decode_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(hex) = std::str::from_utf8(&b[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            out.push(val as char);
            i += 3;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

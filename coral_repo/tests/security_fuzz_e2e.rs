//! Deterministic adversarial / fuzz-corpus regression tests for Coral's parsers and
//! Prometheus `/metrics` & `/healthz` observability endpoints.

use coral_daemon::images::extract::extract_layer;
use coral_daemon::service::prometheus::{render_prometheus_metrics, serve_prometheus_metrics};
use coral_daemon::{CoralDaemonConfig, CoralDaemonService};
use coral_oci::OciSpec;
use coral_oci::state::validate_container_id;
use coral_vmm::virtio_mmio::{VirtQueue, VirtioMmioTransport};
use coral_wasm::http::{WasmHttpHandler, WasmHttpOptions, parse_cgi_or_raw_response};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::path::PathBuf;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

fn next_prng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn fuzz_corpus_oci_spec_and_container_ids_never_panic() {
    let malicious_ids = [
        "",
        ".",
        "..",
        "../etc/passwd",
        "foo/bar",
        "bad id",
        "\0null",
        &"a".repeat(512),
    ];
    for id in malicious_ids {
        assert!(validate_container_id(id).is_err());
    }

    let valid_json = serde_json::to_vec(&OciSpec::default_spec(true, 1000, 1000)).unwrap();
    let mut seed = 0x5A17_C0DA_2026_1120u64;
    for i in 0..120 {
        let mut mutated = valid_json.clone();
        if i % 3 == 0 {
            mutated.truncate(next_prng(&mut seed) as usize % mutated.len().max(1));
        } else {
            for _ in 0..8 {
                let idx = next_prng(&mut seed) as usize % mutated.len().max(1);
                mutated[idx] ^= (next_prng(&mut seed) & 0xFF) as u8;
            }
        }
        if let Ok(spec) = serde_json::from_slice::<OciSpec>(&mutated) {
            let _ = spec.validate();
        }
    }
}

#[test]
fn fuzz_corpus_tar_layer_rejects_traversal_and_corrupt_streams() {
    let tmp = tempdir().unwrap();

    // 1. Crafted tar containing `../../escaped` path traversal entry must be blocked/sanitized.
    let escape_blob = tmp.path().join("escape.tar.gz");
    {
        let f = std::fs::File::create(&escape_blob).unwrap();
        let gz = GzEncoder::new(f, Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(4);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        let _ = hdr.set_path("../outside-escape.txt");
        let _ = tar.append(&hdr, &b"evil"[..]);
        let _ = tar.finish();
    }
    let rootfs = tmp.path().join("rootfs");
    let _ = extract_layer(
        &escape_blob,
        &rootfs,
        "application/vnd.oci.image.layer.v1.tar+gzip",
    );
    assert!(!tmp.path().join("outside-escape.txt").exists());

    // 2. Random/truncated gzip/zstd/raw byte mutations never panic.
    let mut seed = 0xDEAD_BEEF_CAFE_0022u64;
    let blob = tmp.path().join("fuzz.blob");
    let mts = [
        "application/vnd.oci.image.layer.v1.tar+gzip",
        "application/vnd.oci.image.layer.v1.tar+zstd",
        "application/vnd.oci.image.layer.v1.tar",
    ];
    for iter in 0..60 {
        let len = (next_prng(&mut seed) % 256) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (next_prng(&mut seed) & 0xFF) as u8).collect();
        std::fs::write(&blob, &bytes).unwrap();
        let _ = extract_layer(&blob, &rootfs, mts[iter % mts.len()]);
    }
}

#[test]
fn fuzz_corpus_wasm_http_cgi_and_virtio_rings_are_panic_free() {
    let cgi_samples: &[&[u8]] = &[
        b"",
        b"\r\n\r\n",
        b"Status: 999 Weird\r\nContent-Type: text/plain\r\n\r\nhi",
        b"Status: abc\nX-Bad: \xff\xfe\n\nbody",
        b"InvalidHeaderWithoutColon\r\n\r\n",
    ];
    for sample in cgi_samples {
        let _ = parse_cgi_or_raw_response(sample);
    }

    let mut seed = 0x1337_F005_9000_0001u64;
    for _ in 0..64 {
        let len = (next_prng(&mut seed) % 512) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (next_prng(&mut seed) & 0xFF) as u8).collect();
        let _ = parse_cgi_or_raw_response(&bytes);
        let _ = WasmHttpHandler::from_bytes(
            &bytes,
            WasmHttpOptions {
                fuel_limit: 10_000,
                memory_limit_mb: 1,
                extra_envs: vec![],
            },
        );

        let mut mem = bytes.clone();
        if mem.len() < 128 {
            mem.resize(128, 0xA5);
        }
        let mut q = VirtQueue {
            ready: true,
            num: 8,
            desc_addr: (next_prng(&mut seed) % 64) * 4,
            avail_addr: (next_prng(&mut seed) % 64) * 4,
            used_addr: (next_prng(&mut seed) % 64) * 4,
            last_avail_idx: next_prng(&mut seed) as u16,
            last_used_idx: next_prng(&mut seed) as u16,
        };
        if let Some(ch) = q.pop_avail_chain(&mem) {
            let _ = q.push_used(&mut mem, ch.head, ch.segments.len() as u32);
        }
        let mut tr = VirtioMmioTransport::new(1, 0, 0, 2, vec![0u8; 32]);
        let mut out = [0u8; 4];
        tr.write_mmio(next_prng(&mut seed) % 0x140, &[0xFF; 4]);
        tr.read_mmio(next_prng(&mut seed) % 0x140, &mut out);
    }
}

#[tokio::test]
async fn prometheus_metrics_and_healthz_endpoints_e2e() {
    let dir = tempdir().unwrap();
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("state"),
        images_dir: Some(dir.path().join("images")),
        coral_bin: PathBuf::from("coral"),
        default_rootfs: None,
    });
    let snapshot = render_prometheus_metrics(&svc).await;
    assert!(snapshot.contains("coral_daemon_info"));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = watch::channel(false);
    let srv = tokio::spawn(serve_prometheus_metrics(svc, listener, rx));

    let mut s1 = TcpStream::connect(addr).await.unwrap();
    s1.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut r1 = String::new();
    s1.read_to_string(&mut r1).await.unwrap();
    assert!(r1.contains("200 OK") && r1.ends_with("ok\n"));

    let mut s2 = TcpStream::connect(addr).await.unwrap();
    s2.write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut r2 = String::new();
    s2.read_to_string(&mut r2).await.unwrap();
    assert!(r2.contains("coral_containers_total"));
    assert!(r2.contains("coral_pods_total"));
    assert!(r2.contains("coral_daemon_auto_gc_runs_total"));

    let _ = tx.send(true);
    let _ = srv.await;
}

#[tokio::test]
async fn cri_daemon_nextgen_security_and_auto_gc_actions_e2e() {
    use coral_api::runtime_service_server::RuntimeService;
    use coral_api::{PodSandboxConfig, PodSandboxMetadata, RunPodSandboxRequest};
    use std::collections::HashMap;
    use tonic::Request;

    let dir = tempdir().unwrap();
    let rootfs = dir.path().join("sample-rootfs");
    std::fs::create_dir_all(rootfs.join("bin")).unwrap();
    std::fs::write(rootfs.join("bin/sh"), b"#!/bin/sh\n").unwrap();

    let images_dir = dir.path().join("images");
    let priv_key = dir.path().join("test.key");
    let pub_key = dir.path().join("test.pub");

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("state"),
        images_dir: Some(images_dir.clone()),
        coral_bin: PathBuf::from("coral"),
        default_rootfs: None,
        });
    let mut stored = coral_daemon::images::register_local_rootfs("test-sec:v22", &rootfs).unwrap();
    stored.managed = false;
    coral_daemon::images::save_index(&images_dir, std::slice::from_ref(&stored)).unwrap();
    // Reconstruct svc so stored image index is loaded cleanly via public API.
    drop(svc);
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("state"),
        images_dir: Some(images_dir),
        coral_bin: PathBuf::from("coral"),
        default_rootfs: None,
    });

    let sys_call = |ann: HashMap<String, String>| {
        let svc = svc.clone();
        async move {
            svc.run_pod_sandbox(Request::new(RunPodSandboxRequest {
                config: Some(PodSandboxConfig {
                    metadata: Some(PodSandboxMetadata {
                        name: "sys".into(),
                        uid: "sys".into(),
                        namespace: "coral-system".into(),
                        attempt: 0,
                    }),
                    annotations: ann,
                    ..PodSandboxConfig::default()
                }),
                runtime_handler: String::new(),
            }))
            .await
            .expect("sys action")
            .into_inner()
            .pod_sandbox_id
        }
    };

    let sig_json = sys_call(HashMap::from([
        ("coral.system.action".into(), "image_sign".into()),
        ("coral.image.ref".into(), "test-sec:v22".into()),
        ("coral.image.key".into(), priv_key.display().to_string()),
        ("coral.image.pub_key".into(), pub_key.display().to_string()),
    ]))
    .await;
    assert!(sig_json.contains("Ed25519"));

    let ver_json = sys_call(HashMap::from([
        ("coral.system.action".into(), "image_verify".into()),
        ("coral.image.ref".into(), "test-sec:v22".into()),
        ("coral.image.pub_key".into(), pub_key.display().to_string()),
    ]))
    .await;
    assert!(ver_json.contains("\"verified\":true"));

    let sbom_json = sys_call(HashMap::from([
        ("coral.system.action".into(), "image_sbom".into()),
        ("coral.image.ref".into(), "test-sec:v22".into()),
    ]))
    .await;
    assert!(sbom_json.contains("SPDX-2.3"));

    let gc_json = sys_call(HashMap::from([("coral.system.action".into(), "auto_gc".into())])).await;
    assert!(gc_json.contains("bytesReclaimed"));
}

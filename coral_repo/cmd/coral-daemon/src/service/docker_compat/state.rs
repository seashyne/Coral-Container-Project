//! In-memory state for the Docker Engine REST API compatibility shim:
//! `/exec/{id}` instance tracking and `/events` ring buffer.

use chrono::Utc;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

const MAX_RETAINED_EVENTS: usize = 128;

#[derive(Debug, Clone)]
pub(super) struct DockerExecRecord {
    pub(super) id: String,
    pub(super) container_id: String,
    pub(super) cmd: Vec<String>,
    pub(super) tty: bool,
    pub(super) running: bool,
    pub(super) exit_code: i32,
}

#[derive(Debug, Default)]
pub struct DockerCompatState {
    seq: AtomicU64,
    pub(super) execs: RwLock<HashMap<String, DockerExecRecord>>,
    events: RwLock<VecDeque<Value>>,
}

impl DockerCompatState {
    pub fn next_exec_id(&self) -> String {
        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let ts = Utc::now().timestamp_millis();
        format!("exec-{ts}-{n}")
    }

    pub async fn record_event(&self, typ: &str, action: &str, actor_id: &str, attrs: Value) {
        let now = Utc::now();
        let ev = json!({
            "Type": typ,
            "Action": action,
            "status": action,
            "id": actor_id,
            "Actor": {
                "ID": actor_id,
                "Attributes": attrs,
            },
            "scope": "local",
            "time": now.timestamp(),
            "timeNano": now.timestamp_nanos_opt().unwrap_or(0),
        });
        let mut ring = self.events.write().await;
        if ring.len() >= MAX_RETAINED_EVENTS {
            ring.pop_front();
        }
        ring.push_back(ev);
    }

    pub async fn snapshot_events(&self, since_epoch: Option<i64>) -> Vec<Value> {
        let ring = self.events.read().await;
        ring.iter()
            .filter(|ev| {
                since_epoch.is_none_or(|s| {
                    ev.get("time").and_then(|v| v.as_i64()).unwrap_or(0) >= s
                })
            })
            .cloned()
            .collect()
    }
}

/// Encodes stdout/stderr payloads using Docker Engine's 8-byte multiplexed stream header
/// (`[stream_type, 0, 0, 0, len_be32...]`) when `tty == false`.
pub(super) fn encode_docker_stream_frame(stream_type: u8, payload: &[u8], tty: bool) -> Vec<u8> {
    if payload.is_empty() {
        return Vec::new();
    }
    if tty {
        return payload.to_vec();
    }
    let mut out = Vec::with_capacity(8 + payload.len());
    out.push(stream_type);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

//! Tauri full-duplex interactive PTY streaming commands (`pty_open`, `pty_write`,
//! `pty_resize`, `pty_close`) backed by CRI v1 `RuntimeService::ExecStream`.

use super::{AppState, err};
use coral_api::ExecStreamRequest;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

static PTY_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PtyOutputEvent {
    session_id: String,
    data: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PtyExitEvent {
    session_id: String,
    exit_code: i32,
}

#[tauri::command]
pub async fn pty_open(
    app: AppHandle,
    state: State<'_, AppState>,
    container_id: String,
    cols: Option<u32>,
    rows: Option<u32>,
    cmd: Option<Vec<String>>,
) -> Result<String, String> {
    let mut client = state.runtime().await?;
    let (tx, rx) = mpsc::channel::<ExecStreamRequest>(64);

    let init_req = ExecStreamRequest {
        container_id,
        cmd: cmd.unwrap_or_else(|| vec!["/bin/sh".into()]),
        cols: cols.unwrap_or(80),
        rows: rows.unwrap_or(24),
        stdin: Vec::new(),
        resize: false,
    };
    tx.send(init_req).await.map_err(err)?;

    let mut inbound = client
        .exec_stream(ReceiverStream::new(rx))
        .await
        .map_err(err)?
        .into_inner();

    let session_id = format!("pty-{}", PTY_SEQ.fetch_add(1, Ordering::Relaxed));
    state
        .pty_sessions
        .lock()
        .await
        .insert(session_id.clone(), tx);

    let sid = session_id.clone();
    let app_clone = app.clone();
    tokio::spawn(async move {
        let mut final_code = 0i32;
        while let Ok(Some(msg)) = inbound.message().await {
            if !msg.data.is_empty() {
                let text = String::from_utf8_lossy(&msg.data).into_owned();
                let _ = app_clone.emit(
                    "pty-output",
                    PtyOutputEvent {
                        session_id: sid.clone(),
                        data: text,
                    },
                );
            }
            if msg.exited {
                final_code = msg.exit_code;
                break;
            }
        }
        let _ = app_clone.emit(
            "pty-exit",
            PtyExitEvent {
                session_id: sid,
                exit_code: final_code,
            },
        );
    });

    Ok(session_id)
}

#[tauri::command]
pub async fn pty_write(
    state: State<'_, AppState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let tx = state
        .pty_sessions
        .lock()
        .await
        .get(&session_id)
        .cloned()
        .ok_or_else(|| "PTY session is not active".to_string())?;
    tx.send(ExecStreamRequest {
        container_id: String::new(),
        cmd: Vec::new(),
        cols: 0,
        rows: 0,
        stdin: data.into_bytes(),
        resize: false,
    })
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn pty_resize(
    state: State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let Some(tx) = state.pty_sessions.lock().await.get(&session_id).cloned() else {
        return Ok(());
    };
    let _ = tx
        .send(ExecStreamRequest {
            container_id: String::new(),
            cmd: Vec::new(),
            cols,
            rows,
            stdin: Vec::new(),
            resize: true,
        })
        .await;
    Ok(())
}

#[tauri::command]
pub async fn pty_close(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.pty_sessions.lock().await.remove(&session_id);
    Ok(())
}

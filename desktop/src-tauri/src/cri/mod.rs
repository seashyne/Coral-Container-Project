//! Tauri CRI v1 gRPC client state, daemon connection lifecycle, and re-exported commands.

pub mod compose;
pub mod containers;
pub mod images;
pub mod nextgen;
pub mod pods;
pub mod pty;
pub mod system;

pub use compose::*;
pub use containers::*;
pub use images::*;
pub use nextgen::*;
pub use pods::*;
pub use pty::*;
pub use system::*;

use crate::daemon::{self, SpawnedDaemon};
use crate::settings::{self, Settings};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, ExecStreamRequest, VersionRequest};
use serde::Serialize;
use std::collections::HashMap;
use tauri::{AppHandle, State};
use tokio::sync::{Mutex, mpsc};
use tonic::transport::Channel;

pub struct Connection {
    pub runtime: RuntimeServiceClient<Channel>,
    pub image: ImageServiceClient<Channel>,
    pub endpoint: String,
    pub version: String,
    pub spawned: Option<SpawnedDaemon>,
}

#[derive(Default)]
pub struct AppState {
    pub settings: Mutex<Option<Settings>>,
    pub conn: Mutex<Option<Connection>>,
    pub pty_sessions: Mutex<HashMap<String, mpsc::Sender<ExecStreamRequest>>>,
}

impl AppState {
    pub(crate) async fn settings(&self) -> Settings {
        self.settings.lock().await.clone().unwrap_or_default()
    }

    pub(crate) async fn runtime(&self) -> Result<RuntimeServiceClient<Channel>, String> {
        self.conn
            .lock()
            .await
            .as_ref()
            .map(|c| c.runtime.clone())
            .ok_or_else(|| "not connected — connect to a daemon first".into())
    }

    pub(crate) async fn image(&self) -> Result<ImageServiceClient<Channel>, String> {
        self.conn
            .lock()
            .await
            .as_ref()
            .map(|c| c.image.clone())
            .ok_or_else(|| "not connected — connect to a daemon first".into())
    }
}

pub(crate) fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

pub(crate) fn container_state_name(state: i32) -> &'static str {
    match ContainerState::try_from(state) {
        Ok(ContainerState::ContainerCreated) => "created",
        Ok(ContainerState::ContainerRunning) => "running",
        Ok(ContainerState::ContainerExited) => "exited",
        _ => "unknown",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub connected: bool,
    pub endpoint: String,
    pub mode: String,
    pub runtime_version: String,
    pub attached: bool,
}

#[tauri::command]
pub async fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<Settings, String> {
    let mut guard = state.settings.lock().await;
    if guard.is_none() {
        *guard = Some(settings::load(&app));
    }
    Ok(guard.clone().unwrap())
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    new_settings: Settings,
) -> Result<(), String> {
    if new_settings.endpoint.trim().is_empty() {
        return Err("endpoint must not be empty".into());
    }
    settings::save(&app, &new_settings)?;
    *state.settings.lock().await = Some(new_settings);
    disconnect_inner(&state).await;
    Ok(())
}

async fn disconnect_inner(state: &AppState) {
    let mut guard = state.conn.lock().await;
    if let Some(mut conn) = guard.take()
        && let Some(spawned) = conn.spawned.as_mut()
        && spawned.mode == crate::settings::Mode::Local
    {
        let _ = spawned.child.kill().await;
    }
}

#[tauri::command]
pub async fn connect(state: State<'_, AppState>) -> Result<ConnectionInfo, String> {
    let settings = state.settings().await;
    disconnect_inner(&state).await;

    let spawned = daemon::ensure(&settings).await?;
    let channel = Channel::from_shared(format!("http://{}", settings.endpoint))
        .map_err(err)?
        .connect_timeout(std::time::Duration::from_secs(5))
        .connect()
        .await
        .map_err(|e| format!("connect to {}: {e}", settings.endpoint))?;

    let mut runtime = RuntimeServiceClient::new(channel.clone());
    let version = runtime
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .map_err(err)?
        .into_inner();

    let info = ConnectionInfo {
        connected: true,
        endpoint: settings.endpoint.clone(),
        mode: format!("{:?}", settings.mode).to_lowercase(),
        runtime_version: version.runtime_version.clone(),
        attached: spawned.is_none(),
    };

    *state.conn.lock().await = Some(Connection {
        runtime,
        image: ImageServiceClient::new(channel),
        endpoint: settings.endpoint,
        version: version.runtime_version,
        spawned,
    });
    Ok(info)
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    disconnect_inner(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn connection_status(state: State<'_, AppState>) -> Result<ConnectionInfo, String> {
    let guard = state.conn.lock().await;
    let settings = state.settings().await;
    Ok(match guard.as_ref() {
        Some(conn) => ConnectionInfo {
            connected: true,
            endpoint: conn.endpoint.clone(),
            mode: format!("{:?}", settings.mode).to_lowercase(),
            runtime_version: conn.version.clone(),
            attached: conn.spawned.is_none(),
        },
        None => ConnectionInfo {
            connected: false,
            endpoint: settings.endpoint,
            mode: format!("{:?}", settings.mode).to_lowercase(),
            runtime_version: String::new(),
            attached: false,
        },
    })
}

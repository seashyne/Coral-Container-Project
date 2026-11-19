use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Local,
    Wsl2,
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub mode: Mode,
    pub endpoint: String,
    pub daemon_bin: String,
    pub coral_bin: String,
    pub rootfs: String,
    pub images_dir: String,
    pub state_dir: String,
    pub wsl_distro: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: if cfg!(windows) {
                Mode::Wsl2
            } else {
                Mode::Local
            },
            endpoint: "127.0.0.1:50051".into(),
            daemon_bin: "coral-daemon".into(),
            coral_bin: "coral".into(),
            rootfs: String::new(),
            images_dir: String::new(),
            state_dir: String::new(),
            wsl_distro: String::new(),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("resolve config dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create config dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("write settings: {e}"))
}

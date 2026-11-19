mod cri;
mod daemon;
mod settings;

use cri::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            cri::get_settings,
            cri::save_settings,
            cri::connect,
            cri::disconnect,
            cri::connection_status,
            cri::list_containers,
            cri::run_container,
            cri::stop_container,
            cri::remove_container,
            cri::container_detail,
            cri::exec_container,
            cri::list_pods,
            cri::stop_pod,
            cri::remove_pod,
            cri::list_images,
            cri::pull_image,
            cri::remove_image,
            cri::commit_container,
            cri::build_image,
            cri::tag_image,
            cri::save_image,
            cri::load_image,
            cri::compose_up,
            cri::compose_down,
            cri::system_df,
            cri::system_prune,
            cri::list_volumes,
            cri::create_volume,
            cri::remove_volume,
            cri::registry_login,
            cri::registry_logout,
            cri::push_image,
            cri::list_networks,
            cri::create_network,
            cri::remove_network,
            cri::pty_open,
            cri::pty_write,
            cri::pty_resize,
            cri::pty_close,
            cri::vmm_probe,
            cri::wasm_run,
            cri::sandbox_run,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Coral Desktop")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                if let Ok(mut guard) = state.conn.try_lock()
                    && let Some(conn) = guard.as_mut()
                    && let Some(spawned) = conn.spawned.as_mut()
                    && spawned.mode == settings::Mode::Local
                {
                    let _ = spawned.child.start_kill();
                }
            }
        });
}

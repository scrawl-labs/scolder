mod audio;
mod commands;
mod decode;
mod dsp;
mod feedback;
mod mic_permission;
mod ollama;
mod session;
mod setup;
mod transcribe;
mod turns;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_macos_permissions::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::run_setup,
            commands::start_session,
            commands::start_session_from_file,
            commands::stop_session,
            commands::get_utterances,
            commands::get_transcript,
            commands::get_recording_status,
            commands::list_sessions,
            commands::get_stats,
            commands::get_session_info,
            commands::read_session,
            commands::read_session_utterances,
            commands::session_has_utterances_json,
            commands::read_session_transcript,
            commands::rename_session,
            commands::delete_session,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 앱 종료 시 백그라운드로 띄워둔 Ollama 서버도 같이 종료 - 그렇지 않으면
            // 앱을 껐다 켜도 이전 프로세스가 포트를 잡고 있는 고아 프로세스가 됨
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let state = app_handle.state::<AppState>();
                let child = state.ollama_process.lock().unwrap().take();
                if let Some(mut child) = child {
                    let _ = child.kill();
                }
            }
        });
}

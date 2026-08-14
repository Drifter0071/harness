mod agent;
mod agent_loop;
mod capabilities;
mod compact;
mod commands;
mod hci;
mod health_agent;
mod llm_client;
mod models;
mod personalities;
mod secrets;
mod session;
mod session_watcher;
mod tools;
mod web_search;

use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // Initialize state. The secret store uses the OS keyring
            // in production; tests swap in `InMemoryStore` via a
            // different `run()` entry point. Tauri commands take
            // `State<'_, AppState>` directly, so we manage the bare
            // type; HCI gets a clone to share the same instance.
            // `AppState: Clone` so this is safe.
            let state = commands::AppState::new(Arc::new(secrets::KeyringStore::new()));
            let state_for_hci = state.clone();
            app.manage(state);

            // HCI: start the control-interface server in a background
            // task. The server handle is stored in the Tauri state so
            // its lifetime is tied to the app (the previous `let
            // _server = ...` pattern dropped it instantly, taking the
            // listener and the conn-info file with it).
            tauri::async_runtime::spawn(async move {
                match hci::integration::start(Arc::new(state_for_hci)).await {
                    Ok(Some(server)) => {
                        // Leak the server handle: it owns the TcpListener
                        // and the conn-info file. The OS will reclaim
                        // both when the process exits.
                        Box::leak(Box::new(server));
                    }
                    Ok(None) => {} // HCI disabled, normal.
                    Err(e) => {
                        eprintln!("[hci] failed to start: {e}");
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Workspace commands
            commands::read_file,
            commands::write_file,
            commands::list_directory,
            commands::search_files,
            commands::create_directory,
            // CLI execution
            commands::execute_command,
            commands::kill_process,
            // Agent commands
            commands::spawn_agent,
            commands::send_agent_message,
            commands::get_agent_status,
            commands::list_active_agents,
            commands::stop_agent,
            commands::health_check,
            // Web search
            commands::web_search,
            // Saved models + per-model prefs (Phase 2.0 spec)
            commands::list_saved_models,
            commands::save_saved_model,
            commands::delete_saved_model,
            commands::get_active_model,
            commands::set_active_model,
            commands::get_model_prefs,
            commands::set_model_prefs,
            commands::test_saved_model,
            commands::reset_secret_store,
            commands::probe_saved_model,
            // Chat streaming (Phase 2.0 spec)
            commands::chat_complete,
            commands::cancel_chat,
            // Session file watcher (live reload from disk)
            commands::watch_session,
            // Sessions + recents + project opening (Phase 1.5 spec)
            commands::pick_project_folder,
            commands::list_sessions,
            commands::load_active_session,
            commands::create_session,
            commands::load_session,
            commands::save_session,
            commands::rename_session,
            commands::delete_session,
            commands::set_active_session,
            commands::set_session_mode,
            commands::set_session_model,
            commands::compact_chat,
            commands::list_recent_projects,
            commands::forget_recent_project,
            commands::touch_recent_project,
            commands::list_files_recursive,
            // Global agent personalities (this commit)
            commands::load_personalities,
            commands::save_personalities,
            commands::get_personality,
            // Agent turn loop (agent-sessions spec)
            commands::run_agent_turn,
            commands::respond_approval,
            commands::cancel_agent,
            // Subagent dispatch (registry introspection for the canvas)
            commands::list_subagents,
            // Bip mascot state (bip-mascot-tool spec)
            commands::get_bip_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Harness");
}

mod agent;
mod agent_loop;
mod agents;
mod commands;
mod llm_client;
mod models;
mod secrets;
mod session;
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
            // different `run()` entry point.
            let state = commands::AppState::new(Arc::new(secrets::KeyringStore::new()));
            app.manage(state);
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
            // Chat streaming (Phase 2.0 spec)
            commands::chat_complete,
            commands::cancel_chat,
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
            commands::list_recent_projects,
            commands::forget_recent_project,
            commands::touch_recent_project,
            commands::list_files_recursive,
            // Agent roster + turn loop (agent-sessions spec)
            commands::load_agent_roster,
            commands::save_agent_roster,
            commands::run_agent_turn,
            commands::respond_approval,
            commands::cancel_agent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Harness");
}

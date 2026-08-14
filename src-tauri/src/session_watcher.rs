//! File watcher for the session directory.
//!
//! Watches `<workspace>/.harness/sessions/` for write events. When a
//! file changes (because the frontend's `flushPendingSave` wrote it,
// OR because the agent loop's `persist` wrote it), emits a
//! `session-file-changed` Tauri event. The frontend listens for that
//! and reloads the active session from disk, so the chat UI updates
//! in real time even when the streaming `chat-chunk` events aren't
//! reaching the WebView.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

/// One watcher per active workspace. Stops when `stop()` is called.
pub struct SessionWatcher {
    /// Keep the watcher alive; dropping it stops the OS-level watch.
    _watcher: RecommendedWatcher,
    /// Held to allow stopping from another task.
    stop_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl SessionWatcher {
    /// Start watching the sessions directory under `workspace`.
    /// The directory is `<workspace>/.harness/sessions/`. If the
    /// directory doesn't exist yet, the watcher is set up anyway and
    /// will catch it when it appears (notify creates the watch on
    /// the parent + filters on the basename).
    pub fn start(app: AppHandle, workspace: PathBuf) -> Result<Self, String> {
        let sessions_dir = workspace.join(".harness").join("sessions");
        std::fs::create_dir_all(&sessions_dir).map_err(|e| format!("create sessions dir: {e}"))?;

        let app_for_event = app.clone();
        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                // Only forward Create + Modify(Data) on regular files.
                // Modify(Name) (rename) and Remove are noise.
                let is_data_change = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(ModifyKind::Data(_))
                );
                if !is_data_change {
                    return;
                }
                for path in &event.paths {
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    eprintln!("[hci:debug] session file changed: {}", path.display());
                    let _ = app_for_event.emit(
                        "session-file-changed",
                        json!({ "path": path.to_string_lossy() }),
                    );
                }
            })
            .map_err(|e| format!("watcher init: {e}"))?;

        // Watch the parent so we catch new files too.
        let watch_target = sessions_dir.parent().unwrap_or(Path::new("."));
        watcher
            .watch(watch_target, RecursiveMode::Recursive)
            .map_err(|e| format!("watch: {e}"))?;

        // (We don't actually need the stop channel for the current
        // design — the watcher is dropped with the app — but the
        // field is here for symmetry with future "stop watching on
        // project switch" use cases.)
        let (stop_tx, _stop_rx) = tokio::sync::oneshot::channel();
        let stop_tx = Arc::new(Mutex::new(Some(stop_tx)));

        // Touch a file in 100ms if the dir is empty, just to confirm
        // the watcher is alive (debug aid).
        let probe_dir = sessions_dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = std::fs::create_dir_all(&probe_dir);
        });

        Ok(Self {
            _watcher: watcher,
            stop_tx,
        })
    }
}

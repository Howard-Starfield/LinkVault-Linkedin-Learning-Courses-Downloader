use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

pub struct PendingUpdate(pub Mutex<Option<Update>>);

impl Default for PendingUpdate {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateMetadata {
    version: String,
    current_version: String,
}

#[tauri::command]
pub async fn check_for_app_update(
    app: AppHandle,
    pending_update: State<'_, PendingUpdate>,
) -> Result<Option<UpdateMetadata>, String> {
    let before_exit_handle = app.clone();
    let update = app
        .updater_builder()
        .on_before_exit(move || {
            if crate::shutdown_transient_workflow(&before_exit_handle) {
                crate::authorize_cooperative_exit(&before_exit_handle);
            }
        })
        .build()
        .map_err(|error| format!("Updater unavailable: {error}"))?
        .check()
        .await
        .map_err(|error| format!("Update check failed: {error}"))?;

    let metadata = update.as_ref().map(|update| UpdateMetadata {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
    });

    *pending_update
        .0
        .lock()
        .map_err(|_| "Update state is unavailable".to_string())? = update;

    Ok(metadata)
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    pending_update: State<'_, PendingUpdate>,
) -> Result<(), String> {
    let prepare_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::prepare_renderer_for_update(&prepare_handle)
    })
    .await
    .map_err(|error| format!("Update preparation failed: {error}"))??;

    if !crate::shutdown_transient_workflow(&app) {
        return Err("Timed out while stopping active YouTube work".to_string());
    }

    let update = pending_update
        .0
        .lock()
        .map_err(|_| "Update state is unavailable".to_string())?
        .take()
        .ok_or_else(|| "No pending update is available".to_string())?;

    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| format!("Update install failed: {error}"))?;

    Ok(())
}

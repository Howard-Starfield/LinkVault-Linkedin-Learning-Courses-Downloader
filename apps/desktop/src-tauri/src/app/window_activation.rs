//! Main-window activation shared by tray, blocked-exit, and second launch.

use tauri::{Emitter, Manager};

use super::cooperative_exit::ExitReason;

pub fn show_main_window(app: &tauri::AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    true
}

pub fn restore_main_window(app: &tauri::AppHandle, reason: ExitReason) {
    show_main_window(app);
    let _ = app.emit(
        "linkvault://exit-blocked",
        serde_json::json!({ "reason": reason }),
    );
}

pub fn activate_existing_instance(app: &tauri::AppHandle) {
    show_main_window(app);
    let _ = app.emit("linkvault://instance-activated", ());
}

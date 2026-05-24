pub mod artifact_downloader;
pub mod auth;
pub mod browser_cookies;
pub mod cache;
mod commands;
pub mod course;
pub mod download_orchestrator;
pub mod exercise_archive;
mod linkedin;
pub mod live_clients;
pub mod quality;
pub mod quiz_hints;
pub mod security;
pub mod token_store;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap_state,
            commands::cancel_active_download,
            commands::clear_failed_download_jobs,
            commands::clear_saved_li_at_token,
            commands::parse_linkedin_course_urls,
            commands::process_next_queued_download_from_browser_source,
            commands::process_next_queued_download_with_saved_token,
            commands::process_next_queued_download_with_li_at,
            commands::quality_fallback_order,
            commands::retry_failed_download_job,
            commands::save_download_preferences,
            commands::save_li_at_token,
            commands::start_download_jobs,
            commands::validate_browser_token_source,
            commands::validate_li_at_token
        ])
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;
            let db_path = app_data.join("linkvault.sqlite3");
            let connection = cache::open_or_initialize(&db_path)?;
            cache::reconcile_active_jobs_after_restart(
                &connection,
                commands::now_unix_timestamp(),
            )?;
            drop(connection);
            app.manage(commands::LinkVaultState::new(db_path));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LinkVault");
}

#![deny(unused)]

mod app_updates;
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
pub mod storage;
pub mod token_store;

// Coursera tab: fully isolated sibling. The Tauri command surface is
// registered below alongside the LinkedIn handlers. Per
// `docs/learning/agent-harness-coursera/ISOLATION_RULES.md`, the
// LinkedIn command surface in `commands::*` is unchanged.
pub mod coursera;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            app_updates::check_for_app_update,
            app_updates::install_app_update,
            commands::bootstrap_state,
            commands::cancel_active_download,
            commands::clear_failed_download_jobs,
            commands::clear_saved_li_at_token,
            commands::open_download_folder,
            commands::parse_linkedin_course_urls,
            commands::process_next_queued_download_from_browser_source,
            commands::process_next_queued_download_with_saved_token,
            commands::process_queued_download_batch_with_saved_token,
            commands::quality_fallback_order,
            commands::retry_failed_download_job,
            commands::save_download_preferences,
            commands::save_li_at_token,
            commands::start_download_jobs,
            coursera::commands::bootstrap_coursera_state,
            coursera::commands::parse_coursera_class_input,
            coursera::commands::coursera_login,
            coursera::commands::save_coursera_token,
            coursera::commands::clear_saved_coursera_token,
            coursera::commands::has_saved_coursera_token,
            coursera::commands::save_coursera_preferences,
            coursera::commands::load_coursera_preferences,
            coursera::commands::start_coursera_download_jobs,
            coursera::commands::process_next_queued_coursera_job,
            coursera::commands::process_queued_coursera_batch,
            coursera::commands::cancel_active_coursera_download,
            coursera::commands::retry_failed_coursera_job,
            coursera::commands::clear_failed_coursera_jobs,
            coursera::commands::list_coursera_history,
            coursera::commands::open_coursera_download_folder,
            coursera::commands::fetch_coursera_syllabus_preview,
        ])
        .setup(|app| {
            let db_path = storage::resolve_db_path()?;
            if let Some(data_dir) = db_path.parent() {
                let legacy_app_data = app.path().app_data_dir()?;
                storage::migrate_legacy_app_data(&legacy_app_data, data_dir)?;
            }
            let connection = cache::open_or_initialize(&db_path)?;
            cache::reconcile_active_jobs_after_restart(
                &connection,
                commands::now_unix_timestamp(),
            )?;
            drop(connection);
            app.manage(commands::LinkVaultState::new(db_path.clone()));
            app.manage(coursera::commands::CourseraState::new(db_path));
            app.manage(app_updates::PendingUpdate::default());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LinkVault");
}

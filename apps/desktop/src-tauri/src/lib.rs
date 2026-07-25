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
pub mod newspaper;

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
            commands::delete_completed_download,
            commands::download_scheduled_job_now,
            commands::open_download_folder,
            commands::parse_linkedin_course_urls,
            commands::process_next_queued_download_from_browser_source,
            commands::process_next_queued_download_with_saved_token,
            commands::process_queued_download_batch_with_saved_token,
            commands::quality_fallback_order,
            commands::remove_download_queue_item,
            commands::retry_failed_download_job,
            commands::save_download_preferences,
            commands::save_li_at_token,
            commands::set_all_downloads_paused,
            commands::set_download_job_pause,
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
            coursera::commands::remove_failed_coursera_job,
            coursera::commands::list_coursera_history,
            coursera::commands::open_coursera_download_folder,
            coursera::commands::fetch_coursera_syllabus_preview,
            newspaper::commands::bootstrap_newspaper_state,
            newspaper::commands::list_newspaper_catalog,
            newspaper::commands::refresh_newspaper_catalog,
            newspaper::commands::create_newspaper_batch,
            newspaper::commands::process_newspaper_queue,
            newspaper::commands::pause_newspaper_batch,
            newspaper::commands::cancel_newspaper_batch,
            newspaper::commands::retry_newspaper_job,
            newspaper::commands::list_newspaper_library,
            newspaper::commands::get_newspaper_reader_manifest,
            newspaper::commands::get_newspaper_preview,
            newspaper::commands::get_newspaper_page_image,
            newspaper::commands::open_newspaper_download_folder,
            newspaper::commands::import_existing_newspaper_archive,
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
            newspaper::storage::reconcile_after_restart(
                &connection,
                commands::now_unix_timestamp(),
            )?;
            drop(connection);
            app.manage(commands::LinkVaultState::new(db_path.clone()));
            app.manage(coursera::commands::CourseraState::new(db_path.clone()));
            app.manage(newspaper::commands::NewspaperState::new(db_path));
            app.manage(app_updates::PendingUpdate::default());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LinkVault");
}

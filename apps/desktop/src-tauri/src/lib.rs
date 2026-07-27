#![deny(unused)]

mod app;
mod providers;
pub mod workflow;

use app::updates as app_updates;
pub use app::{database as cache, security, storage};
pub use providers::linkedin::{
    artifact_downloader, auth, browser_cookies, course, download_orchestrator, exercise_archive,
    live_clients, quality, quiz_hints, token_store,
};
use providers::linkedin::{commands, linkedin};
pub use providers::{coursera, newspaper};

use tauri::Manager;

pub fn run() {
    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol(
            "newspaper-media",
            |context, request, responder| {
                let app = context.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let db_path = app
                        .state::<newspaper::commands::NewspaperState>()
                        .db_path()
                        .to_path_buf();
                    let cache_root = app
                        .state::<newspaper::thumbnails::ThumbnailCoordinator>()
                        .cache_root()
                        .to_path_buf();
                    let response = tauri::async_runtime::spawn_blocking(move || {
                        newspaper::media_protocol::handle_request(&db_path, &cache_root, &request)
                    })
                    .await
                    .unwrap_or_else(|_| {
                        tauri::http::Response::builder()
                            .status(tauri::http::StatusCode::INTERNAL_SERVER_ERROR)
                            .body(b"Newspaper media could not be loaded.".to_vec())
                            .expect("static protocol failure response must be valid")
                    });
                    responder.respond(response);
                });
            },
        )
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
            newspaper::commands::create_newspaper_schedule,
            newspaper::commands::toggle_newspaper_schedule,
            newspaper::commands::delete_newspaper_schedule,
            newspaper::commands::process_newspaper_queue,
            newspaper::commands::process_newspaper_optimization_queue,
            newspaper::commands::pause_newspaper_batch,
            newspaper::commands::cancel_newspaper_batch,
            newspaper::commands::retry_newspaper_job,
            newspaper::commands::set_newspaper_job_pause,
            newspaper::commands::reorder_newspaper_jobs,
            newspaper::commands::remove_newspaper_job,
            newspaper::commands::list_newspaper_library,
            newspaper::commands::get_newspaper_library_page,
            newspaper::commands::get_newspaper_activity_snapshot,
            newspaper::commands::get_newspaper_reader_manifest,
            newspaper::commands::save_newspaper_reading_progress,
            newspaper::commands::ensure_newspaper_thumbnail,
            newspaper::commands::open_newspaper_download_folder,
            newspaper::commands::import_existing_newspaper_archive,
            newspaper::commands::repair_newspaper_library,
        ])
        .setup(|app| {
            let db_path = storage::resolve_db_path()?;
            if let Some(data_dir) = db_path.parent() {
                let legacy_app_data = app.path().app_data_dir()?;
                storage::migrate_legacy_app_data(&legacy_app_data, data_dir)?;
            }
            let diagnostics = app::database_diagnostics::DatabaseDiagnostics::default();
            let (connection, _initialization) =
                cache::initialize_database_with_diagnostics(&db_path, &diagnostics)?;
            cache::reconcile_active_jobs_after_restart(
                &connection,
                commands::now_unix_timestamp(),
            )?;
            newspaper::storage::reconcile_after_restart(
                &connection,
                commands::now_unix_timestamp(),
            )?;
            drop(connection);
            let writer =
                app::database_writer::DatabaseWriter::start(db_path.clone(), diagnostics.clone())?;
            app.manage(diagnostics);
            app.manage(writer);
            app.manage(commands::LinkVaultState::new(db_path.clone()));
            app.manage(coursera::commands::CourseraState::new(db_path.clone()));
            app.manage(newspaper::thumbnails::ThumbnailCoordinator::new(
                db_path.clone(),
            ));
            app.manage(newspaper::commands::NewspaperState::new(db_path));
            newspaper::commands::schedule_page_dimension_backfill(app.handle());
            app.manage(app_updates::PendingUpdate::default());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building LinkVault");

    app.run(|handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            let _ = handle
                .state::<app::database_writer::DatabaseWriter>()
                .shutdown();
        }
    });
}

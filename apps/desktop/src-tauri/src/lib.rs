#![deny(unused)]

mod app;
#[cfg(feature = "crop-baseline")]
pub use app::newspaper_clipping_crop_baseline as crop_baseline;
#[cfg(feature = "durability-baseline")]
pub use app::newspaper_clipping_note_durability_baseline as durability_baseline;
mod providers;
pub mod workflow;
use app::cooperative_exit::{CooperativeExit, ExitReason, WaitOutcome};
use app::updates as app_updates;
use app::window_activation::{activate_existing_instance, restore_main_window, show_main_window};
pub use app::{database as cache, dpapi, managed_process, security, shell, storage};
pub use providers::linkedin::{
    artifact_downloader, auth, browser_cookies, course, download_orchestrator, exercise_archive,
    live_clients, quality, quiz_hints, token_store,
};
use providers::linkedin::{commands, executor, linkedin};
pub use providers::{coursera, newspaper, youtube};

use std::sync::{atomic::AtomicBool, Arc, Mutex};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{Emitter, Manager};

#[tauri::command]
fn resolve_cooperative_exit(
    state: tauri::State<'_, CooperativeExit>,
    token: u64,
    durable: bool,
) -> bool {
    state.resolve(token, durable)
}

fn request_cooperative_exit(app: &tauri::AppHandle, reason: ExitReason) {
    const EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
    let coordinator = app.state::<CooperativeExit>().inner().clone();
    let request = coordinator.request(reason);
    if !request.started {
        return;
    }
    if app
        .emit(
            "linkvault://prepare-exit",
            serde_json::json!({
                "token": request.token,
                "reason": reason,
                "deadlineMs": EXIT_TIMEOUT.as_millis()
            }),
        )
        .is_err()
    {
        coordinator.resolve(request.token, false);
    }
    let handle = app.clone();
    std::thread::spawn(
        move || match coordinator.wait(request.token, EXIT_TIMEOUT) {
            WaitOutcome::Durable(ExitReason::Close) => {
                let hidden = handle
                    .get_webview_window("main")
                    .is_some_and(|window| window.hide().is_ok());
                if !hidden {
                    restore_main_window(&handle, ExitReason::Close);
                }
            }
            WaitOutcome::Durable(ExitReason::Exit) => {
                coordinator.authorize_exit();
                handle.exit(0);
            }
            WaitOutcome::Blocked(reason) | WaitOutcome::TimedOut(reason) => {
                restore_main_window(&handle, reason);
            }
            WaitOutcome::Stale => {}
        },
    );
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            activate_existing_instance(app);
        }))
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
                    let clipping_service = app
                        .state::<newspaper::clipping_service::ClippingService>()
                        .inner()
                        .clone();
                    let response = tauri::async_runtime::spawn_blocking(move || {
                        newspaper::media_protocol::handle_request(
                            &db_path,
                            &cache_root,
                            &clipping_service,
                            &request,
                        )
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
            resolve_cooperative_exit,
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
            commands::reset_linkedin_database,
            commands::retry_failed_download_job,
            commands::save_download_preferences,
            commands::set_linkedin_video_wait_bounds,
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
            coursera::commands::reset_coursera_database,
            coursera::commands::list_coursera_history,
            coursera::commands::open_coursera_download_folder,
            coursera::commands::fetch_coursera_syllabus_preview,
            youtube::commands::get_youtube_helper_status,
            youtube::commands::get_youtube_preferences,
            youtube::commands::save_youtube_preferences,
            youtube::commands::scan_youtube_source,
            youtube::commands::inspect_youtube_transcripts,
            youtube::commands::cancel_youtube_discovery,
            youtube::commands::start_youtube_download,
            youtube::commands::get_youtube_download_state,
            youtube::commands::pause_youtube_download,
            youtube::commands::resume_youtube_download,
            youtube::commands::cancel_youtube_download,
            youtube::commands::open_youtube_download_folder,
            youtube::commands::list_youtube_history,
            newspaper::commands::bootstrap_newspaper_state,
            newspaper::commands::list_newspaper_catalog,
            newspaper::commands::refresh_newspaper_catalog,
            newspaper::commands::create_newspaper_batch,
            newspaper::commands::create_newspaper_clipping,
            newspaper::commands::create_newspaper_schedule,
            newspaper::commands::toggle_newspaper_schedule,
            newspaper::commands::delete_newspaper_schedule,
            newspaper::commands::process_newspaper_queue,
            newspaper::commands::process_newspaper_optimization_queue,
            newspaper::commands::pause_newspaper_batch,
            newspaper::commands::cancel_newspaper_batch,
            newspaper::commands::retry_newspaper_job,
            newspaper::commands::set_newspaper_job_pause,
            newspaper::commands::set_all_newspaper_jobs_paused,
            newspaper::commands::reorder_newspaper_jobs,
            newspaper::commands::reset_newspaper_database,
            newspaper::commands::remove_newspaper_job,
            newspaper::commands::list_newspaper_library,
            newspaper::commands::get_newspaper_library_page,
            newspaper::commands::get_newspaper_library_item,
            newspaper::commands::get_newspaper_activity_snapshot,
            newspaper::commands::get_newspaper_reader_manifest,
            newspaper::commands::save_newspaper_reading_progress,
            newspaper::commands::ensure_newspaper_thumbnail,
            newspaper::commands::get_newspaper_clippings_page,
            newspaper::commands::get_newspaper_clipping,
            newspaper::commands::update_newspaper_clipping,
            newspaper::commands::delete_newspaper_clipping,
            newspaper::commands::recover_newspaper_clipping_asset,
            newspaper::commands::checkpoint_newspaper_clipping_note,
            newspaper::commands::load_newspaper_clipping_note_recovery,
            newspaper::commands::claim_newspaper_clipping_note_recovery,
            newspaper::commands::discard_newspaper_clipping_note_recovery,
            newspaper::commands::ensure_newspaper_clipping_thumbnail,
            newspaper::commands::search_newspaper_clippings,
            newspaper::commands::search_possible_newspaper_clippings,
            newspaper::commands::list_newspaper_snapshot_roots,
            newspaper::commands::check_newspaper_snapshot_root,
            newspaper::commands::reconnect_newspaper_snapshot_root,
            newspaper::commands::open_newspaper_snapshot_root,
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
            coursera::job::reconcile_after_restart(&connection, commands::now_unix_timestamp())?;
            // Self-heal the built-in newspaper catalog on every startup.
            // Fresh databases and intact v0.2.7 installs hit the no-op path
            // (one COUNT(*)); v0.2.7 installs whose users clicked Reset
            // World Journal database get the 13 built-in editions
            // restored here without any user action.
            let reseeded = newspaper::storage::ensure_catalog_populated(
                &connection,
                commands::now_unix_timestamp(),
            )?;
            if reseeded {
                diagnostics.record(app::database_diagnostics::DatabaseDiagnosticInput {
                    kind: app::database_diagnostics::DatabaseDiagnosticKind::Initialization,
                    operation: "ensure_newspaper_catalog_populated",
                    provider: app::database_diagnostics::DatabaseProvider::Newspaper,
                    workflow_id: None,
                    elapsed: std::time::Duration::ZERO,
                    queue_depth: 0,
                    outcome: app::database_diagnostics::DatabaseDiagnosticOutcome::Ok,
                    error_class: None,
                });
            }
            drop(connection);
            let writer =
                app::database_writer::DatabaseWriter::start(db_path.clone(), diagnostics.clone())?;
            let workflow_runtime = workflow::WorkflowRuntime::new(writer.clone());
            let coursera_cancellation = Arc::new(AtomicBool::new(false));
            let coursera_data_dir =
                db_path
                    .parent()
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "database path has no parent directory",
                        )
                    })?;
            workflow_runtime.register_executor(Arc::new(
                coursera::executor::CourseraDownloadExecutor {
                    data_dir: coursera_data_dir,
                    cancellation: Arc::clone(&coursera_cancellation),
                },
            ));
            let youtube_live = Arc::new(youtube::live::YouTubeLiveHandle::default());
            youtube_live.bind_app(app.handle().clone());
            workflow_runtime.register_executor(Arc::new(
                youtube::kernel::YoutubeDownloadExecutor {
                    live: Arc::clone(&youtube_live),
                },
            ));
            let linkedin_cancellation = Arc::new(AtomicBool::new(false));
            let linkedin_paused = Arc::new(AtomicBool::new(false));
            let linkedin_session_token = Arc::new(Mutex::new(None));
            let linkedin_token_path = db_path.with_file_name("linkvault.li_at.dpapi");
            workflow_runtime.register_executor(Arc::new(executor::LinkedInDownloadExecutor {
                db_path: db_path.clone(),
                token_path: linkedin_token_path,
                cancellation: Arc::clone(&linkedin_cancellation),
                paused: Arc::clone(&linkedin_paused),
                session_token: Arc::clone(&linkedin_session_token),
            }));
            let newspaper_cancellation = Arc::new(AtomicBool::new(false));
            workflow_runtime.register_executor(Arc::new(
                newspaper::executor::NewspaperDownloadExecutor {
                    db_path: db_path.clone(),
                    cancellation: Arc::clone(&newspaper_cancellation),
                },
            ));
            workflow_runtime.reconcile_coursera_after_restart(commands::now_unix_timestamp())?;
            workflow_runtime.reconcile_youtube_after_restart(commands::now_unix_timestamp())?;
            workflow_runtime.reconcile_linkedin_after_restart(commands::now_unix_timestamp())?;
            workflow_runtime.reconcile_newspaper_after_restart(commands::now_unix_timestamp())?;
            workflow_runtime.start_supervisor()?;
            let clipping_layout = newspaper::clipping_assets::ClippingAssetLayout::new(
                storage::resolve_newspaper_clippings_root()?,
            );
            let clipping_service = newspaper::clipping_service::ClippingService::new(
                db_path.clone(),
                writer.clone(),
                clipping_layout,
                diagnostics.clone(),
            );
            let _recovery_summary =
                newspaper::clipping_startup::recover_and_schedule_reconciliation(
                    &clipping_service,
                    &diagnostics,
                    commands::now_unix_timestamp(),
                );
            app.manage(diagnostics);
            app.manage(writer);
            app.manage(workflow_runtime);
            app.manage(clipping_service);
            app.manage(commands::LinkVaultState::with_shared_flags(
                db_path.clone(),
                linkedin_cancellation,
                linkedin_paused,
                linkedin_session_token,
            ));
            app.manage(coursera::commands::CourseraState::with_cancellation(
                db_path.clone(),
                coursera_cancellation,
            ));
            app.manage(youtube::commands::YouTubeState::new(db_path.clone()));
            app.manage(youtube::commands::YouTubePlanStore::default());
            app.manage(youtube_live);
            app.manage(newspaper::thumbnails::ThumbnailCoordinator::new(
                db_path.clone(),
            ));
            app.manage(newspaper::commands::NewspaperState::with_cancellation(
                db_path,
                newspaper_cancellation,
            ));
            newspaper::commands::schedule_page_dimension_backfill(app.handle());
            app.manage(app_updates::PendingUpdate::default());
            app.manage(CooperativeExit::default());

            // Embed the taskbar/tray icon so the binary has no runtime file
            // dependency. The tray keeps Show / Quit available after close.
            let icon_bytes = include_bytes!("../icons/icon-taskbar.png");
            if let Ok(decoded) = image::load_from_memory(icon_bytes) {
                let rgba = decoded.to_rgba8();
                let (w, h) = rgba.dimensions();
                let taskbar_icon = tauri::image::Image::new_owned(rgba.into_raw(), w, h);

                for (_, window) in app.webview_windows() {
                    let _ = window.set_icon(taskbar_icon.clone());
                }

                let show_item =
                    MenuItem::with_id(app, "show", "Show LinkedVault", true, None::<&str>)?;
                let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
                let separator = PredefinedMenuItem::separator(app)?;
                let tray_menu = Menu::with_items(app, &[&show_item, &separator, &quit_item])?;

                let _tray = tauri::tray::TrayIconBuilder::with_id("linkvault-main-tray")
                    .icon(taskbar_icon)
                    .icon_as_template(false)
                    .tooltip("LinkedVault")
                    .menu(&tray_menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "show" => {
                            show_main_window(app);
                        }
                        "quit" => {
                            request_cooperative_exit(app, ExitReason::Exit);
                        }
                        _ => {}
                    })
                    .build(app)?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                request_cooperative_exit(window.app_handle(), ExitReason::Close);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building LinkedVault");

    app.run(|handle, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            let coordinator = handle.state::<CooperativeExit>();
            if !coordinator.consume_exit_authorization() {
                api.prevent_exit();
                request_cooperative_exit(handle, ExitReason::Exit);
            }
        }
        tauri::RunEvent::Exit => {
            handle.state::<workflow::WorkflowRuntime>().shutdown();
            handle
                .state::<newspaper::clipping_service::ClippingService>()
                .shutdown_crop_service();
            let _ = handle
                .state::<app::database_writer::DatabaseWriter>()
                .shutdown();
        }
        _ => {}
    });
}

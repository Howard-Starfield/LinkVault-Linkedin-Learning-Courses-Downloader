//! Startup orchestration for clipping recovery.
//!
//! Transactional rows are repaired before the UI can observe them. Filesystem
//! reconciliation is intentionally delayed and moved off the runtime thread so
//! opening the application does not compete with the first render or database
//! initialization.

use std::time::Duration;

use crate::app::database_diagnostics::DatabaseDiagnostics;

use super::{clipping_recovery::StartupRecoverySummary, clipping_service::ClippingService};

/// A short quiet period lets the window and provider catalog finish starting
/// before managed clipping folders are enumerated.
pub const STARTUP_FOLDER_RECONCILIATION_DELAY: Duration = Duration::from_secs(5);
/// Yield between small note-mirror pages so startup repair remains background
/// maintenance rather than a burst of filesystem work.
pub const NOTE_MIRROR_BATCH_DELAY: Duration = Duration::from_millis(100);

pub fn recover_transactional_state(
    service: &ClippingService,
    diagnostics: &DatabaseDiagnostics,
    now: i64,
) -> StartupRecoverySummary {
    service.recover_startup(diagnostics, now)
}

pub fn recover_and_schedule_reconciliation(
    service: &ClippingService,
    diagnostics: &DatabaseDiagnostics,
    now: i64,
) -> StartupRecoverySummary {
    let summary = recover_transactional_state(service, diagnostics, now);
    schedule_managed_folder_reconciliation(service.clone(), diagnostics.clone());
    summary
}

pub fn schedule_managed_folder_reconciliation(
    service: ClippingService,
    diagnostics: DatabaseDiagnostics,
) {
    let _reconciliation_task = tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_FOLDER_RECONCILIATION_DELAY).await;
        let cleanup_service = service.clone();
        let cleanup_diagnostics = diagnostics.clone();
        let _worker_result = tauri::async_runtime::spawn_blocking(move || {
            cleanup_service.run_deferred_cleanup(&cleanup_diagnostics)
        })
        .await;

        let mut after_id = None;
        loop {
            let worker = service.clone();
            let worker_after = after_id.clone();
            let batch = tauri::async_runtime::spawn_blocking(move || {
                worker.reconcile_note_mirror_batch(
                    worker_after.as_deref(),
                    super::clipping_service::NOTE_MIRROR_RECONCILIATION_BATCH_SIZE,
                )
            })
            .await;
            let Ok(Ok(batch)) = batch else {
                break;
            };
            let Some(next_after) = batch.next_after else {
                break;
            };
            after_id = Some(next_after);
            tokio::time::sleep(NOTE_MIRROR_BATCH_DELAY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_folder_reconciliation_has_a_bounded_quiet_period() {
        assert_eq!(STARTUP_FOLDER_RECONCILIATION_DELAY, Duration::from_secs(5));
        assert!(STARTUP_FOLDER_RECONCILIATION_DELAY < Duration::from_secs(30));
        assert!(NOTE_MIRROR_BATCH_DELAY >= Duration::from_millis(50));
        assert!(NOTE_MIRROR_BATCH_DELAY <= Duration::from_millis(500));
    }
}

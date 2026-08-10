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
        let _worker_result = tauri::async_runtime::spawn_blocking(move || {
            service.run_deferred_cleanup(&diagnostics)
        })
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_folder_reconciliation_has_a_bounded_quiet_period() {
        assert_eq!(STARTUP_FOLDER_RECONCILIATION_DELAY, Duration::from_secs(5));
        assert!(STARTUP_FOLDER_RECONCILIATION_DELAY < Duration::from_secs(30));
    }
}

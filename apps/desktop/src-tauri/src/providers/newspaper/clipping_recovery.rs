//! Startup reconciliation and deferred cleanup for clipping aggregates
//! (specification 02 section 14, RECOVERY-001..005).
//!
//! Recovery runs after application database initialization and before any
//! clipping view claims ready state. It is idempotent: repeated runs reach
//! the same terminal states (AC-PERSIST-009). Diagnostics record only safe
//! IDs, states, operations, elapsed time, and error classes (RECOVERY-005).

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app::database_diagnostics::{
    DatabaseDiagnosticInput, DatabaseDiagnosticKind, DatabaseDiagnosticOutcome,
    DatabaseDiagnostics, DatabaseProvider,
};
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
use crate::cache::open_runtime;

use super::clipping_assets::ClippingAssetLayout;
use super::clipping_models::{ClippingAssetState, ClippingError, ClippingErrorCode};
use super::clipping_repository::{self as repository};

/// Missing canonical bytes recorded by creation recovery.
pub const ASSET_CREATION_INCOMPLETE: &str = "ASSET_CREATION_INCOMPLETE";
/// Orphan age thresholds (RECOVERY-004).
pub const ORPHAN_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
pub const QUARANTINE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Bounded per-launch cleanup budget per category (RECOVERY-004).
pub const CLEANUP_BUDGET_PER_CATEGORY: usize = 32;

fn write_context(operation: &'static str) -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation,
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupRecoverySummary {
    pub creating_marked_ready: usize,
    pub creating_marked_missing: usize,
    pub deletions_completed: usize,
    pub failures: usize,
}

/// Recover one creating-state clipping to a terminal state (RECOVERY-001).
/// Shared by startup reconciliation and the idempotent create retry path
/// (FR-IDEMPOTENCY-003). Never creates a second row or changes the ID.
pub fn recover_creating_id(
    db_path: &Path,
    writer: &DatabaseWriter,
    layout: &ClippingAssetLayout,
    clipping_id: &str,
    now: i64,
) -> Result<ClippingAssetState, ClippingError> {
    let target = {
        let connection = open_runtime(db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_creating_rows(&connection)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .into_iter()
            .find(|row| row.id == clipping_id)
    };
    let Some(target) = target else {
        // Not (or no longer) creating: report the current public state.
        let connection = open_runtime(db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        return repository::row_state(&connection, clipping_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound));
    };
    ClippingAssetLayout::validate_relative_path(&target.asset_relative_path)?;

    // 1. Canonical final file present and valid -> mark ready.
    if layout
        .verify_canonical(
            &target.id,
            target.asset_byte_count,
            target.asset_pixel_width,
            target.asset_pixel_height,
            &target.asset_checksum_sha256,
        )
        .is_ok()
    {
        mark_ready(writer, &target.id, now)?;
        return Ok(ClippingAssetState::Ready);
    }

    // 2. Complete staging file present and valid -> promote, then mark ready.
    let staging_path = layout.staging_complete_path(&target.id)?;
    if let Ok(metadata) = fs::symlink_metadata(&staging_path) {
        if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
            if let Ok(bytes) = fs::read(&staging_path) {
                let valid = ClippingAssetLayout::validate_canonical_bytes(
                    &bytes,
                    target.asset_byte_count,
                    target.asset_pixel_width,
                    target.asset_pixel_height,
                    &target.asset_checksum_sha256,
                )
                .is_ok();
                if valid
                    && layout.promote_staging(&target.id).is_ok()
                    && layout
                        .verify_canonical(
                            &target.id,
                            target.asset_byte_count,
                            target.asset_pixel_width,
                            target.asset_pixel_height,
                            &target.asset_checksum_sha256,
                        )
                        .is_ok()
                {
                    mark_ready(writer, &target.id, now)?;
                    return Ok(ClippingAssetState::Ready);
                }
            }
        }
    }

    // 3. Otherwise the asset is unrecoverable; preserve the row as missing.
    let id = target.id.clone();
    let moved = writer
        .execute(
            write_context("clipping_recovery_mark_missing"),
            move |connection| {
                repository::mark_missing_from_creating(
                    connection,
                    &id,
                    ASSET_CREATION_INCOMPLETE,
                    now,
                )
                .map_err(Into::into)
            },
        )
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
    if !moved {
        return Err(ClippingError::new(ClippingErrorCode::RecoveryFailed));
    }
    layout.discard_staging(&target.id);
    Ok(ClippingAssetState::Missing)
}

fn mark_ready(writer: &DatabaseWriter, clipping_id: &str, now: i64) -> Result<(), ClippingError> {
    let id = clipping_id.to_string();
    let moved = writer
        .execute(
            write_context("clipping_recovery_mark_ready"),
            move |connection| {
                repository::mark_ready_from_creating(connection, &id, now).map_err(Into::into)
            },
        )
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
    if moved {
        Ok(())
    } else {
        Err(ClippingError::new(ClippingErrorCode::RecoveryFailed))
    }
}

/// Complete one confirmed deletion (RECOVERY-002 / DELETE-STATE-002..004).
pub fn complete_delete_pending_id(
    db_path: &Path,
    writer: &DatabaseWriter,
    layout: &ClippingAssetLayout,
    clipping_id: &str,
) -> Result<(), ClippingError> {
    let still_pending = {
        let connection = open_runtime(db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        matches!(
            repository::row_state(&connection, clipping_id)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?,
            Some(ClippingAssetState::DeletePending)
        )
    };
    if !still_pending {
        return Ok(());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let trash_entry = layout.move_canonical_to_trash(clipping_id, nonce)?;
    layout.remove_thumbnail(clipping_id);
    let id = clipping_id.to_string();
    let deleted = writer
        .execute(write_context("clipping_delete_row"), move |connection| {
            repository::delete_if_pending(connection, &id).map_err(Into::into)
        })
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
    if !deleted {
        // Leave a retryable delete_pending row and trash entry; the next
        // recovery pass completes them.
        return Err(ClippingError::new(ClippingErrorCode::DeleteFailed));
    }
    if let Some(entry) = trash_entry {
        if layout.remove_trash_entry(&entry).is_err() {
            // Trash cleanup failures are retryable through orphan cleanup and
            // never recreate the deleted clipping.
            return Err(ClippingError::new(ClippingErrorCode::DeleteFailed));
        }
    }
    Ok(())
}

/// Bounded synchronous reconciliation executed at startup after database
/// initialization (RECOVERY-001/002). Creating and delete_pending rows are
/// expected to be very few; both sets are processed fully.
pub fn run_startup_recovery(
    db_path: &Path,
    writer: &DatabaseWriter,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    now: i64,
) -> StartupRecoverySummary {
    let started = std::time::Instant::now();
    let mut summary = StartupRecoverySummary::default();
    let outcome = (|| -> Result<(), ClippingError> {
        let (creating_ids, delete_pending_ids) = {
            let connection = open_runtime(db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            let creating = repository::load_creating_rows(&connection)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let deleting = repository::load_delete_pending_ids(&connection)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            (creating, deleting)
        };
        for id in creating_ids {
            match recover_creating_id(db_path, writer, layout, &id, now) {
                Ok(ClippingAssetState::Ready) => summary.creating_marked_ready += 1,
                Ok(_) => summary.creating_marked_missing += 1,
                Err(_) => summary.failures += 1,
            }
        }
        for id in delete_pending_ids {
            match complete_delete_pending_id(db_path, writer, layout, &id) {
                Ok(()) => summary.deletions_completed += 1,
                Err(_) => summary.failures += 1,
            }
        }
        Ok(())
    })();
    if outcome.is_err() {
        summary.failures = summary.failures.saturating_add(1);
    }
    diagnostics.record(DatabaseDiagnosticInput {
        kind: DatabaseDiagnosticKind::Recovery,
        operation: "clipping_startup_recovery",
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
        elapsed: started.elapsed(),
        queue_depth: 0,
        outcome: if outcome.is_err() {
            DatabaseDiagnosticOutcome::Error
        } else {
            DatabaseDiagnosticOutcome::Ok
        },
        error_class: None,
    });
    summary
}

/// Bounded deferred cleanup for orphaned managed directories (RECOVERY-004).
/// Scans only the managed clipping root - never user newspaper download
/// directories - and processes at most `CLEANUP_BUDGET_PER_CATEGORY` entries
/// per category so work resumes on later launches.
pub fn run_deferred_cleanup(
    db_path: &Path,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
) -> usize {
    let started = std::time::Instant::now();
    let mut processed = 0usize;
    let outcome = (|| -> Result<(), ClippingError> {
        let known_ids = {
            let connection = open_runtime(db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            repository::load_all_ids(&connection)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
        };
        let now_system = SystemTime::now();

        // Staging directories without a row older than the grace period move
        // to quarantine.
        processed += quarantine_orphans(
            layout,
            &layout.staging_dir()?,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
            "stale-staging",
        )?;

        // Canonical asset directories without a row older than the grace
        // period move to quarantine rather than immediate deletion.
        processed += quarantine_orphans(
            layout,
            &layout.assets_dir()?,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
            "stale-asset",
        )?;

        // Trash entries without a row older than the grace period may be
        // deleted outright; their rows were already removed by confirmed
        // deletion.
        processed += delete_orphan_trash(layout, &known_ids, now_system, ORPHAN_GRACE)?;

        // Quarantine entries are retained for seven days, then deleted.
        processed += delete_expired_quarantine(layout, now_system, QUARANTINE_RETENTION)?;

        Ok(())
    })();
    diagnostics.record(DatabaseDiagnosticInput {
        kind: DatabaseDiagnosticKind::Recovery,
        operation: "clipping_deferred_cleanup",
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
        elapsed: started.elapsed(),
        queue_depth: 0,
        outcome: if outcome.is_err() {
            DatabaseDiagnosticOutcome::Error
        } else {
            DatabaseDiagnosticOutcome::Ok
        },
        error_class: None,
    });
    processed
}

fn directory_age(entry: &fs::DirEntry, now: SystemTime) -> Option<Duration> {
    let metadata = entry.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    now.duration_since(modified).ok()
}

fn quarantine_orphans(
    layout: &ClippingAssetLayout,
    directory: &Path,
    known_ids: &[String],
    now: SystemTime,
    grace: Duration,
    reason: &'static str,
) -> Result<usize, ClippingError> {
    let mut processed = 0usize;
    let read = match fs::read_dir(directory) {
        Ok(read) => read,
        Err(_) => return Ok(0),
    };
    for entry in read.flatten() {
        if processed >= CLEANUP_BUDGET_PER_CATEGORY {
            break;
        }
        let is_dir = entry
            .metadata()
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if known_ids.iter().any(|id| name.starts_with(id.as_str())) {
            continue;
        }
        let Some(age) = directory_age(&entry, now) else {
            continue;
        };
        if age < grace {
            continue;
        }
        let timestamp = now
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or_default();
        if layout
            .quarantine_directory(&entry.path(), reason, timestamp)
            .is_ok()
        {
            processed += 1;
        }
    }
    Ok(processed)
}

fn delete_orphan_trash(
    layout: &ClippingAssetLayout,
    known_ids: &[String],
    now: SystemTime,
    grace: Duration,
) -> Result<usize, ClippingError> {
    let trash_root = layout.trash_dir()?;
    let mut processed = 0usize;
    let read = match fs::read_dir(&trash_root) {
        Ok(read) => read,
        Err(_) => return Ok(0),
    };
    for entry in read.flatten() {
        if processed >= CLEANUP_BUDGET_PER_CATEGORY {
            break;
        }
        let is_dir = entry
            .metadata()
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Trash entries are named <clipping-id>-<nonce>; keep entries whose
        // clipping ID still exists.
        let owned = known_ids.iter().any(|id| name.starts_with(id.as_str()));
        if owned {
            continue;
        }
        let Some(age) = directory_age(&entry, now) else {
            continue;
        };
        if age < grace {
            continue;
        }
        if layout.remove_trash_entry(&entry.path()).is_ok() {
            processed += 1;
        }
    }
    Ok(processed)
}

fn delete_expired_quarantine(
    layout: &ClippingAssetLayout,
    now: SystemTime,
    retention: Duration,
) -> Result<usize, ClippingError> {
    let quarantine_root = layout.quarantine_dir()?;
    let mut processed = 0usize;
    let read = match fs::read_dir(&quarantine_root) {
        Ok(read) => read,
        Err(_) => return Ok(0),
    };
    for entry in read.flatten() {
        if processed >= CLEANUP_BUDGET_PER_CATEGORY {
            break;
        }
        let Some(age) = directory_age(&entry, now) else {
            continue;
        };
        if age < retention {
            continue;
        }
        if layout.remove_quarantine_entry(&entry.path()).is_ok() {
            processed += 1;
        }
    }
    Ok(processed)
}
